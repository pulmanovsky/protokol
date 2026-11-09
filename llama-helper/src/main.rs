use std::io::{self, BufRead, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use encoding_rs;
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use serde::{Deserialize, Serialize};

// ============================================================================
// Protocol Messages (JSON over stdin/stdout)
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Generate {
        prompt: String,
        max_tokens: Option<i32>,
        context_size: Option<u32>,
        model_path: Option<String>,
        // Sampling parameters
        temperature: Option<f32>,
        top_k: Option<i32>,
        top_p: Option<f32>,
        presence_penalty: Option<f32>,
        frequency_penalty: Option<f32>,
        repeat_penalty: Option<f32>,
        penalty_last_n: Option<i32>,
        stop_tokens: Option<Vec<String>>,
        /// The answer must match this JSON schema (turned into a grammar)
        #[serde(default)]
        json_schema: Option<String>,
        /// Or this GBNF grammar (root rule «root»)
        #[serde(default)]
        grammar: Option<String>,
    },
    Ping,
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Response { text: String, error: Option<String> },
    Pong,
    Goodbye,
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SamplingConfig {
    temperature: f32,
    top_k: i32,
    top_p: f32,
    presence_penalty: f32,
    frequency_penalty: f32,
    repeat_penalty: f32,
    penalty_last_n: i32,
}

impl SamplingConfig {
    fn from_request(
        temperature: Option<f32>,
        top_k: Option<i32>,
        top_p: Option<f32>,
        presence_penalty: Option<f32>,
        frequency_penalty: Option<f32>,
        repeat_penalty: Option<f32>,
        penalty_last_n: Option<i32>,
    ) -> Self {
        let temperature = temperature.unwrap_or(1.0);
        let temperature = if temperature.is_finite() {
            temperature.max(0.0)
        } else {
            0.0
        };
        let top_k = top_k.unwrap_or(64).max(1);
        let top_p = top_p.unwrap_or(0.95);
        let top_p = if top_p.is_finite() && top_p > 0.0 && top_p <= 1.0 {
            top_p
        } else {
            1.0
        };
        let presence_penalty = presence_penalty.unwrap_or(0.0);
        let presence_penalty = if presence_penalty.is_finite() {
            presence_penalty.max(0.0)
        } else {
            0.0
        };
        let frequency_penalty = frequency_penalty.unwrap_or(0.0);
        let frequency_penalty = if frequency_penalty.is_finite() {
            frequency_penalty.max(0.0)
        } else {
            0.0
        };
        let repeat_penalty = repeat_penalty.unwrap_or(1.0);
        let repeat_penalty = if repeat_penalty.is_finite() && repeat_penalty > 0.0 {
            repeat_penalty
        } else {
            1.0
        };
        let penalty_last_n = penalty_last_n.unwrap_or(0).max(0);

        Self {
            temperature,
            top_k,
            top_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
            penalty_last_n,
        }
    }

    fn uses_penalties(&self) -> bool {
        self.penalty_last_n > 0
            && (self.presence_penalty > 0.0
                || self.frequency_penalty > 0.0
                || (self.repeat_penalty - 1.0).abs() > f32::EPSILON)
    }
}

// ============================================================================
// VRAM Detection and GPU Layer Calculation
// ============================================================================

/// Detect available VRAM in GB
fn detect_vram_gb() -> f32 {
    #[cfg(feature = "metal")]
    {
        // macOS Metal: Query recommended max working set size
        if let Some(vram) = detect_metal_vram() {
            eprintln!("Metal VRAM detected: {:.2} GB", vram);
            return vram;
        }
    }

    #[cfg(feature = "cuda")]
    {
        // NVIDIA CUDA: Query device memory
        if let Some(vram) = detect_cuda_vram() {
            eprintln!("CUDA VRAM detected: {:.2} GB", vram);
            return vram;
        }
    }

    /// TODO: Vulkan VRAM detection

    eprintln!("VRAM detection not available, using conservative estimate");
    4.0 // Conservative fallback
}

#[cfg(feature = "metal")]
fn detect_metal_vram() -> Option<f32> {
    if let Ok(output) = std::process::Command::new("sysctl")
        .arg("hw.memsize")
        .output()
    {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            if let Some(bytes_str) = stdout.split(':').nth(1) {
                if let Ok(bytes) = bytes_str.trim().parse::<u64>() {
                    let gb = bytes as f32 / (1024.0 * 1024.0 * 1024.0);
                    // Assume GPU can use ~60% of system memory on Apple Silicon
                    return Some(gb * 0.6);
                }
            }
        }
    }
    None
}

