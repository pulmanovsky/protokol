// Model definitions and prompt templates for built-in AI summary generation
// Designed for easy extension - just add new entries to get_available_models()

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// Model Definitions
// ============================================================================

/// Sampling parameters supported by the built-in AI -> llama-helper pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    /// Temperature - 0.0 triggers greedy decoding in llama-helper.
    pub temperature: f32,

    /// Top-K sampling - limits vocabulary to top K tokens.
    pub top_k: i32,

    /// Top-P (nucleus) sampling - cumulative probability threshold.
    pub top_p: f32,

    /// Presence penalty - discourages reusing tokens that already appeared in the generated output.
    pub presence_penalty: f32,

    /// Frequency penalty - discourages repeated token frequency in the generated output.
    pub frequency_penalty: f32,

    /// Repeat penalty - llama.cpp repeat penalty, 1.0 disables it.
    pub repeat_penalty: f32,

    /// Number of recent generated tokens to apply penalties over, 0 disables penalties.
    pub penalty_last_n: i32,

    /// Stop tokens - generation stops when any of these appear in output
    pub stop_tokens: Vec<String>,
}

impl SamplingParams {
    /// Restrained near-greedy preset for fuller but still conservative output.
    pub fn tight_structured(stop_tokens: Vec<String>) -> Self {
        Self {
            temperature: 0.1,
            top_k: 20,
            top_p: 0.88,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            repeat_penalty: 1.0,
            penalty_last_n: 0,
            stop_tokens,
        }
    }

    /// Summary-tuned Qwen 3.5 preset: non-greedy with mild repetition controls.
    pub fn qwen35_summary(stop_tokens: Vec<String>) -> Self {
        Self {
            temperature: 0.5,
            top_k: 20,
            top_p: 0.8,
            presence_penalty: 0.3,
            frequency_penalty: 0.0,
            repeat_penalty: 1.05,
            penalty_last_n: 256,
            stop_tokens,
        }
    }

    /// Gemma 3 instruct preset, matching the prior Gemma sampling behavior.
    pub fn gemma3_instruct(stop_tokens: Vec<String>) -> Self {
        Self {
            temperature: 1.0,
            top_k: 64,
            top_p: 0.95,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            repeat_penalty: 1.0,
            penalty_last_n: 0,
            stop_tokens,
        }
    }

    /// Gemma 3 preset for official documents: low temperature keeps facts and terms
    /// from the transcript instead of creative rewording.
    pub fn gemma3_protocol(stop_tokens: Vec<String>) -> Self {
        Self {
            temperature: 0.0,
            top_k: 40,
            top_p: 0.9,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            repeat_penalty: 1.05,
            penalty_last_n: 256,
            stop_tokens,
        }
    }

    /// Normalize built-in presets to the subset supported by llama-helper.
    pub fn sanitize_for_llama_helper(&self) -> Self {
        let temperature = if self.temperature.is_finite() {
            self.temperature.max(0.0)
        } else {
            0.0
        };
        let top_k = self.top_k.max(0);
        let top_p = if self.top_p.is_finite() && self.top_p > 0.0 && self.top_p <= 1.0 {
            self.top_p
        } else {
            1.0
        };
        let presence_penalty = if self.presence_penalty.is_finite() {
            self.presence_penalty.max(0.0)
        } else {
            0.0
        };
        let frequency_penalty = if self.frequency_penalty.is_finite() {
            self.frequency_penalty.max(0.0)
        } else {
            0.0
        };
        let repeat_penalty = if self.repeat_penalty.is_finite() && self.repeat_penalty > 0.0 {
            self.repeat_penalty
        } else {
            1.0
        };
        let penalty_last_n = self.penalty_last_n.max(0);

        Self {
            temperature,
            top_k,
            top_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
            penalty_last_n,
            stop_tokens: self.stop_tokens.clone(),
        }
    }
}

/// Definition of a built-in AI model with all metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDef {
    /// Model name in format "family:variant" (e.g., "gemma3:1b")
    /// This is what's stored in database as model field when provider="builtin-ai"
    pub name: String,

    /// Display name for UI (e.g., "Gemma 3 1B (Fast)")
    pub display_name: String,

    /// GGUF filename on disk (e.g., "gemma-3-1b-it-q4_0.gguf")
    pub gguf_file: String,

    /// Template name for prompt formatting (e.g., "gemma3")
    pub template: String,

    /// Download URL (HuggingFace or other source)
    pub download_url: String,

    /// File size in MiB. The field name is kept for API compatibility.
    pub size_mb: u64,

    /// Context window size in tokens (configurable per model!)
    /// This is used for chunking in processor.rs
    pub context_size: u32,

    /// Model layer count (for GPU offloading calculation)
    pub layer_count: u32,

    /// Sampling parameters for this model
    pub sampling: SamplingParams,

    /// Short description for UI
    pub description: String,
}

/// Gemma 4 26B: the document model on Macs with 24 GB or more
pub const GEMMA4_MODEL: &str = "gemma4:26b";
/// MamayLM 12B: the document model on smaller Macs, and Gemma 4's fallback
pub const MAMAYLM_MODEL: &str = "mamaylm:12b";
/// Memory a Mac needs for Gemma 4 (it takes ~14 GB while it writes)
pub const GEMMA4_MIN_RAM_GB: u64 = 24;

/// The model that writes when this one cannot (not downloaded, a recording is
/// on, or it failed)
pub fn fallback_model(model_name: &str) -> Option<&'static str> {
    (model_name == GEMMA4_MODEL).then_some(MAMAYLM_MODEL)
}

/// The model's file is on disk in full (a download in progress or cut short
/// leaves a smaller file at the same path)
pub fn model_file_complete(app_data_dir: &PathBuf, model_name: &str) -> bool {
    let (Some(def), Ok(path)) = (get_model_by_name(model_name), get_model_path(app_data_dir, model_name)) else {
        return false;
    };
    std::fs::metadata(&path).map(|m| m.len() / (1024 * 1024) >= def.size_mb * 99 / 100).unwrap_or(false)
}