#[cfg(feature = "cuda")]
fn detect_cuda_vram() -> Option<f32> {
    // Use nvidia-smi to query VRAM
    if let Ok(output) = std::process::Command::new("nvidia-smi")
        .args(&["--query-gpu=memory.free", "--format=csv,noheader,nounits"])
        .output()
    {
        if let Ok(stdout) = String::from_utf8(output.stdout) {
            if let Ok(mb) = stdout.trim().parse::<f32>() {
                return Some(mb / 1024.0); // Convert MB to GB
            }
        }
    }
    None
}

/// Upper bound for a request's context: enough for an hour-long transcript plus the answer
const MAX_CONTEXT_TOKENS: u32 = 24576;
/// Smallest context we create, so short prompts still have room to answer
const MIN_CONTEXT_TOKENS: u32 = 4096;
/// Passed as n_gpu_layers to mean "all layers"
const ALL_LAYERS: u32 = 999;

/// Context for one request: prompt + answer budget + margin, rounded up to 1024,
/// capped by the model's configured size and MAX_CONTEXT_TOKENS.
fn request_context_size(prompt_tokens: usize, max_tokens: i32, configured: u32) -> u32 {
    let cap = configured.min(MAX_CONTEXT_TOKENS).max(MIN_CONTEXT_TOKENS);
    let wanted = prompt_tokens as u32 + max_tokens.max(0) as u32 + 256;
    let rounded = wanted.div_ceil(1024) * 1024;
    rounded.clamp(MIN_CONTEXT_TOKENS, cap)
}

/// Calculate safe GPU layer count based on VRAM, model file size, and context size
fn calculate_gpu_layers(
    model_path: &PathBuf,
    model_layers: u32,
    vram_gb: f32,
    context_size: u32,
) -> u32 {
    let file_size_gb = std::fs::metadata(model_path)
        .map(|m| m.len() as f32 / 1024.0 / 1024.0 / 1024.0)
        .unwrap_or(0.0);

    if file_size_gb == 0.0 {
        eprintln!("⚠️ Could not determine model file size, using conservative default");
        return 0;
    }

    // Heuristic: Estimate KV cache size
    // 7B models (approx > 2.5GB) usually have 4096 hidden dim -> ~256MB per 1k context
    // 1B models (approx < 2.5GB) usually have 2048 hidden dim -> ~128MB per 1k context
    // (q8_0 KV cache and compact sliding-window cache: about a quarter of an f16 full cache)
    let kv_per_1k_gb = if file_size_gb > 2.5 { 0.0625 } else { 0.03 };
    let total_kv_gb = (context_size as f32 / 1000.0) * kv_per_1k_gb;

    // Safety buffer (500MB) for OS/Display
    let safe_vram = vram_gb - 0.5;

    // For debugging
    eprintln!("📊 VRAM Analysis:");
    eprintln!("   • Available: {:.2} GB", vram_gb);
    eprintln!("   • Safe Limit: {:.2} GB", safe_vram);
    eprintln!("   • Model Weights: {:.2} GB", file_size_gb);
    eprintln!(
        "   • KV Cache ({} ctx): {:.2} GB",
        context_size, total_kv_gb
    );

    if safe_vram <= 0.0 {
        eprintln!("⚠️ No safe VRAM available, using CPU only");
        return 0;
    }

    // Everything fits: offload all layers. The layer count below is only a size
    // heuristic and undercounts large models (Gemma 3 12B has 48 layers, not 33).
    if file_size_gb + total_kv_gb <= safe_vram {
        eprintln!("✅ Full offload possible (model + KV {:.2} GB)", file_size_gb + total_kv_gb);
        return ALL_LAYERS;
    }

    // Calculate cost per layer
    let weight_per_layer = file_size_gb / model_layers as f32;
    let kv_per_layer = total_kv_gb / model_layers as f32;
    let total_per_layer = weight_per_layer + kv_per_layer;

    // Calculate how many layers fit
    let safe_layers = (safe_vram / total_per_layer).floor() as u32;
    let layers = safe_layers.min(model_layers);

    eprintln!(
        "   • Cost per layer: {:.2} MB (Weights) + {:.2} MB (KV) = {:.2} MB",
        weight_per_layer * 1024.0,
        kv_per_layer * 1024.0,
        total_per_layer * 1024.0
    );

    if layers < model_layers {
        eprintln!(
            "⚠️ Memory constrained. Offloading {}/{} layers ({:.1}%)",
            layers,
            model_layers,
            (layers as f32 / model_layers as f32) * 100.0
        );
    } else {
        eprintln!("✅ Full offload possible ({} layers)", layers);
    }

    layers
}

/// Get default GPU layer count with smart detection
fn get_default_gpu_layers(model_path: &PathBuf, context_size: u32) -> u32 {
    let vram = detect_vram_gb();
    // TODO: Use actual model metadata instead of heuristics
    // Heuristic: Estimate total layers based on file size
    // 7B models (Q4) are ~4.1GB and have ~32-35 layers
    // 1B models (Q4) are ~1.1GB and have ~20-28 layers
    let file_size_gb = std::fs::metadata(model_path)
        .map(|m| m.len() as f32 / 1024.0 / 1024.0 / 1024.0)
        .unwrap_or(0.0);

    let estimated_layers = if file_size_gb > 2.5 { 33 } else { 28 };

    calculate_gpu_layers(model_path, estimated_layers, vram, context_size)
}

// ============================================================================
// Model State Management
// ============================================================================

struct ModelState {
    backend: LlamaBackend,
    model: Option<LlamaModel>,
    model_path: Option<PathBuf>,
    context_size: u32,
    last_activity: Arc<AtomicU64>,
}

impl ModelState {
    fn new() -> Result<Self> {
        let backend = LlamaBackend::init().context("Failed to init LlamaBackend")?;
        Ok(Self {
            backend,
            model: None,
            model_path: None,
            context_size: 2048,
            last_activity: Arc::new(AtomicU64::new(Self::current_timestamp())),
        })
    }

    fn current_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn update_activity(&self) {
        self.last_activity
            .store(Self::current_timestamp(), Ordering::SeqCst);
    }

    fn seconds_since_activity(&self) -> u64 {
        Self::current_timestamp() - self.last_activity.load(Ordering::SeqCst)
    }

    fn load_model_if_needed(&mut self, model_path: PathBuf, context_size: u32) -> Result<()> {
        // Check if model is already loaded
        if let Some(ref loaded_path) = self.model_path {
            // The context is created per request, so a different context size needs no reload
            if loaded_path == &model_path {
                self.context_size = context_size;
                eprintln!("✓ Model already loaded");
                self.update_activity();
                return Ok(());
            }
        }

        eprintln!("📥 Loading model: {}", model_path.display());

        // Detect GPU layers
        let gpu_layers = get_default_gpu_layers(&model_path, context_size.min(MAX_CONTEXT_TOKENS));

        // Configure model parameters with GPU offload
        let model_params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
        let model_params = pin!(model_params);

        let model = LlamaModel::load_from_file(&self.backend, model_path.clone(), &model_params)
            .with_context(|| format!("unable to load model at {:?}", model_path))?;

        self.model = Some(model);
        self.model_path = Some(model_path);
        self.context_size = context_size;
        self.update_activity();

        eprintln!("✅ Model loaded successfully");
        Ok(())
    }