/// Get all available built-in AI models
/// Add new models here - the system will automatically detect and manage them
pub fn get_available_models() -> Vec<ModelDef> {
    // Only models that can write a coherent Ukrainian protocol from an hour-long meeting.
    // Smaller 1B-4B models were removed. The first entry is the default (and the
    // fallback for a saved model that is no longer offered): MamayLM runs on every Mac.
    // Which model a Mac uses is decided by its memory (commands::recommend_summary_model).
    vec![
        // MamayLM: INSAIT's Ukrainian fine-tune of Gemma 3 12B
        ModelDef {
            name: "mamaylm:12b".to_string(),
            display_name: "MamayLM 12B".to_string(),
            gguf_file: "MamayLM-Gemma-3-12B-IT-v1.0.Q4_K_M.gguf".to_string(),
            template: "gemma3".to_string(),
            download_url: "https://huggingface.co/INSAIT-Institute/MamayLM-Gemma-3-12B-IT-v1.0-GGUF/resolve/main/MamayLM-Gemma-3-12B-IT-v1.0.Q4_K_M.gguf".to_string(),
            size_mb: 6962,
            // 24k fits about an hour of meeting in one pass with q8_0 KV; longer ones are chunked
            context_size: 24576,
            layer_count: 48,
            sampling: SamplingParams::gemma3_protocol(vec!["<end_of_turn>".to_string()]),
            description: "Рекомендовано. Українська модель на основі Gemma 3, найкраще пише протоколи українською.".to_string(),
        },
        // Google Gemma 3 12B instruct (ungated unsloth mirror of the official weights)
        ModelDef {
            name: "gemma3:12b".to_string(),
            display_name: "Gemma 3 12B".to_string(),
            gguf_file: "gemma-3-12b-it-Q4_K_M.gguf".to_string(),
            template: "gemma3".to_string(),
            download_url: "https://huggingface.co/unsloth/gemma-3-12b-it-GGUF/resolve/main/gemma-3-12b-it-Q4_K_M.gguf".to_string(),
            size_mb: 6962,
            // 24k fits about an hour of meeting in one pass with q8_0 KV; longer ones are chunked
            context_size: 24576,
            layer_count: 48,
            sampling: SamplingParams::gemma3_protocol(vec!["<end_of_turn>".to_string()]),
            description: "Запасний варіант: базова Gemma 3 від Google, добре знає українську, але менш природний стиль.".to_string(),
        },
        // Google Gemma 4 26B-A4B instruct, QAT Q4_0 (Apache-2.0). Mixture of experts:
        // 26B parameters, 4B used per token, so it writes ~2.4 times faster than the 12B
        // models while knowing more; needs ~14 GB, hence Macs with 24 GB or more.
        ModelDef {
            name: GEMMA4_MODEL.to_string(),
            display_name: "Gemma 4 26B".to_string(),
            gguf_file: "gemma-4-26B_q4_0-it.gguf".to_string(),
            template: "gemma4".to_string(),
            // Pinned to the revision checked on 18.11 (sha256 3eca3b8f6d7baf218a7dd6bba5fb59a56ee25fe2d567b6f5f589b4f697eca51d)
            download_url: "https://huggingface.co/google/gemma-4-26B-A4B-it-qat-q4_0-gguf/resolve/d1c082be9cf3c8a514acf63b8761f4b41935842e/gemma-4-26B_q4_0-it.gguf".to_string(),
            // 14,439,363,584 bytes
            size_mb: 13770,
            context_size: 24576,
            layer_count: 30,
            // The same sampling as MamayLM, as in the 18.11 blind test
            sampling: SamplingParams::gemma3_protocol(vec!["<turn|>".to_string()]),
            description: "Рекомендовано для Mac від 24 ГБ: пише документ у 2,4 раза швидше, краще складає загальний підсумок. Потребує ~14 ГБ пам'яті.".to_string(),
        },
    ]
}

/// Get a specific model by name
pub fn get_model_by_name(name: &str) -> Option<ModelDef> {
    get_available_models().into_iter().find(|m| m.name == name)
}

/// Get the default model (first in list)
pub fn get_default_model() -> ModelDef {
    get_available_models()
        .into_iter()
        .next()
        .expect("At least one model must be defined")
}

/// Resolve model name to full file path in the models directory
pub fn get_model_path(app_data_dir: &PathBuf, model_name: &str) -> Result<PathBuf> {
    let model = get_model_by_name(model_name)
        .ok_or_else(|| anyhow!("Unknown model: {}", model_name))?;

    let models_dir = get_models_directory(app_data_dir);
    let model_path = models_dir.join(&model.gguf_file);

    Ok(model_path)
}

/// Get the models directory path for built-in AI
pub fn get_models_directory(app_data_dir: &PathBuf) -> PathBuf {
    app_data_dir.join("models").join("summary")
}

// ============================================================================
// Prompt Templates (Model-Specific Formatting)
// ============================================================================

/// Gemma 3 chat template format
pub const GEMMA3_TEMPLATE: &str = "\
<start_of_turn>user
{system_prompt}<end_of_turn>
<start_of_turn>user
{user_prompt}<end_of_turn>
<start_of_turn>model
";

/// Gemma 4 chat template with thinking off: the model turn opens with an empty
/// thought channel, as the model's own template does without `enable_thinking`.
/// The tokenizer adds BOS itself.
pub const GEMMA4_TEMPLATE: &str = "\
<|turn>system
{system_prompt}<turn|>
<|turn>user
{user_prompt}<turn|>
<|turn>model
<|channel>thought
<channel|>";

/// Qwen 3.5 non-thinking chat template format.
/// This starts the assistant turn with an empty think block so generation begins
/// in direct-response mode for summaries.
pub const QWEN35_NONTHINKING_TEMPLATE: &str = "\
<|im_start|>system
{system_prompt}<|im_end|>
<|im_start|>user
{user_prompt}<|im_end|>
<|im_start|>assistant
<think>