    fn generate(
        &mut self,
        prompt: String,
        max_tokens: i32,
        sampling: SamplingConfig,
        stop_tokens: Vec<String>,
        grammar: Option<String>,
    ) -> Result<String> {
        let start_time = Instant::now();
        let model = self.model.as_ref().context("Model not loaded")?;

        // Calculate thread count (conservative default: max(1, (Cores / 2) + 2))
        // This ensures the UI thread is never starved
        let threads: i32 = std::thread::available_parallelism()
            .map(|n| {
                let cores = n.get() as i32;
                ((cores / 2) + 2).max(1)
            })
            .unwrap_or(2);

        let tokens_list = model
            .str_to_token(&prompt, AddBos::Always)
            .with_context(|| "failed to tokenize prompt")?;

        eprintln!("📝 Tokenized prompt: {} tokens", tokens_list.len());

        // Size the context to this request: prompt + answer budget, capped.
        // A smaller context means a much smaller KV cache and no swapping.
        let n_ctx = request_context_size(tokens_list.len(), max_tokens, self.context_size);
        eprintln!("🧮 Context for this request: {} tokens (cap {})", n_ctx, self.context_size.min(MAX_CONTEXT_TOKENS));
        if tokens_list.len() as u32 >= n_ctx {
            anyhow::bail!("Prompt has {} tokens, more than the {} token context", tokens_list.len(), n_ctx);
        }

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(NonZeroU32::new(n_ctx).context("Invalid ctx size")?))
            .with_n_batch(n_ctx)
            .with_n_threads(threads)
            .with_n_threads_batch(threads)
            // Flash attention + 8-bit KV cache roughly halve KV memory with no visible quality loss
            .with_flash_attention_policy(llama_cpp_sys_2::LLAMA_FLASH_ATTN_TYPE_ENABLED)
            .with_type_k(KvCacheType::Q8_0)
            .with_type_v(KvCacheType::Q8_0)
            // Sliding-window layers (Gemma 3) keep only their window instead of the full context
            .with_swa_full(false);

        let mut ctx = model
            .new_context(&self.backend, ctx_params)
            .context("unable to create the llama_context")?;

        // Batch capacity matches the context so long prompts decode in one pass
        let batch_size = n_ctx as usize;
        let mut batch = LlamaBatch::new(batch_size, 1);

        let last_index: i32 = (tokens_list.len() - 1) as i32;
        for (i, token) in (0_i32..).zip(tokens_list.into_iter()) {
            let is_last = i == last_index;
            batch
                .add(token, i, &[0], is_last)
                .context("Failed to add token to batch")?;
        }

        ctx.decode(&mut batch).context("llama_decode() failed")?;
        let prompt_time = start_time.elapsed();

        let n_prompt_tokens = batch.n_tokens();
        let mut n_cur = n_prompt_tokens;
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut output = String::new();

        eprintln!("🔄 Starting generation (max_tokens: {})", max_tokens);

        use llama_cpp_2::sampling::LlamaSampler;

        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        let sampler = if sampling.temperature <= 0.0 {
            if sampling.uses_penalties() {
                LlamaSampler::chain_simple([
                    LlamaSampler::penalties(
                        sampling.penalty_last_n,
                        sampling.repeat_penalty,
                        sampling.frequency_penalty,
                        sampling.presence_penalty,
                    ),
                    LlamaSampler::greedy(),
                ])
            } else {
                LlamaSampler::chain_simple([LlamaSampler::greedy()])
            }
        } else if sampling.uses_penalties() {
            LlamaSampler::chain_simple([
                LlamaSampler::penalties(
                    sampling.penalty_last_n,
                    sampling.repeat_penalty,
                    sampling.frequency_penalty,
                    sampling.presence_penalty,
                ),
                LlamaSampler::top_k(sampling.top_k),
                LlamaSampler::top_p(sampling.top_p, 1),
                LlamaSampler::temp(sampling.temperature),
                LlamaSampler::dist(seed),
            ])
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::top_k(sampling.top_k),
                LlamaSampler::top_p(sampling.top_p, 1),
                LlamaSampler::temp(sampling.temperature),
                LlamaSampler::dist(seed),
            ])
        };
        // A grammar keeps the answer valid. Checking it against the whole
        // vocabulary for every token is slow (Gemma has 262k tokens), so the
        // token is chosen as usual and only checked; the grammar filters the
        // full list only when that token would break the answer.
        let mut grammar_sampler = match grammar.as_deref() {
            Some(g) => Some(LlamaSampler::grammar(model, g, "root").map_err(|e| anyhow::anyhow!("grammar: {e:?}"))?),
            None => None,
        };
        let mut sampler = pin!(sampler);

        loop {
            // Check if we've generated enough tokens
            if (n_cur - n_prompt_tokens) >= max_tokens {
                eprintln!("✓ Reached max_tokens limit");
                break;
            }

            // llama_sampler_sample already accepts the token: accepting it again
            // ran a grammar past its end (and counted repeats twice)
            let token = match grammar_sampler.as_mut() {
                None => sampler.as_mut().sample(&ctx, batch.n_tokens() - 1),
                Some(g) => {
                    use llama_cpp_2::token::data::LlamaTokenData;
                    use llama_cpp_2::token::data_array::LlamaTokenDataArray;
                    let idx = batch.n_tokens() - 1;
                    let mut all = ctx.token_data_array_ith(idx);
                    all.apply_sampler(&sampler);
                    let first = all.selected_token().context("no token selected")?;
                    let mut one = LlamaTokenDataArray::new(vec![LlamaTokenData::new(first, 0.0, 0.0)], false);
                    one.apply_sampler(g);
                    let chosen = if one.data[0].logit().is_finite() {
                        first
                    } else {
                        let mut all = ctx.token_data_array_ith(idx);
                        all.apply_sampler(g);
                        all.apply_sampler(&sampler);
                        all.selected_token().context("no token fits the grammar")?
                    };
                    g.accept(chosen);
                    sampler.as_mut().accept(chosen);
                    chosen
                }
            };

            if model.is_eog_token(token) {
                eprintln!(
                    "✓ End-of-generation token reached (generated {} chars)",
                    output.len()
                );
                break;
            }

            let output_bytes = match model.token_to_piece_bytes(token, 32, true, None) {
                Err(llama_cpp_2::TokenToStringError::InsufficientBufferSpace(size)) => {
                    let required_size: usize = size
                        .checked_neg()
                        .context("Invalid token piece buffer size")?
                        .try_into()
                        .context("Invalid token piece buffer size")?;
                    model.token_to_piece_bytes(token, required_size, true, None)
                }
                result => result,
            }
            .context("Failed to convert token to bytes")?;

            let mut token_text = String::with_capacity(32);
            let _ = decoder.decode_to_string(&output_bytes, &mut token_text, false);
            output.push_str(&token_text);

            // Check for model-specific stop tokens
            let mut should_stop = false;
            for stop_token in &stop_tokens {
                if output.contains(stop_token) {
                    eprintln!(
                        "✓ Stop token '{}' detected (generated {} chars)",
                        stop_token,
                        output.len()
                    );
                    // Remove the stop token from output
                    output = output.replace(stop_token, "").trim_end().to_string();
                    should_stop = true;
                    break;
                }
            }
            if should_stop {
                break;
            }

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .context("Failed to add generated token to batch")?;
            n_cur += 1;
            ctx.decode(&mut batch).context("failed to eval")?;
        }

        // Generation statistics
        let total_time = start_time.elapsed();
        let gen_time = total_time.saturating_sub(prompt_time);
        let output_tokens = (n_cur - n_prompt_tokens) as u64;
        let prompt_tokens = n_prompt_tokens as u64;

        let tokens_per_sec = if gen_time.as_secs_f64() > 0.0 {
            output_tokens as f64 / gen_time.as_secs_f64()
        } else {
            0.0
        };

        eprintln!("📊 Generation Statistics:");
        eprintln!("   • Prompt tokens: {}", prompt_tokens);
        eprintln!("   • Output tokens: {}", output_tokens);
        eprintln!("   • Prompt processing: {:.2}s", prompt_time.as_secs_f64());
        eprintln!("   • Generation time: {:.2}s", gen_time.as_secs_f64());
        eprintln!("   • Total time: {:.2}s", total_time.as_secs_f64());
        eprintln!("   • Speed: {:.2} tokens/sec", tokens_per_sec);

        self.update_activity();
        Ok(output)
    }
}