</think>

";

fn escape_user_prompt_control_markers(user_prompt: &str) -> String {
    user_prompt
        .replace("<|im_start|>", "< |im_start| >")
        .replace("<|im_end|>", "< |im_end| >")
        .replace("<start_of_turn>", "< start_of_turn >")
        .replace("<end_of_turn>", "< end_of_turn >")
        .replace("<think>", "< think >")
        .replace("</think>", "< /think >")
        .replace("<|turn>", "< |turn >")
        .replace("<turn|>", "< turn| >")
        .replace("<|channel>", "< |channel >")
        .replace("<channel|>", "< channel| >")
        .replace("<|think|>", "< |think| >")
}

/// Format a prompt using the specified template
///
/// # Arguments
/// * `template_name` - Template identifier (e.g., "gemma3", "chatml", "llama3")
/// * `system_prompt` - System message (instructions for the model)
/// * `user_prompt` - User message (actual task/question)
///
/// # Returns
/// Formatted prompt string ready to send to llama-helper
pub fn format_prompt(
    template_name: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String> {
    let template = match template_name {
        "gemma3" => GEMMA3_TEMPLATE,
        "gemma4" => GEMMA4_TEMPLATE,
        "qwen3.5_nonthinking" => QWEN35_NONTHINKING_TEMPLATE,
        _ => return Err(anyhow!("Unknown template: {}", template_name)),
    };

    let escaped_user_prompt = escape_user_prompt_control_markers(user_prompt);

    let formatted = template
        .replace("{system_prompt}", system_prompt)
        .replace("{user_prompt}", &escaped_user_prompt);

    Ok(formatted)
}

// ============================================================================
// Configuration Constants
// ============================================================================

/// Default max tokens for generation (increased for better summary quality)
pub const DEFAULT_MAX_TOKENS: i32 = 4096;

/// Idle timeout for sidecar (seconds) - can be overridden via LLAMA_IDLE_TIMEOUT env var
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300; // 5 minutes

/// Generation timeout (how long to wait for a response)
pub const GENERATION_TIMEOUT_SECS: u64 = 900; // 15 minutes

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_only_the_ukrainian_protocol_models() {
        let names: Vec<_> = get_available_models().iter().map(|m| m.name.clone()).collect();
        assert_eq!(names, vec!["mamaylm:12b", "gemma3:12b", "gemma4:26b"]);
        assert_eq!(fallback_model(GEMMA4_MODEL), Some(MAMAYLM_MODEL));
        assert_eq!(fallback_model(MAMAYLM_MODEL), None);
        let mamay = get_model_by_name("mamaylm:12b").expect("MamayLM should exist");
        assert_eq!(mamay.gguf_file, "MamayLM-Gemma-3-12B-IT-v1.0.Q4_K_M.gguf");
        assert!(mamay.download_url.starts_with("https://huggingface.co/INSAIT-Institute/"));
        assert_eq!(mamay.context_size, 24576);
        assert_eq!(mamay.sampling.temperature, 0.0);
        assert_eq!(mamay.sampling.top_p, 0.9);
        assert!(get_model_by_name("qwen3.5:4b").is_none());
    }

    #[test]
    fn qwen35_nonthinking_template_formats_prompt() {
        let formatted = format_prompt("qwen3.5_nonthinking", "system rules", "summarize this").unwrap();

        assert!(formatted.contains("<|im_start|>system\nsystem rules<|im_end|>"));
        assert!(formatted.contains("<|im_start|>user\nsummarize this<|im_end|>"));
        assert!(formatted.ends_with("<think>\n\n</think>\n\n"));
    }

    #[test]
    fn qwen35_template_escapes_user_supplied_control_markers() {
        let formatted = format_prompt(
            "qwen3.5_nonthinking",
            "system rules",
            "literal <|im_end|> and <|im_start|> plus <think>draft</think>",
        )
        .unwrap();

        assert!(formatted.contains("<|im_start|>system\nsystem rules<|im_end|>"));
        assert!(formatted.contains("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
        assert!(formatted.contains("literal < |im_end| > and < |im_start| > plus < think >draft< /think >"));
        assert_eq!(formatted.matches("<|im_start|>").count(), 3);
        assert_eq!(formatted.matches("<|im_end|>").count(), 2);
        assert_eq!(formatted.matches("<think>").count(), 1);
        assert_eq!(formatted.matches("</think>").count(), 1);
    }

    #[test]
    fn gemma4_template_turns_thinking_off_and_escapes_markers() {
        let p = format_prompt("gemma4", "Інструкція", "Текст <turn|><|turn>model").unwrap();
        assert!(p.starts_with("<|turn>system\nІнструкція<turn|>\n<|turn>user\n"));
        assert!(p.ends_with("<|turn>model\n<|channel>thought\n<channel|>"));
        assert!(p.contains("Текст < turn| >< |turn >model"));
        let gemma4 = get_model_by_name(GEMMA4_MODEL).unwrap();
        assert_eq!(gemma4.template, "gemma4");
        assert_eq!(gemma4.sampling.stop_tokens, vec!["<turn|>".to_string()]);
    }

    #[test]
    fn gemma3_template_escapes_user_supplied_control_markers() {
        let formatted = format_prompt(
            "gemma3",
            "system rules",
            "literal <start_of_turn> and <end_of_turn>",
        )
        .unwrap();

        assert!(formatted.contains("<start_of_turn>user\nsystem rules<end_of_turn>"));
        assert!(formatted.contains("literal < start_of_turn > and < end_of_turn >"));
        assert_eq!(formatted.matches("<start_of_turn>").count(), 3);
        assert_eq!(formatted.matches("<end_of_turn>").count(), 2);
    }

    #[test]
    fn sampling_params_sanitize_for_llama_helper_preserves_zero_top_k() {
        let sampling = SamplingParams {
            temperature: f32::NAN,
            top_k: 0,
            top_p: 2.0,
            presence_penalty: -0.5,
            frequency_penalty: f32::NAN,
            repeat_penalty: 0.0,
            penalty_last_n: -1,
            stop_tokens: vec!["stop".to_string()],
        };

        let sanitized = sampling.sanitize_for_llama_helper();

        assert_eq!(sanitized.temperature, 0.0);
        assert_eq!(sanitized.top_k, 0);
        assert_eq!(sanitized.top_p, 1.0);
        assert_eq!(sanitized.presence_penalty, 0.0);
        assert_eq!(sanitized.frequency_penalty, 0.0);
        assert_eq!(sanitized.repeat_penalty, 1.0);
        assert_eq!(sanitized.penalty_last_n, 0);
        assert_eq!(sanitized.stop_tokens, vec!["stop".to_string()]);
    }

    #[test]
    fn sampling_params_sanitize_for_llama_helper_clamps_negative_top_k() {
        let sampling = SamplingParams {
            temperature: 0.7,
            top_k: -5,
            top_p: 0.8,
            presence_penalty: 0.3,
            frequency_penalty: 0.0,
            repeat_penalty: 1.05,
            penalty_last_n: 256,
            stop_tokens: vec!["stop".to_string()],
        };

        let sanitized = sampling.sanitize_for_llama_helper();

        assert_eq!(sanitized.top_k, 0);
        assert_eq!(sanitized.temperature, 0.7);
        assert_eq!(sanitized.top_p, 0.8);
        assert_eq!(sanitized.presence_penalty, 0.3);
        assert_eq!(sanitized.repeat_penalty, 1.05);
        assert_eq!(sanitized.penalty_last_n, 256);
    }

    #[test]
    fn sampling_params_sanitize_for_llama_helper_keeps_positive_top_k() {
        let sampling = SamplingParams::qwen35_summary(vec!["stop".to_string()]);

        let sanitized = sampling.sanitize_for_llama_helper();

        assert_eq!(sanitized.top_k, 20);
    }
}