// ============================================================================
// Main Loop with Keep-Alive Protocol
// ============================================================================

fn send_response(response: &Response) -> Result<()> {
    let json = serde_json::to_string(response)?;
    println!("{}", json);
    io::stdout().flush()?;
    Ok(())
}

fn main() -> Result<()> {
    // Get idle timeout from environment variable (default 5 minutes)
    let idle_timeout_secs = std::env::var("LLAMA_IDLE_TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300); // 5 minutes default

    eprintln!(
        "🦙 llama-helper starting (idle timeout: {}s)",
        idle_timeout_secs
    );

    let mut state = ModelState::new()?;

    let stdin = io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut buffer = String::new();

    loop {
        // Check idle timeout
        if state.seconds_since_activity() > idle_timeout_secs {
            eprintln!("💤 Idle timeout reached, shutting down");
            send_response(&Response::Goodbye)?;
            break;
        }

        // Read line from stdin
        buffer.clear();
        match stdin_lock.read_line(&mut buffer) {
            Ok(0) => {
                // EOF reached
                eprintln!("📪 EOF received, shutting down");
                break;
            }
            Ok(_) => {
                let line = buffer.trim();
                if line.is_empty() {
                    continue;
                }

                // Parse request
                match serde_json::from_str::<Request>(line) {
                    Ok(Request::Generate {
                        prompt,
                        max_tokens,
                        context_size,
                        model_path,
                        temperature,
                        top_k,
                        top_p,
                        presence_penalty,
                        frequency_penalty,
                        repeat_penalty,
                        penalty_last_n,
                        stop_tokens,
                        json_schema,
                        grammar,
                    }) => {
                        let max_tokens = max_tokens.unwrap_or(512);
                        // A JSON schema becomes a grammar (llama.cpp's converter)
                        let grammar = match (json_schema, grammar) {
                            (Some(schema), _) => match llama_cpp_2::json_schema_to_grammar(&schema) {
                                Ok(g) => Some(g),
                                Err(e) => {
                                    send_response(&Response::Response { text: String::new(), error: Some(format!("Bad JSON schema: {e}")) })?;
                                    continue;
                                }
                            },
                            (None, g) => g,
                        };
                        let context_size = context_size.unwrap_or(2048);

                        let sampling = SamplingConfig::from_request(
                            temperature,
                            top_k,
                            top_p,
                            presence_penalty,
                            frequency_penalty,
                            repeat_penalty,
                            penalty_last_n,
                        );
                        let stop_tokens = stop_tokens.unwrap_or_else(Vec::new);

                        // Load model if path provided
                        if let Some(path_str) = model_path {
                            let path = PathBuf::from(path_str);
                            if let Err(e) = state.load_model_if_needed(path, context_size) {
                                send_response(&Response::Response {
                                    text: String::new(),
                                    error: Some(format!("Failed to load model: {}", e)),
                                })?;
                                continue;
                            }
                        }

                        // Generate response with sampling parameters
                        match state.generate(
                            prompt,
                            max_tokens,
                            sampling,
                            stop_tokens,
                            grammar,
                        ) {
                            Ok(text) => {
                                send_response(&Response::Response { text, error: None })?;
                            }
                            Err(e) => {
                                send_response(&Response::Response {
                                    text: String::new(),
                                    error: Some(format!("Generation failed: {}", e)),
                                })?;
                            }
                        }
                    }
                    Ok(Request::Ping) => {
                        state.update_activity();
                        send_response(&Response::Pong)?;
                    }
                    Ok(Request::Shutdown) => {
                        eprintln!("🛑 Shutdown requested");
                        send_response(&Response::Goodbye)?;
                        break;
                    }
                    Err(e) => {
                        eprintln!("❌ Failed to parse request: {}", e);
                        send_response(&Response::Error {
                            message: format!("Invalid request: {}", e),
                        })?;
                    }
                }
            }
            Err(e) => {
                eprintln!("❌ Error reading stdin: {}", e);
                break;
            }
        }
    }

    eprintln!("👋 llama-helper exiting");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_context_size_fits_prompt_and_answer_under_the_cap() {
        assert_eq!(request_context_size(100, 512, 32768), 4096);
        assert_eq!(request_context_size(7600, 4096, 32768), 12288);
        assert_eq!(request_context_size(30000, 4096, 32768), 24576);
        assert_eq!(request_context_size(7600, 4096, 8192), 8192);
    }

    #[test]
    fn generate_request_takes_a_json_schema() {
        let json = r#"{"type":"generate","prompt":"x","json_schema":"{\"type\":\"object\"}"}"#;
        let Request::Generate { json_schema, grammar, .. } = serde_json::from_str(json).unwrap() else { panic!() };
        assert_eq!(json_schema.as_deref(), Some("{\"type\":\"object\"}"));
        assert!(grammar.is_none());
        let g = llama_cpp_2::json_schema_to_grammar(r#"{"type":"object","properties":{"answer":{"enum":["так","ні"]}},"required":["answer"]}"#).unwrap();
        assert!(g.contains("root"));
    }

    #[test]
    fn generate_request_defaults_penalties_when_omitted() {
        let json = r#"{"type":"generate","prompt":"summarize","temperature":0.5,"top_k":20,"top_p":0.8}"#;
        let request: Request = serde_json::from_str(json).unwrap();
        let Request::Generate {
            temperature,
            top_k,
            top_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
            penalty_last_n,
            ..
        } = request else {
            panic!("expected generate request");
        };

        let sampling = SamplingConfig::from_request(
            temperature,
            top_k,
            top_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
            penalty_last_n,
        );

        assert_eq!(sampling.presence_penalty, 0.0);
        assert_eq!(sampling.frequency_penalty, 0.0);
        assert_eq!(sampling.repeat_penalty, 1.0);
        assert_eq!(sampling.penalty_last_n, 0);
        assert!(!sampling.uses_penalties());
    }

    #[test]
    fn generate_request_deserializes_qwen_penalties() {
        let json = r#"{"type":"generate","prompt":"summarize","temperature":0.5,"top_k":20,"top_p":0.8,"presence_penalty":0.3,"frequency_penalty":0.0,"repeat_penalty":1.05,"penalty_last_n":256}"#;
        let request: Request = serde_json::from_str(json).unwrap();
        let Request::Generate {
            temperature,
            top_k,
            top_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
            penalty_last_n,
            ..
        } = request else {
            panic!("expected generate request");
        };

        let sampling = SamplingConfig::from_request(
            temperature,
            top_k,
            top_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
            penalty_last_n,
        );

        assert_eq!(sampling.temperature, 0.5);
        assert_eq!(sampling.top_k, 20);
        assert_eq!(sampling.top_p, 0.8);
        assert_eq!(sampling.presence_penalty, 0.3);
        assert_eq!(sampling.frequency_penalty, 0.0);
        assert_eq!(sampling.repeat_penalty, 1.05);
        assert_eq!(sampling.penalty_last_n, 256);
        assert!(sampling.uses_penalties());
    }
}
