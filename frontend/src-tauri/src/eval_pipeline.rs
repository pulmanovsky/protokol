//! End-to-end quality check on a real recording, using the app's own pipeline.
//!
//! Ignored by default. Run with:
//!   PROTOKOL_EVAL_AUDIO=/path/meeting.m4a PROTOKOL_EVAL_OUT=/path/out \
//!   PROTOKOL_EVAL_STAGE=asr|protocol|all|export|replay|rescore|reference \
//!   PROTOKOL_LLAMA_HELPER=/path/llama-helper \
//!   cargo test --release --features coreml --lib eval_pipeline -- --ignored --nocapture
//!
//! Stage "asr" mirrors the audio import (decode, resample, VAD, 25 s splits, Whisper
//! with forced Ukrainian and the glossary prompt) and writes transcript.txt.
//! Stage "protocol" feeds that transcript, formatted like the meeting screen does,
//! to the Ukrainian protocol path with the default built-in model, and writes
//! protocol.md and protocol.docx. timings.txt collects wall times for both.

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

const MAX_SEGMENT_SAMPLES: usize = 25 * 16000; // same as audio import
const VAD_REDEMPTION_TIME_MS: u32 = 2000; // same as audio import
const SEGMENTS_FILE: &str = "segments.json";

fn app_data_dir() -> PathBuf {
    dirs::data_dir().expect("data dir").join(crate::identity::IDENTIFIER)
}

/// The dictionary and learned corrections recognition uses: those of the
/// Mac the meetings come from when PROTOKOL_EVAL_GLOSSARY_DIR names a folder
/// holding its glossary.txt, glossary_meta.json and corrections.txt, else
/// this Mac's own
fn init_eval_glossary(data_dir: &std::path::Path) {
    match std::env::var("PROTOKOL_EVAL_GLOSSARY_DIR") {
        Ok(dir) => crate::ukrainian::init_glossary(std::path::Path::new(&dir)),
        Err(_) => crate::ukrainian::init_glossary(data_dir),
    }
}

/// Where Whisper's models are: PROTOKOL_EVAL_MODELS_DIR (e.g. a folder with
/// a fine-tuned model saved under a catalogue name such as
/// ggml-large-v3.bin), else the app's own models folder
fn whisper_models_dir(data_dir: &std::path::Path) -> PathBuf {
    std::env::var("PROTOKOL_EVAL_MODELS_DIR").map(PathBuf::from).unwrap_or_else(|_| data_dir.join("models"))
}

/// Meetings a replay is limited to (PROTOKOL_EVAL_MEETINGS, comma-separated
/// ids); all when unset
fn wanted_meeting(name: &str) -> bool {
    match std::env::var("PROTOKOL_EVAL_MEETINGS") {
        Ok(list) => list.split(',').map(str::trim).any(|m| m == name),
        Err(_) => true,
    }
}

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(std::env::var("PROTOKOL_EVAL_OUT").expect("PROTOKOL_EVAL_OUT"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn log_timing(line: &str) {
    println!("{line}");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_dir().join("timings.txt"))
        .unwrap();
    writeln!(file, "{line}").unwrap();
}

fn hms(ms: f64) -> String {
    let s = (ms / 1000.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Same "[MM:SS] text" lines the meeting screen sends to the protocol generator
fn mmss(ms: f64) -> String {
    let s = (ms / 1000.0) as u64;
    format!("[{:02}:{:02}]", s / 60, s % 60)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Segment {
    start_ms: f64,
    end_ms: f64,
    text: String,
    confidence: f32,
}

async fn run_asr() {
    let audio = PathBuf::from(std::env::var("PROTOKOL_EVAL_AUDIO").expect("PROTOKOL_EVAL_AUDIO"));
    let model = std::env::var("PROTOKOL_EVAL_WHISPER").unwrap_or_else(|_| crate::config::DEFAULT_WHISPER_MODEL.to_string());
    let data_dir = app_data_dir();
    crate::ukrainian::init_glossary(&data_dir);
    log_timing(&format!("whisper model: {model}; glossary terms: {}", crate::ukrainian::glossary().len()));

    let started = Instant::now();
    let decoded = crate::audio::decoder::decode_audio_file_with_progress(&audio, None).expect("decode");
    let duration_s = decoded.duration_seconds;
    let samples = decoded.to_whisper_format();
    // Experiment knobs (defaults = the app's import settings)
    let env_f = |name: &str, default: f64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let redemption_ms = env_f("PROTOKOL_ASR_REDEMPTION_MS", VAD_REDEMPTION_TIME_MS as f64) as u32;
    let pad_ms = env_f("PROTOKOL_ASR_PAD_MS", 0.0);
    let window_s = env_f("PROTOKOL_ASR_WINDOW_S", 0.0); // 0 = split at silence like the app
    let overlap_s = env_f("PROTOKOL_ASR_OVERLAP_S", 0.0);
    log_timing(&format!(
        "asr knobs: redemption {redemption_ms} ms, pad {pad_ms} ms, window {window_s} s, overlap {overlap_s} s, temp_inc {}",
        std::env::var("PROTOKOL_ASR_TEMP_INC").unwrap_or_else(|_| "default".into())
    ));
    let mut speech = crate::audio::vad::get_speech_chunks(&samples, redemption_ms).expect("vad");
    if pad_ms > 0.0 {
        // Re-cut each speech segment from the full audio with some padding
        let pad = (pad_ms * 16.0) as usize;
        for seg in speech.iter_mut() {
            let start = ((seg.start_timestamp_ms * 16.0) as usize).saturating_sub(pad);
            let end = (((seg.end_timestamp_ms * 16.0) as usize) + pad).min(samples.len());
            seg.samples = samples[start..end].to_vec();
            seg.start_timestamp_ms = start as f64 / 16.0;
            seg.end_timestamp_ms = end as f64 / 16.0;
        }
    }
    let mut segments = Vec::new();
    for seg in &speech {
        if window_s > 0.0 {
            segments.extend(overlapping_windows(seg, window_s, overlap_s));
        } else {
            let max_samples = (env_f("PROTOKOL_ASR_MAX_S", (MAX_SEGMENT_SAMPLES / 16000) as f64) * 16000.0) as usize;
            segments.extend(crate::audio::common::split_segment_at_silence(seg, max_samples));
        }
    }
    let speech_s: f64 = speech.iter().map(|s| (s.end_timestamp_ms - s.start_timestamp_ms) / 1000.0).sum();
    log_timing(&format!(
        "audio {:.0}s; decode+resample+vad {:.1}s; {} speech segments ({:.0}s speech), {} after 25s splits",
        duration_s,
        started.elapsed().as_secs_f64(),
        speech.len(),
        speech_s,
        segments.len()
    ));

    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(whisper_models_dir(&data_dir))).expect("engine");
    engine.discover_models().await.expect("discover");
    let load_started = Instant::now();
    engine.load_model(&model).await.expect("load model");
    log_timing(&format!("whisper load {:.1}s", load_started.elapsed().as_secs_f64()));

    let asr_started = Instant::now();
    let mut out: Vec<Segment> = Vec::new();
    // Experiment knob: only the segments starting at these seconds ("77,203")
    let only: Option<Vec<u64>> = std::env::var("PROTOKOL_ASR_ONLY_S")
        .ok()
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect());
    for (i, seg) in segments.iter().enumerate() {
        if let Some(only) = &only {
            if !only.contains(&((seg.start_timestamp_ms / 1000.0) as u64)) {
                continue;
            }
        }
        let (text, confidence, _) = engine
            .transcribe_audio_with_confidence(seg.samples.clone(), Some(std::env::var("PROTOKOL_ASR_LANG").unwrap_or_else(|_| "uk".into())))
            .await
            .expect("transcribe");
        let mut text = text.trim().to_string();
        // Overlapping windows repeat the words at the seam: drop them
        if overlap_s > 0.0 {
            if let Some(prev) = out.last() {
                if seg.start_timestamp_ms < prev.end_ms {
                    text = drop_repeated_prefix(&prev.text, &text);
                }
            }
        }
        if (i + 1) % 20 == 0 {
            println!("  {}/{} segments, {:.0}s", i + 1, segments.len(), asr_started.elapsed().as_secs_f64());
        }
        // Same repeat filter as the audio import
        if crate::ukrainian::hallucination::is_repeated_segment(out.last().map(|s: &Segment| s.text.as_str()), &text) {
            continue;
        }
        if !text.is_empty() {
            out.push(Segment { start_ms: seg.start_timestamp_ms, end_ms: seg.end_timestamp_ms, text, confidence });
        }
    }
    let asr_s = asr_started.elapsed().as_secs_f64();
    log_timing(&format!(
        "ASR {:.1}s ({:.1} min) for {:.0}s audio: realtime factor {:.3} ({:.1}x faster than realtime)",
        asr_s,
        asr_s / 60.0,
        duration_s,
        asr_s / duration_s,
        duration_s / asr_s
    ));

    let transcript: String = out.iter().map(|s| format!("[{}] {}\n", hms(s.start_ms), s.text)).collect();
    std::fs::write(out_dir().join("transcript.txt"), transcript).unwrap();
    std::fs::write(out_dir().join(SEGMENTS_FILE), serde_json::to_string(&out).unwrap()).unwrap();
}

/// Fixed windows with overlap over one speech segment
fn overlapping_windows(seg: &crate::audio::vad::SpeechSegment, window_s: f64, overlap_s: f64) -> Vec<crate::audio::vad::SpeechSegment> {
    let window = (window_s * 16000.0) as usize;
    let step = (((window_s - overlap_s).max(1.0)) * 16000.0) as usize;
    if seg.samples.len() <= window {
        return vec![seg.clone()];
    }
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        let end = (pos + window).min(seg.samples.len());
        out.push(crate::audio::vad::SpeechSegment {
            samples: seg.samples[pos..end].to_vec(),
            start_timestamp_ms: seg.start_timestamp_ms + pos as f64 / 16.0,
            end_timestamp_ms: seg.start_timestamp_ms + end as f64 / 16.0,
            confidence: seg.confidence,
        });
        if end == seg.samples.len() {
            break;
        }
        pos += step;
    }
    out
}

/// Removes from `next` the longest word prefix that repeats the end of `prev`
fn drop_repeated_prefix(prev: &str, next: &str) -> String {
    let norm = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let p: Vec<&str> = prev.split_whitespace().collect();
    let n: Vec<&str> = next.split_whitespace().collect();
    let max = p.len().min(n.len()).min(40);
    for k in (2..=max).rev() {
        let tail: Vec<String> = p[p.len() - k..].iter().map(|w| norm(w)).collect();
        let head: Vec<String> = n[..k].iter().map(|w| norm(w)).collect();
        if tail == head {
            return n[k..].join(" ");
        }
    }
    next.to_string()
}

async fn run_protocol() {
    use crate::summary::processor::generate_meeting_summary;
    // PROTOKOL_EVAL_EXAMPLES: learned examples (JSON) as if from approved protocols
    let examples: Vec<crate::summary::learned::Example> = std::env::var("PROTOKOL_EVAL_EXAMPLES")
        .ok()
        .and_then(|f| serde_json::from_slice(&std::fs::read(f).ok()?).ok())
        .unwrap_or_default();
    log_timing(&format!("learned examples: {}", examples.len()));
    crate::summary::learned::set_current(examples, std::env::var("PROTOKOL_EVAL_MEETING").ok());
    let segments: Vec<Segment> =
        serde_json::from_str(&std::fs::read_to_string(out_dir().join(SEGMENTS_FILE)).expect("run the asr stage first")).unwrap();
    let text: String = segments.iter().map(|s| format!("{} {}", mmss(s.start_ms), s.text)).collect::<Vec<_>>().join("\n");
    // Corrections from «Словник» (and a suggestion list for this transcript)
    let text = crate::ukrainian::corrections::apply_corrections(&text);

    let data_dir = app_data_dir();
    crate::ukrainian::init_glossary(&data_dir);
    crate::summary::templates::set_bundled_templates_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates"));
    // PROTOKOL_EVAL_TYPES_DIR: the user's own meeting types (custom:<name>)
    if let Ok(dir) = std::env::var("PROTOKOL_EVAL_TYPES_DIR") {
        crate::summary::templates::set_custom_templates_dir(PathBuf::from(dir));
    }
    let template_id = std::env::var("PROTOKOL_EVAL_TEMPLATE").unwrap_or_else(|_| "meeting_summary_uk".to_string());
    let template = crate::summary::templates::get_template(&template_id).expect("template");
    let model = crate::summary::summary_engine::get_default_model();
    // Same budget the summary service uses for built-in models
    let token_threshold = model.context_size.saturating_sub(300) as usize;
    log_timing(&format!(
        "protocol model: {} ({}); template: {}; transcript ~{} tokens; threshold {}",
        model.name,
        model.gguf_file,
        template_id,
        crate::summary::rough_token_count(&text),
        token_threshold
    ));

    // As in the summary service: the recording date (here: now, like a fresh import)
    let custom_prompt = crate::summary::ukrainian_prompts::with_meeting_facts(
        "",
        chrono::Local::now(),
        std::env::var("PROTOKOL_EVAL_ORG").ok().as_deref(),
    );
    let client = reqwest::Client::new();
    let started = Instant::now();
    let result = generate_meeting_summary(
        &client,
        &crate::summary::LLMProvider::BuiltInAI,
        &model.name,
        "",
        &text,
        &custom_prompt,
        &template_id,
        &template,
        token_threshold,
        None,
        Some(&data_dir),
        None,
        Some("uk"),
        Some("uk"),
        None,
    )
    .await
    .expect("protocol generation");
    let llm_s = started.elapsed().as_secs_f64();
    log_timing(&format!(
        "LLM {:.1}s ({:.1} min), {} chunk(s)",
        llm_s,
        llm_s / 60.0,
        result.successful_chunk_count
    ));
    crate::summary::summary_engine::force_shutdown_sidecar().await.ok();

    std::fs::write(out_dir().join("protocol.md"), &result.final_markdown).unwrap();
    // Raw model answers for inspection, when PROTOKOL_EVAL_RAW is set (see log)
    let docx = crate::ukrainian::docx_export::build_docx(&result.final_markdown).unwrap();
    std::fs::write(out_dir().join("protocol.docx"), docx).unwrap();
}

/// Speaker diarization, then Whisper per speaker turn: transcript.txt with
/// «Спікер N» on every line, turns.txt and timings. Models from
/// PROTOKOL_SEG_MODEL and PROTOKOL_EMB_MODEL; PROTOKOL_CLUSTER_T sets the threshold.
async fn run_speakers() {
    let audio = PathBuf::from(std::env::var("PROTOKOL_EVAL_AUDIO").expect("PROTOKOL_EVAL_AUDIO"));
    let seg_model = PathBuf::from(std::env::var("PROTOKOL_SEG_MODEL").expect("PROTOKOL_SEG_MODEL"));
    let emb_model = PathBuf::from(std::env::var("PROTOKOL_EMB_MODEL").expect("PROTOKOL_EMB_MODEL"));
    let threshold = std::env::var("PROTOKOL_CLUSTER_T").ok().and_then(|v| v.parse().ok()).unwrap_or(crate::speakers::diarize::DEFAULT_CLUSTER_THRESHOLD);
    let data_dir = app_data_dir();
    crate::ukrainian::init_glossary(&data_dir);
    let decoded = crate::audio::decoder::decode_audio_file_with_progress(&audio, None).expect("decode");
    let duration_s = decoded.duration_seconds;
    let samples = decoded.to_whisper_format();
    log_timing(&format!("speakers: embedding model {}, threshold {threshold}", emb_model.display()));

    let started = Instant::now();
    let d = crate::speakers::diarize::diarize(&samples, &seg_model, &emb_model, threshold).expect("diarize");
    let diar_s = started.elapsed().as_secs_f64();
    log_diarization(&d, diar_s, duration_s);
    if std::env::var("PROTOKOL_SPEAKERS_NO_ASR").is_ok() {
        return;
    }

    let model = std::env::var("PROTOKOL_EVAL_WHISPER").unwrap_or_else(|_| crate::config::DEFAULT_WHISPER_MODEL.to_string());
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(whisper_models_dir(&data_dir))).expect("engine");
    engine.discover_models().await.expect("discover");
    engine.load_model(&model).await.expect("load model");
    let asr_started = Instant::now();
    let pieces = crate::speakers::turn_pieces(&samples, &d.turns);
    let mut lines: Vec<(f64, usize, String)> = Vec::new();
    for (speaker, piece) in &pieces {
        let (text, _, _) = engine.transcribe_audio_with_confidence(piece.samples.clone(), Some("uk".into())).await.expect("transcribe");
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        match lines.last_mut() {
            Some(last) if last.1 == *speaker => {
                last.2.push(' ');
                last.2.push_str(&text);
            }
            _ => lines.push((piece.start_timestamp_ms, *speaker, text)),
        }
    }
    log_timing(&format!("ASR by turns {:.1}s, {} pieces, {} speaker changes", asr_started.elapsed().as_secs_f64(), pieces.len(), lines.len()));
    // As in the app: voices with under 10 s of speech are «Репліки»
    let all: Vec<usize> = (0..d.speech_s.len()).collect();
    let short = crate::speakers::commands::short_voices(&all, &d.speech_s, &|_| false);
    let who = |k: usize| if short.contains(&k) { crate::speakers::commands::REMARKS.to_string() } else { crate::speakers::speaker_label(k) };
    let transcript: String = lines.iter().map(|(ms, k, t)| format!("[{}] {}: {}\n", hms(*ms), who(*k), t)).collect();
    std::fs::write(out_dir().join("transcript.txt"), transcript).unwrap();
}

/// Voices, turns, talk time and the closest pairs of voices, in timings.txt;
/// turns in turns.txt
fn log_diarization(d: &crate::speakers::Diarization, diar_s: f64, duration_s: f64) {
    let mut by_time: Vec<(usize, f64)> = d.speech_s.iter().copied().enumerate().collect();
    by_time.sort_by(|a, b| b.1.total_cmp(&a.1));
    log_timing(&format!(
        "diarization {:.1}s for {:.0}s audio: {} speakers, {} turns; speech per speaker: {}",
        diar_s,
        duration_s,
        d.centroids.len(),
        d.turns.len(),
        by_time.iter().map(|(k, s)| format!("{} {:.0}s", crate::speakers::speaker_label(*k), s)).collect::<Vec<_>>().join(", ")
    ));
    // Closest pairs of voices, to spot one person split in two
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for a in 0..d.centroids.len() {
        for b in (a + 1)..d.centroids.len() {
            pairs.push((crate::speakers::embedding::cosine(&d.centroids[a], &d.centroids[b]), a, b));
        }
    }
    pairs.sort_by(|x, y| y.0.total_cmp(&x.0));
    let close: Vec<String> = pairs.iter().take(8).map(|(s, a, b)| format!("{}~{} {:.2}", crate::speakers::speaker_label(*a), crate::speakers::speaker_label(*b), s)).collect();
    log_timing(&format!("closest voices: {}", close.join(", ")));
    let turns_txt: String = d.turns.iter().map(|t| format!("[{}–{}] {}\n", hms(t.start * 1000.0), hms(t.end * 1000.0), crate::speakers::speaker_label(t.speaker))).collect();
    std::fs::write(out_dir().join("turns.txt"), turns_txt).unwrap();
}

/// A simulated approval: PROTOKOL_EVAL_DRAFT (the app's protocol.md),
/// PROTOKOL_EVAL_FINAL (the approved one), the transcript (segments.json
/// with «Speaker: text» lines in the out dir), PROTOKOL_EVAL_TYPE (meeting
/// type name), PROTOKOL_EVAL_MEETING (an id). Writes examples.json.
fn run_learn() {
    use crate::summary::learned;
    let draft = learned::parse_doc(&std::fs::read_to_string(std::env::var("PROTOKOL_EVAL_DRAFT").expect("PROTOKOL_EVAL_DRAFT")).unwrap());
    let approved = learned::parse_doc(&std::fs::read_to_string(std::env::var("PROTOKOL_EVAL_FINAL").expect("PROTOKOL_EVAL_FINAL")).unwrap());
    let segments: Vec<Segment> = serde_json::from_str(&std::fs::read_to_string(out_dir().join(SEGMENTS_FILE)).unwrap()).unwrap();
    let mut turns: Vec<learned::Turn> = Vec::new();
    for s in &segments {
        let (speaker, text) = s.text.split_once(": ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
        match turns.last_mut() {
            Some(t) if t.speaker == speaker => {
                t.text.push(' ');
                t.text.push_str(&text);
            }
            _ => turns.push(learned::Turn { speaker, start: (s.start_ms / 1000.0) as u32, text }),
        }
    }
    let edits = learned::diff(&draft, &approved);
    let examples = learned::examples_from(
        &std::env::var("PROTOKOL_EVAL_MEETING").unwrap_or_else(|_| "eval".into()),
        &std::env::var("PROTOKOL_EVAL_TYPE").expect("PROTOKOL_EVAL_TYPE"),
        &turns,
        &Default::default(),
        &draft,
        &approved,
    );
    let count = |k: &str| edits.iter().filter(|e| format!("{e:?}").starts_with(k)).count();
    log_timing(&format!(
        "learned from approval: edits {} (tasks added {}, removed {}, reworded {}, reports rewritten {}); examples {}",
        edits.len(),
        count("TaskAdded"),
        count("TaskRemoved"),
        count("TaskReworded"),
        count("ReportReworded"),
        examples.len()
    ));
    std::fs::write(out_dir().join("examples.json"), serde_json::to_string_pretty(&examples).unwrap()).unwrap();
}

/// The import with word times, then diarization, then each word to the
/// voice heard at its time (no second transcription), as the app does. Writes
/// words_segments.json (the import text, for coverage), transcript.txt in the
/// speakers format and timings. Models as for `run_speakers`.
async fn run_words() {
    let audio = PathBuf::from(std::env::var("PROTOKOL_EVAL_AUDIO").expect("PROTOKOL_EVAL_AUDIO"));
    let seg_model = PathBuf::from(std::env::var("PROTOKOL_SEG_MODEL").expect("PROTOKOL_SEG_MODEL"));
    let emb_model = PathBuf::from(std::env::var("PROTOKOL_EMB_MODEL").expect("PROTOKOL_EMB_MODEL"));
    let data_dir = app_data_dir();
    crate::ukrainian::init_glossary(&data_dir);
    let decoded = crate::audio::decoder::decode_audio_file_with_progress(&audio, None).expect("decode");
    let samples = decoded.to_whisper_format();
    // PROTOKOL_EVAL_FOLDER: a recording's folder (system.m4a + channels.json):
    // the call and the user's own speech are transcribed apart, as in the app
    let folder = std::env::var("PROTOKOL_EVAL_FOLDER").ok().map(PathBuf::from);
    let levels = folder.as_ref().and_then(|f| crate::audio::channel_track::load_levels(f));
    let system = folder
        .as_ref()
        .map(|f| f.join(crate::audio::channel_track::SYSTEM_AUDIO_FILE))
        .filter(|p| p.exists() && levels.is_some())
        .and_then(|p| crate::audio::decoder::decode_audio_file_with_progress(&p, None).ok())
        .map(|d| d.to_whisper_format());
    log_timing(&format!("channels: {}", if system.is_some() { "call and own speech apart" } else { "mix" }));
    let segments = crate::speakers::words::channel_pieces(&samples, system.as_deref(), levels.as_ref()).expect("pieces");
    let started = Instant::now();
    let mut pieces: Vec<crate::speakers::words::PieceWords> = Vec::new();
    let mut out: Vec<Segment> = Vec::new();
    let mut previous: Option<String> = None;
    // PROTOKOL_EVAL_WORDS_FILE: word times saved by the app (words.json),
    // to test speaker assignment without transcribing again
    let saved = std::env::var("PROTOKOL_EVAL_WORDS_FILE").ok().and_then(|f| serde_json::from_slice::<Vec<crate::speakers::words::PieceWords>>(&std::fs::read(f).ok()?).ok());
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(whisper_models_dir(&data_dir))).expect("engine");
    let from_file = saved.is_some();
    if let Some(saved) = saved {
        pieces = saved;
        out = pieces.iter().map(|p| Segment { start_ms: p.start * 1000.0, end_ms: p.end * 1000.0, text: p.text.clone(), confidence: 1.0 }).collect();
    } else {
        engine.discover_models().await.expect("discover");
        engine.load_model(crate::config::DEFAULT_WHISPER_MODEL).await.expect("load");
    }
    for seg in segments.iter().filter(|_| !from_file) {
        if seg.samples.len() < 1600 {
            continue;
        }
        let Some(piece) =
            crate::speakers::words::transcribe_piece(&engine, &seg.samples, seg.start_timestamp_ms / 1000.0, seg.end_timestamp_ms / 1000.0, Some("uk".into()))
                .await
                .expect("words")
        else {
            continue;
        };
        if crate::ukrainian::hallucination::is_repeated_segment(previous.as_deref(), &piece.text) {
            continue;
        }
        previous = Some(piece.text.clone());
        out.push(Segment { start_ms: seg.start_timestamp_ms, end_ms: seg.end_timestamp_ms, text: piece.text.clone(), confidence: 1.0 });
        pieces.push(piece);
    }
    let asr_s = started.elapsed().as_secs_f64();
    // Then who spoke when, as the app does after import
    let t = Instant::now();
    let d = crate::speakers::diarize::diarize(&samples, &seg_model, &emb_model, crate::speakers::diarize::DEFAULT_CLUSTER_THRESHOLD).expect("diarize");
    let diar_s = t.elapsed().as_secs_f64();
    log_diarization(&d, diar_s, decoded.duration_seconds);
    let waited = started.elapsed().as_secs_f64();
    let t = Instant::now();
    let rows = crate::speakers::words::assign(&pieces, &d.turns);
    let all: Vec<usize> = (0..d.speech_s.len()).collect();
    let short = crate::speakers::commands::short_voices(&all, &d.speech_s, &|_| false);
    let who = |k: usize| if short.contains(&k) { crate::speakers::commands::REMARKS.to_string() } else { crate::speakers::speaker_label(k) };
    let mut lines: Vec<(f64, usize, String)> = Vec::new();
    for (text, start, _, k) in &rows {
        match lines.last_mut() {
            Some(last) if last.1 == *k => {
                last.2.push(' ');
                last.2.push_str(text);
            }
            _ => lines.push((start * 1000.0, *k, text.clone())),
        }
    }
    let transcript: String = lines.iter().map(|(ms, k, t)| format!("[{}] {}: {}\n", hms(*ms), who(*k), t)).collect();
    std::fs::write(out_dir().join("transcript.txt"), transcript).unwrap();
    std::fs::write(out_dir().join("words_segments.json"), serde_json::to_string(&out).unwrap()).unwrap();
    crate::speakers::words::save(&out_dir(), &pieces).unwrap();
    log_timing(&format!(
        "ASR with word times {:.1}s; diarization {:.1}s; both {:.1}s; assignment {:.2}s; {} pieces, {} rows, {} speaker changes, {} voices",
        asr_s, diar_s, waited, t.elapsed().as_secs_f64(), pieces.len(), rows.len(), lines.len(), d.speech_s.len()
    ));
    let terms = crate::ukrainian::glossary_terms();
    let words: usize = pieces.iter().map(|p| p.words.len()).sum();
    let doubtful: usize = pieces.iter().map(|p| crate::speakers::words::doubtful_in(p, &terms).iter().filter(|d| **d).count()).sum();
    let auto: usize = pieces.iter().map(|p| p.auto.len()).sum();
    log_timing(&format!("doubtful words {doubtful} of {words} ({:.1}%), replaced by the dictionary {auto}", 100.0 * doubtful as f64 / words.max(1) as f64));
}

/// The transcript's time marks match the audio played for them: for a few
/// lines of a meeting (from a copy of the app database), 6 s of the audio
/// file from the line's time are transcribed and must share words with the
/// line's beginning. PROTOKOL_TIMELINE_DB, PROTOKOL_TIMELINE_MEETING.
async fn run_timeline() {
    use sqlx::Row;
    let db = std::env::var("PROTOKOL_TIMELINE_DB").expect("PROTOKOL_TIMELINE_DB");
    let meeting = std::env::var("PROTOKOL_TIMELINE_MEETING").expect("PROTOKOL_TIMELINE_MEETING");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{db}?mode=ro")).await.unwrap();
    let folder: String = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(&meeting).fetch_one(&pool).await.unwrap();
    let rows = sqlx::query("SELECT transcript, audio_start_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(&meeting).fetch_all(&pool).await.unwrap();
    let audio_path = crate::audio::retranscription::find_audio_file(std::path::Path::new(&folder)).unwrap();
    let samples = crate::audio::decoder::decode_audio_file(&audio_path).unwrap().to_whisper_format();
    let data_dir = app_data_dir();
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).unwrap();
    engine.discover_models().await.unwrap();
    engine.load_model(crate::config::DEFAULT_WHISPER_MODEL).await.unwrap();
    let stem = |w: &str| w.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase().chars().take(5).collect::<String>();
    let picks: Vec<usize> = (0..6).map(|i| i * (rows.len() - 1) / 5).collect();
    let mut ok = 0;
    for &i in &picks {
        let text: String = rows[i].get("transcript");
        let start: f64 = rows[i].get("audio_start_time");
        let from = (start * 16_000.0) as usize;
        let to = (from + 6 * 16_000).min(samples.len());
        let (heard, _, _) = engine.transcribe_audio_with_confidence(samples[from..to].to_vec(), Some("uk".into())).await.unwrap();
        let want: std::collections::HashSet<String> = text.split_whitespace().take(15).map(stem).filter(|s| s.chars().count() >= 4).collect();
        let shared = heard.split_whitespace().map(stem).filter(|s| want.contains(s)).count();
        let pass = shared >= 2;
        ok += pass as usize;
        log_timing(&format!("line {i} at {start:.1}s: {shared} shared words — {}", if pass { "ok" } else { "MISMATCH" }));
    }
    log_timing(&format!("timeline: {ok} of {} lines match the audio at their time mark", picks.len()));
    assert_eq!(ok, picks.len());
}

/// Checked lines of meetings as pieces of the recording with their right
/// text (see `quality::pairs`): `<out>/<meeting id>/NNNNN.wav` and
/// `pieces.jsonl` beside them. PROTOKOL_EVAL_DB: a copy of the app database
/// (made with `sqlite3 … ".backup …"` while the app is closed);
/// PROTOKOL_EVAL_MEETINGS: meeting ids, comma-separated (default: every
/// meeting with checked lines). Meeting titles are not written anywhere.
async fn run_export() {
    let db = std::env::var("PROTOKOL_EVAL_DB").expect("PROTOKOL_EVAL_DB");
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{db}?mode=ro")).await.unwrap();
    let wanted: Option<Vec<String>> = std::env::var("PROTOKOL_EVAL_MEETINGS").ok().map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
    let meetings: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, created_at, folder_path FROM meetings ORDER BY created_at").fetch_all(&pool).await.unwrap();
    let (mut total_s, mut total_pieces, mut used) = (0.0f64, 0usize, 0usize);
    for (id, created, folder) in meetings {
        if wanted.as_ref().is_some_and(|w| !w.contains(&id)) {
            continue;
        }
        let Some(folder) = folder else { continue };
        let pieces = match crate::quality::pairs::checked_pieces(&pool, &id).await {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => continue,
            Err(e) => {
                log_timing(&format!("{id}: lines not read ({e})"));
                continue;
            }
        };
        // Meetings copied from another Mac: their recording folders by name
        // under PROTOKOL_EVAL_RECORDINGS instead of the path saved there
        let folder = match (std::env::var("PROTOKOL_EVAL_RECORDINGS").ok(), std::path::Path::new(&folder).file_name()) {
            (Some(root), Some(name)) => PathBuf::from(root).join(name).to_string_lossy().to_string(),
            _ => folder,
        };
        let samples = match crate::audio::retranscription::find_audio_file(std::path::Path::new(&folder)).and_then(|a| crate::speakers::commands::decode_16k(&a)) {
            Ok(s) => s,
            Err(e) => {
                log_timing(&format!("{id}: no recording ({e})"));
                continue;
            }
        };
        let dir = out_dir().join(&id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut jsonl = String::new();
        let mut seconds = 0.0;
        for (k, p) in pieces.iter().enumerate() {
            let a = ((p.start * 16_000.0) as usize).min(samples.len());
            let b = ((p.end * 16_000.0) as usize).min(samples.len());
            if b <= a + 1600 {
                continue;
            }
            let file = format!("{k:05}.wav");
            crate::quality::wav::write(&dir.join(&file), &samples[a..b]).unwrap();
            jsonl.push_str(&serde_json::json!({ "file": file, "start": p.start, "end": p.end, "text": p.text, "lines": p.lines }).to_string());
            jsonl.push('\n');
            seconds += (b - a) as f64 / 16_000.0;
            total_pieces += 1;
        }
        std::fs::write(dir.join("pieces.jsonl"), jsonl).unwrap();
        used += 1;
        total_s += seconds;
        log_timing(&format!("{id} ({}): {} pieces, {:.1} min of checked speech", created.get(..10).unwrap_or(&created), pieces.len(), seconds / 60.0));
    }
    log_timing(&format!("export: {used} meetings, {total_pieces} pieces, {:.2} h of checked speech", total_s / 3600.0));
}

/// The app's recognition run again on exported pieces (see `run_export`;
/// PROTOKOL_EVAL_RECORDINGS there: a folder holding the recording folders of
/// meetings copied from another Mac).
/// Writes `replay-<tag>.jsonl`: for each piece the secretary's text, Whisper's
/// words with how sure it was and which ones the app underlines, and with
/// PROTOKOL_EVAL_PARAKEET=1 Parakeet's words too, plus the time each took.
/// Then measures it (see `run_rescore`). PROTOKOL_EVAL_TAG names the run
/// (default «whisper», «both» with Parakeet), PROTOKOL_EVAL_WHISPER the
/// model, PROTOKOL_EVAL_LIMIT caps the pieces per meeting (a quick look).
async fn run_replay() {
    let root = out_dir();
    let with_parakeet = std::env::var("PROTOKOL_EVAL_PARAKEET").is_ok_and(|v| v == "1");
    let tag = std::env::var("PROTOKOL_EVAL_TAG").unwrap_or_else(|_| if with_parakeet { "both" } else { "whisper" }.to_string());
    let limit: usize = std::env::var("PROTOKOL_EVAL_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let model = std::env::var("PROTOKOL_EVAL_WHISPER").unwrap_or_else(|_| crate::config::DEFAULT_WHISPER_MODEL.to_string());
    let data_dir = app_data_dir();
    init_eval_glossary(&data_dir);
    let terms = crate::ukrainian::glossary_terms();
    log_timing(&format!("replay «{tag}»: whisper model {model}; glossary terms: {}; learned corrections: {}", terms.len(), crate::ukrainian::corrections::corrections().len()));
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(whisper_models_dir(&data_dir))).expect("engine");
    engine.discover_models().await.expect("discover");
    engine.load_model(&model).await.expect("load");
    let parakeet = if with_parakeet { Some(load_parakeet(&data_dir).await) } else { None };
    let mut judge = if with_parakeet { load_judge(&data_dir) } else { None };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("pieces.jsonl").exists())
        .filter(|p| wanted_meeting(&p.file_name().unwrap_or_default().to_string_lossy()))
        .collect();
    dirs.sort();
    let started = Instant::now();
    let mut out = String::new();
    for dir in dirs {
        let meeting = dir.file_name().unwrap().to_string_lossy().to_string();
        let mut count = 0;
        for line in std::fs::read_to_string(dir.join("pieces.jsonl")).unwrap().lines().take(limit) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let (file, text) = (v["file"].as_str().unwrap(), v["text"].as_str().unwrap());
            let samples = crate::quality::wav::read(&dir.join(file)).unwrap();
            let seconds = samples.len() as f64 / 16_000.0;
            let mut record = serde_json::json!({ "meeting": meeting, "file": file, "seconds": seconds, "right": text });
            hear(&engine, parakeet.as_ref(), judge.as_mut(), &terms, &samples, &mut record).await;
            out.push_str(&record.to_string());
            out.push('\n');
            count += 1;
        }
        println!("{meeting}: {count} pieces replayed");
    }
    std::fs::write(root.join(format!("replay-{tag}.jsonl")), out).unwrap();
    log_timing(&format!("replay «{tag}» in {:.0} s", started.elapsed().as_secs_f64()));
    rescore(&root, &tag);
}

/// What the learned corrections («wrong => right», applied everywhere after
/// recognition) do to the text, measured on `replay-<tag>.jsonl` made with
/// them switched off (PROTOKOL_EVAL_GLOSSARY_DIR with an empty
/// corrections.txt) against the secretary's text, with the rules of
/// PROTOKOL_EVAL_RULES_DIR/corrections.txt. No models needed. For each rule:
/// how often it fires and how many mistakes it removes or adds; and the
/// mistakes under policies that skip risky rules:
/// - «ending»: one word whose right form differs only in the ending (a
///   grammatical form, right in one sentence and wrong in the next);
/// - «used»: the wrong form is a word the secretary herself left in the
///   checked text of another meeting.
/// Writes `rules-<tag>.json` (it holds the rules: stays on this Mac); the
/// summary is printed and goes to timings.txt, numbers only.
fn run_rules(root: &std::path::Path, tag: &str, dates: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>) {
    use crate::quality::score::{align, norm};
    use crate::ukrainian::corrections::{apply_corrections_with, applied_rules, same_word_forms};
    let rules_dir = std::env::var("PROTOKOL_EVAL_RULES_DIR").expect("PROTOKOL_EVAL_RULES_DIR: the folder with the secretary's corrections.txt");
    let rules: Vec<(String, String)> = std::fs::read_to_string(std::path::Path::new(&rules_dir).join("corrections.txt"))
        .expect("corrections.txt")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('!'))
        .map(|l| l.trim_start().trim_start_matches('?'))
        .filter_map(|l| l.split_once("=>").map(|(a, b)| (a.trim().to_string(), b.trim().to_string())))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty() && a != b)
        .collect();
    let text = std::fs::read_to_string(root.join(format!("replay-{tag}.jsonl"))).expect("replay file");
    let records: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let words_of = |t: &str| -> Vec<String> { t.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect() };
    let pieces: Vec<(String, String, Vec<String>)> = records
        .iter()
        .map(|r| {
            let heard = r["whisper"].as_array().map(|ws| ws.iter().filter_map(|w| w["w"].as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
            (r["meeting"].as_str().unwrap_or("?").to_string(), heard, words_of(r["right"].as_str().unwrap_or("")))
        })
        .collect();
    let mistakes = |heard: &str, right: &[String]| -> usize {
        let a = align(right, &words_of(heard));
        a.substituted + a.deleted + a.inserted
    };
    // The secretary's own words, per meeting
    let mut used: std::collections::HashMap<String, std::collections::HashSet<String>> = Default::default();
    for (m, _, right) in &pieces {
        used.entry(m.clone()).or_default().extend(right.iter().cloned());
    }
    let ending = |(w, r): &(String, String)| !w.contains(' ') && !r.contains(' ') && same_word_forms(w, r);
    // What the app now applies by itself (a name, abbreviation or nickname,
    // not another ending): `Correction::applies_by_itself`
    let name = |rule: &(String, String)| crate::ukrainian::corrections::Correction { wrong: rule.0.clone(), right: rule.1.clone(), enabled: true, hint_only: false }.applies_by_itself();
    let used_elsewhere = |rule: &(String, String), meeting: &str| {
        let w = norm(&rule.0);
        !rule.0.contains(' ') && used.iter().any(|(m, set)| m != meeting && set.contains(&w))
    };
    // Per rule: pieces where it fires, mistakes removed, mistakes added
    let mut per: Vec<(usize, usize, usize)> = vec![(0, 0, 0); rules.len()];
    let (mut base, mut all, mut no_ending, mut no_used, mut neither, mut names, mut words) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut firing_of: Vec<Vec<usize>> = Vec::with_capacity(pieces.len());
    // Meetings where a rule alone removes a mistake
    let mut fixes_in: Vec<std::collections::HashSet<String>> = vec![Default::default(); rules.len()];
    for (meeting, heard, right) in &pieces {
        words += right.len();
        let before = mistakes(heard, right);
        base += before;
        let firing: Vec<usize> = (0..rules.len()).filter(|&i| !applied_rules(heard, &rules[i..=i]).is_empty()).collect();
        firing_of.push(firing.clone());
        for &i in &firing {
            let after = mistakes(&apply_corrections_with(heard, &rules[i..=i]), right);
            per[i].0 += 1;
            if after < before {
                fixes_in[i].insert(meeting.clone());
            }
            if after < before { per[i].1 += before - after } else { per[i].2 += after - before }
        }
        let with = |keep: &dyn Fn(&(String, String)) -> bool| -> usize {
            let chosen: Vec<(String, String)> = firing.iter().map(|&i| rules[i].clone()).filter(|r| keep(r)).collect();
            mistakes(&apply_corrections_with(heard, &chosen), right)
        };
        all += with(&|_| true);
        no_ending += with(&|r| !ending(r));
        no_used += with(&|r| !used_elsewhere(r, meeting));
        neither += with(&|r| !ending(r) && !used_elsewhere(r, meeting));
        names += with(&|r| name(r) && !used_elsewhere(r, meeting));
    }
    let pct = |n: usize| 100.0 * n as f64 / words.max(1) as f64;
    let fired = per.iter().filter(|p| p.0 > 0).count();
    let helping = per.iter().filter(|p| p.1 > p.2).count();
    let hurting = per.iter().filter(|p| p.2 > p.1).count();
    let (removed, added): (usize, usize) = per.iter().fold((0, 0), |(a, b), p| (a + p.1, b + p.2));
    let kind = |pred: &dyn Fn(&(String, String)) -> bool| -> (usize, usize, usize, usize) {
        let idx: Vec<usize> = (0..rules.len()).filter(|&i| pred(&rules[i])).collect();
        (idx.len(), idx.iter().filter(|&&i| per[i].0 > 0).count(), idx.iter().map(|&i| per[i].1).sum(), idx.iter().map(|&i| per[i].2).sum())
    };
    let (e_n, e_f, e_rm, e_add) = kind(&|r| ending(r));
    let (n_n, n_f, n_rm, n_add) = kind(&|r| name(r));
    let (o_n, o_f, o_rm, o_add) = kind(&|r| !ending(r) && !name(r));
    let multi = rules.iter().filter(|r| r.0.contains(' ')).count();
    let lines = [
        format!("rules «{tag}»: {} rules ({} ending-only, {} of several words), {} pieces, {} words of the secretary's text", rules.len(), e_n, multi, pieces.len(), words),
        format!("  fired: {fired} rules; mistakes removed {removed}, added {added} (one rule at a time); rules helping {helping}, hurting {hurting}"),
        format!("  ending-only rules: {e_f} fired; removed {e_rm}, added {e_add}"),
        format!("  name/abbreviation rules: {n_n}, {n_f} fired; removed {n_rm}, added {n_add} · other one-word and phrase rules: {o_n}, {o_f} fired; removed {o_rm}, added {o_add}"),
        format!("  WER without rules {:.2}% · all rules {:.2}% · without ending-only {:.2}% · without «used» {:.2}% · without both {:.2}% · names only, not «used» {:.2}%", pct(base), pct(all), pct(no_ending), pct(no_used), pct(neither), pct(names)),
    ];
    for l in &lines {
        println!("{l}");
        log_timing(l);
    }
    // As it would have been in use: on each meeting only the rules learned
    // before it (learned_log.jsonl beside corrections.txt; meeting dates from
    // PROTOKOL_EVAL_DB) and never from that meeting itself, with «used» from
    // the checked text of earlier meetings only. Rules without a date (not
    // in the log): left out, or counted as older than every meeting.
    // Without the log, a rule counts as learned from the first meeting
    // where it fixes a word (rules that fix none: undated).
    let mut learned = learned_rules(std::path::Path::new(&rules_dir));
    let estimated = learned.is_empty() && !dates.is_empty();
    if estimated {
        for (r, fixes) in rules.iter().zip(&fixes_in) {
            if let Some((m, at)) = fixes.iter().filter_map(|m| dates.get(m.as_str()).map(|d| (m, *d))).min_by_key(|(_, d)| *d) {
                learned.insert((r.0.to_lowercase(), r.1.clone()), (at, Some(m.clone())));
            }
        }
    }
    if !learned.is_empty() {
        let info: Vec<Option<&(chrono::DateTime<chrono::Utc>, Option<String>)>> = rules.iter().map(|r| learned.get(&(r.0.to_lowercase(), r.1.clone()))).collect();
        let dated = info.iter().filter(|i| i.is_some()).count();
        let mut earlier: std::collections::HashMap<&str, std::collections::HashSet<String>> = Default::default();
        for m in used.keys() {
            let set = used
                .iter()
                .filter(|(o, _)| *o != m && match (dates.get(o.as_str()), dates.get(m.as_str())) {
                    (Some(a), Some(b)) => a < b,
                    _ => true,
                })
                .flat_map(|(_, set)| set.iter().cloned())
                .collect();
            earlier.insert(m.as_str(), set);
        }
        // [unknown left out, unknown as old] × [all, names, names not used, all not used, not ending not used]
        let mut f = [[0usize; 5]; 2];
        for ((meeting, heard, right), firing) in pieces.iter().zip(&firing_of) {
            let when = dates.get(meeting.as_str());
            let before_set = earlier.get(meeting.as_str());
            let used_before = |r: &(String, String)| !r.0.contains(' ') && before_set.is_some_and(|set| set.contains(&norm(&r.0)));
            for (k, unknown_old) in [false, true].into_iter().enumerate() {
                let in_use: Vec<usize> = firing
                    .iter()
                    .copied()
                    .filter(|&i| match info[i] {
                        Some((at, from)) => from.as_deref() != Some(meeting.as_str()) && when.map_or(true, |w| at < w),
                        None => unknown_old,
                    })
                    .collect();
                let with = |keep: &dyn Fn(&(String, String)) -> bool| -> usize {
                    let chosen: Vec<(String, String)> = in_use.iter().map(|&i| rules[i].clone()).filter(|r| keep(r)).collect();
                    mistakes(&apply_corrections_with(heard, &chosen), right)
                };
                f[k][0] += with(&|_| true);
                f[k][1] += with(&|r| name(r));
                f[k][2] += with(&|r| name(r) && !used_before(r));
                f[k][3] += with(&|r| !used_before(r));
                f[k][4] += with(&|r| !ending(r) && !used_before(r));
            }
        }
        let source = if estimated { "estimated: the first meeting where the rule fixes a word" } else { "learned_log" };
        let mut out = vec![format!("  learned before the meeting: {dated} of {} rules have a date ({source}), meeting dates {}", rules.len(), if dates.is_empty() { "unknown (PROTOKOL_EVAL_DB)" } else { "known" })];
        for (k, label) in ["undated rules left out", "undated rules as old"].iter().enumerate() {
            out.push(format!(
                "  {label}: WER without rules {:.2}% · all {:.2}% · names only {:.2}% · names, not «used» {:.2}% · all, not «used» {:.2}% · without ending-only, not «used» {:.2}%",
                pct(base),
                pct(f[k][0]),
                pct(f[k][1]),
                pct(f[k][2]),
                pct(f[k][3]),
                pct(f[k][4])
            ));
        }
        for l in &out {
            println!("{l}");
            log_timing(l);
        }
    }
    // Underlines: the app's own («now») and with the words a rule only
    // offers to change (the new hints), on the text recognised without rules
    let hint_words: std::collections::HashSet<String> = rules
        .iter()
        .filter(|r| !name(r) && !r.0.contains(char::is_whitespace))
        .map(|r| r.0.to_lowercase())
        .collect();
    let (mut now, mut hinted) = (crate::quality::score::Tally::default(), crate::quality::score::Tally::default());
    for r in &records {
        let ws = r["whisper"].as_array().cloned().unwrap_or_default();
        let words: Vec<String> = ws.iter().map(|w| w["w"].as_str().unwrap_or("").to_string()).collect();
        let doubt: Vec<bool> = ws.iter().map(|w| w["doubt"].as_bool().unwrap_or(false)).collect();
        let hint: Vec<bool> = ws
            .iter()
            .zip(&doubt)
            .map(|(w, d)| *d || crate::ukrainian::doubt::hinted(w["w"].as_str().unwrap_or(""), w["p"].as_f64().unwrap_or(1.0) as f32, &hint_words))
            .collect();
        let (seconds, right) = (r["seconds"].as_f64().unwrap_or(0.0), r["right"].as_str().unwrap_or(""));
        now.add(seconds, right, &words, &doubt);
        hinted.add(seconds, right, &words, &hint);
    }
    for (label, t) in [("underlines now", &now), ("underlines + hints", &hinted)] {
        let l = format!("  {label}: {}", t.summary());
        println!("{l}");
        log_timing(&l);
    }
    let detail: Vec<serde_json::Value> = rules
        .iter()
        .zip(&per)
        .filter(|(_, p)| p.0 > 0)
        .map(|(r, p)| serde_json::json!({ "wrong": r.0, "right": r.1, "fired": p.0, "removed": p.1, "added": p.2, "ending": ending(r), "name": name(r) }))
        .collect();
    std::fs::write(root.join(format!("rules-{tag}.json")), serde_json::to_string_pretty(&detail).unwrap()).unwrap();
}

/// When each word rule was learned and from which meeting: the first
/// «word» row of learned_log.jsonl (and its rotated .1) in `dir`, by
/// (wrong in lower case, right)
fn learned_rules(dir: &std::path::Path) -> std::collections::HashMap<(String, String), (chrono::DateTime<chrono::Utc>, Option<String>)> {
    let mut out: std::collections::HashMap<(String, String), (chrono::DateTime<chrono::Utc>, Option<String>)> = Default::default();
    for name in ["learned_log.jsonl.1", "learned_log.jsonl"] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else { continue };
        for v in text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
            if v["kind"].as_str() != Some("word") {
                continue;
            }
            let (Some(from), Some(to), Some(at)) = (v["from"].as_str(), v["to"].as_str(), v["at"].as_str().and_then(parse_time)) else { continue };
            let meeting = v["meeting"].as_str().map(String::from);
            let e = out.entry((from.to_lowercase(), to.to_string())).or_insert((at, meeting.clone()));
            if at < e.0 {
                *e = (at, meeting);
            }
        }
    }
    out
}

/// A time as the app stores it: RFC 3339, or SQLite's «YYYY-MM-DD HH:MM:SS» (UTC)
fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&chrono::Utc)).ok().or_else(|| {
        ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
            .iter()
            .find_map(|f| chrono::NaiveDateTime::parse_from_str(s, f).ok())
            .map(|n| n.and_utc())
    })
}

/// When each meeting was recorded (PROTOKOL_EVAL_DB), for the «rules» stage
async fn meeting_dates() -> std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> {
    let Ok(db) = std::env::var("PROTOKOL_EVAL_DB") else { return Default::default() };
    let Ok(pool) = sqlx::SqlitePool::connect(&format!("sqlite:{db}?mode=ro")).await else { return Default::default() };
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, created_at FROM meetings").fetch_all(&pool).await.unwrap_or_default();
    rows.into_iter().filter_map(|(id, at)| parse_time(&at).map(|t| (id, t))).collect()
}

/// Writes `prompt.txt`: the initial prompt Whisper gets in the app with the
/// glossary in use (PROTOKOL_EVAL_GLOSSARY_DIR), so fine-tuning can teach
/// under the same conditions (scripts/finetune/train.py --prompt-file).
/// It holds dictionary terms: it stays on this Mac.
async fn run_prompt() {
    let data_dir = app_data_dir();
    init_eval_glossary(&data_dir);
    let model = std::env::var("PROTOKOL_EVAL_WHISPER").unwrap_or_else(|_| crate::config::DEFAULT_WHISPER_MODEL.to_string());
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(whisper_models_dir(&data_dir))).expect("engine");
    engine.discover_models().await.expect("discover");
    engine.load_model(&model).await.expect("load");
    let (language, _) = crate::ukrainian::resolve_transcription_language(Some("uk"));
    let prompt = engine.initial_prompt(language.as_deref()).await.unwrap_or_default();
    std::fs::write(out_dir().join("prompt.txt"), &prompt).unwrap();
    println!("prompt.txt: {} chars", prompt.chars().count());
}

/// Both recognisers on one piece: fills `record` with Whisper's words (how
/// sure, underlined by the app's rule, replaced by a dictionary term), the
/// time it took, and Parakeet's words and time when it is loaded
async fn hear(
    engine: &crate::whisper_engine::WhisperEngine,
    parakeet: Option<&crate::parakeet_engine::ParakeetEngine>,
    aligner: Option<&mut crate::speakers::aligner::Aligner>,
    terms: &[String],
    samples: &[f32],
    record: &mut serde_json::Value,
) {
    let seconds = samples.len() as f64 / 16_000.0;
    let t = Instant::now();
    let mut piece = crate::speakers::words::transcribe_piece(engine, samples, 0.0, seconds, Some("uk".into())).await.unwrap_or(None);
    record["whisper_ms"] = serde_json::json!(t.elapsed().as_secs_f64() * 1000.0);
    let mut heard: Option<Vec<(String, f64, f32)>> = None;
    if let Some(engine) = parakeet {
        let t = Instant::now();
        heard = Some(engine.transcribe_words(samples.to_vec()).await.unwrap_or_default());
        record["parakeet_ms"] = serde_json::json!(t.elapsed().as_secs_f64() * 1000.0);
    }
    // Stage 2 (PROTOKOL_EVAL_TERMS=1): dictionary terms that sound like
    // what either model heard go first in Whisper's prompt for a second pass
    if std::env::var("PROTOKOL_EVAL_TERMS").is_ok_and(|v| v == "1") {
        let mut words: Vec<String> = piece.as_ref().map(|p| p.words.iter().map(|w| w.0.clone()).collect()).unwrap_or_default();
        words.extend(heard.iter().flatten().map(|w| w.0.clone()));
        let first: Vec<String> = piece.as_ref().map(|p| p.words.iter().map(|w| w.0.clone()).collect()).unwrap_or_default();
        let picked = crate::ukrainian::piece_terms::missing_first(crate::ukrainian::piece_terms::terms_heard(terms, &words, 60), &first);
        record["terms"] = serde_json::json!(picked.len());
        if !picked.is_empty() {
            record["whisper_first"] = serde_json::json!(first);
            let t = Instant::now();
            let again = crate::whisper_engine::PROMPT_TERMS
                .scope(picked, crate::speakers::words::transcribe_piece(engine, samples, 0.0, seconds, Some("uk".into())))
                .await
                .unwrap_or(None);
            record["terms_ms"] = serde_json::json!(t.elapsed().as_secs_f64() * 1000.0);
            if again.is_some() {
                piece = again;
            }
        }
    }
    let whisper: Vec<serde_json::Value> = match &piece {
        Some(p) => {
            let doubtful = crate::speakers::words::doubtful_in(p, terms);
            let hints = crate::ukrainian::corrections::hint_words();
            p.words
                .iter()
                .enumerate()
                .map(|(i, (w, _))| {
                    serde_json::json!({
                        "w": w,
                        "p": p.probs.get(i).copied().unwrap_or(1.0),
                        "doubt": doubtful.get(i).copied().unwrap_or(false) || crate::ukrainian::doubt::hinted(w, p.probs.get(i).copied().unwrap_or(1.0), &hints),
                        "auto": p.auto.iter().any(|(j, _)| *j == i),
                    })
                })
                .collect()
        }
        None => Vec::new(),
    };
    record["whisper"] = serde_json::json!(whisper);
    if let Some(words) = heard {
        // Where the two differ, which version fits the sound better
        if let Some(aligner) = aligner {
            let t = Instant::now();
            let first: Vec<String> = whisper.iter().map(|w| w["w"].as_str().unwrap_or("").to_string()).collect();
            let mut disputes = crate::audio::transcription::ensemble::disputes(&first, &words);
            if let Err(e) = crate::audio::transcription::ensemble::judge(aligner, samples, &first, &mut disputes) {
                log::warn!("judging failed: {e}");
            }
            record["judge_ms"] = serde_json::json!(t.elapsed().as_secs_f64() * 1000.0);
            record["disputes"] = serde_json::json!(disputes);
        }
        record["parakeet"] = serde_json::json!(words);
    }
}

/// A recording with a text checked by hand (PROTOKOL_EVAL_AUDIO,
/// PROTOKOL_EVAL_REF; PROTOKOL_EVAL_FROM/PROTOKOL_EVAL_TO in seconds when
/// the text covers only part of it), heard as on import (speech pieces of up
/// to 25 s) by Whisper and, with PROTOKOL_EVAL_PARAKEET=1, Parakeet. The
/// whole span is measured as one piece. Writes `replay-<tag>.jsonl` (tag
/// default «reference») and measures it like a replay.
async fn run_reference() {
    let root = out_dir();
    let audio = PathBuf::from(std::env::var("PROTOKOL_EVAL_AUDIO").expect("PROTOKOL_EVAL_AUDIO"));
    let right = std::fs::read_to_string(std::env::var("PROTOKOL_EVAL_REF").expect("PROTOKOL_EVAL_REF")).expect("reference text");
    let with_parakeet = std::env::var("PROTOKOL_EVAL_PARAKEET").is_ok_and(|v| v == "1");
    let tag = std::env::var("PROTOKOL_EVAL_TAG").unwrap_or_else(|_| "reference".to_string());
    let model = std::env::var("PROTOKOL_EVAL_WHISPER").unwrap_or_else(|_| crate::config::DEFAULT_WHISPER_MODEL.to_string());
    let env_s = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f64>().ok());
    let data_dir = app_data_dir();
    init_eval_glossary(&data_dir);
    let terms = crate::ukrainian::glossary_terms();
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(whisper_models_dir(&data_dir))).expect("engine");
    engine.discover_models().await.expect("discover");
    engine.load_model(&model).await.expect("load");
    let parakeet = if with_parakeet { Some(load_parakeet(&data_dir).await) } else { None };
    let mut judge = if with_parakeet { load_judge(&data_dir) } else { None };
    let samples = crate::speakers::commands::decode_16k(&audio).expect("decode");
    let a = ((env_s("PROTOKOL_EVAL_FROM").unwrap_or(0.0) * 16_000.0) as usize).min(samples.len());
    let b = env_s("PROTOKOL_EVAL_TO").map(|t| ((t * 16_000.0) as usize).min(samples.len())).unwrap_or(samples.len()).max(a);
    let span = &samples[a..b];
    let pieces = crate::speakers::words::speech_pieces(span).expect("speech pieces");
    log_timing(&format!(
        "reference «{tag}»: whisper model {model}; glossary terms: {}; {:.1} min, {} speech pieces, {} words in the text",
        terms.len(),
        span.len() as f64 / 16_000.0 / 60.0,
        pieces.len(),
        right.split_whitespace().count()
    ));
    let started = Instant::now();
    // All pieces joined into one record: the text has no times to split it by
    let mut whole = serde_json::json!({ "meeting": "reference", "file": "", "seconds": span.len() as f64 / 16_000.0, "right": right, "whisper": [], "whisper_ms": 0.0 });
    if with_parakeet {
        whole["parakeet"] = serde_json::json!([]);
        whole["parakeet_ms"] = serde_json::json!(0.0);
        if judge.is_some() {
            whole["disputes"] = serde_json::json!([]);
            whole["judge_ms"] = serde_json::json!(0.0);
        }
    }
    for piece in &pieces {
        let mut r = serde_json::json!({});
        hear(&engine, parakeet.as_ref(), judge.as_mut(), &terms, &piece.samples, &mut r).await;
        // This piece's places, counted among all the words so far
        let offset = whole["whisper"].as_array().map(|a| a.len()).unwrap_or(0);
        if let Some(list) = r["disputes"].as_array_mut() {
            for d in list.iter_mut() {
                d["at"] = serde_json::json!(d["at"].as_u64().unwrap_or(0) as usize + offset);
                if let Some(first) = d["first"].as_array_mut() {
                    for i in first.iter_mut() {
                        *i = serde_json::json!(i.as_u64().unwrap_or(0) as usize + offset);
                    }
                }
            }
        }
        for key in ["whisper", "parakeet", "disputes"] {
            if let (Some(all), Some(more)) = (whole[key].as_array_mut(), r[key].as_array()) {
                all.extend(more.iter().cloned());
            }
        }
        for key in ["whisper_ms", "parakeet_ms", "judge_ms"] {
            if let (Some(sum), Some(more)) = (whole[key].as_f64(), r[key].as_f64()) {
                whole[key] = serde_json::json!(sum + more);
            }
        }
    }
    std::fs::write(root.join(format!("replay-{tag}.jsonl")), format!("{whole}\n")).unwrap();
    log_timing(&format!("reference «{tag}» heard in {:.0} s", started.elapsed().as_secs_f64()));
    rescore(&root, &tag);
}

/// The word aligner's character model, the judge where the two recognisers
/// differ (PROTOKOL_EVAL_JUDGE=0 leaves it out); None when it is not found
fn load_judge(data_dir: &std::path::Path) -> Option<crate::speakers::aligner::Aligner> {
    if std::env::var("PROTOKOL_EVAL_JUDGE").is_ok_and(|v| v == "0") {
        return None;
    }
    // PROTOKOL_EVAL_ALIGNER_DIR, the app data folder, or the model bundled
    // with the installed app (read only)
    let bundled = PathBuf::from("/Applications/Протокол.app/Contents/Resources/aligner-model");
    let dir = std::env::var("PROTOKOL_EVAL_ALIGNER_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| crate::speakers::aligner::known_dir(&data_dir.join("models")))
        .or_else(|| bundled.join(crate::speakers::aligner::MODEL_FILE).exists().then_some(bundled));
    let aligner = dir.and_then(|d| crate::speakers::aligner::Aligner::open_with(&d, 4).ok());
    log_timing(if aligner.is_some() { "judge: word aligner model loaded" } else { "judge: word aligner model not found, places are not judged" });
    aligner
}

/// Parakeet v3 from the app's models folder, downloaded there first if it
/// is not yet (about 670 MB)
async fn load_parakeet(data_dir: &std::path::Path) -> crate::parakeet_engine::ParakeetEngine {
    const MODEL: &str = "parakeet-tdt-0.6b-v3-int8";
    let engine = crate::parakeet_engine::ParakeetEngine::new_with_models_dir(Some(data_dir.join("models"))).expect("parakeet engine");
    let models = engine.discover_models().await.expect("discover parakeet");
    let ready = models.iter().any(|m| m.name == MODEL && matches!(m.status, crate::parakeet_engine::ModelStatus::Available));
    if !ready {
        log_timing(&format!("downloading {MODEL}"));
        engine.download_model(MODEL, None).await.expect("download parakeet");
        engine.discover_models().await.expect("discover parakeet");
    }
    engine.load_model(MODEL).await.expect("load parakeet");
    log_timing(&format!("parakeet model {MODEL} loaded"));
    engine
}

/// One way to choose the words to underline, from what both recognisers heard
struct Rule {
    name: &'static str,
    /// Whisper's own underlines are kept
    whisper: bool,
    /// Words the two heard differently are underlined when Parakeet was at
    /// least this sure of its own word there (0: always; above 1: never)
    differ_sure: f32,
    /// Words Parakeet heard the same and was at least this sure of lose
    /// Whisper's underline (above 1: never)
    agreed_clears: f32,
    /// Places where the two differ are underlined when Parakeet's version
    /// fits the sound better than Whisper's by more than this (nats; may be
    /// below 0; NEVER: the judge is not used)
    judged: f32,
}

const NEVER: f32 = 2.0;
const OFF: f32 = f32::INFINITY;
const RULES: [Rule; 14] = [
    Rule { name: "now (Whisper's underlines)", whisper: true, differ_sure: NEVER, agreed_clears: NEVER, judged: OFF },
    Rule { name: "where the two differ", whisper: false, differ_sure: 0.0, agreed_clears: NEVER, judged: OFF },
    Rule { name: "differ, Parakeet ≥0.5 sure", whisper: false, differ_sure: 0.5, agreed_clears: NEVER, judged: OFF },
    Rule { name: "differ, Parakeet ≥0.8 sure", whisper: false, differ_sure: 0.8, agreed_clears: NEVER, judged: OFF },
    Rule { name: "either", whisper: true, differ_sure: 0.0, agreed_clears: NEVER, judged: OFF },
    Rule { name: "either, Parakeet ≥0.5 sure", whisper: true, differ_sure: 0.5, agreed_clears: NEVER, judged: OFF },
    Rule { name: "either, agreed ≥0.7 cleared", whisper: true, differ_sure: 0.0, agreed_clears: 0.7, judged: OFF },
    Rule { name: "judge: sound favours Parakeet (>0)", whisper: false, differ_sure: NEVER, agreed_clears: NEVER, judged: 0.0 },
    Rule { name: "judge: favours Parakeet by >2", whisper: false, differ_sure: NEVER, agreed_clears: NEVER, judged: 2.0 },
    Rule { name: "judge: not clearly Whisper's (>-2)", whisper: false, differ_sure: NEVER, agreed_clears: NEVER, judged: -2.0 },
    Rule { name: "judge: not clearly Whisper's (>-5)", whisper: false, differ_sure: NEVER, agreed_clears: NEVER, judged: -5.0 },
    Rule { name: "now + judge >0", whisper: true, differ_sure: NEVER, agreed_clears: NEVER, judged: 0.0 },
    Rule { name: "now + judge >-2", whisper: true, differ_sure: NEVER, agreed_clears: NEVER, judged: -2.0 },
    Rule { name: "now + judge >-5", whisper: true, differ_sure: NEVER, agreed_clears: NEVER, judged: -5.0 },
];

/// Measures `replay-<tag>.jsonl` against the secretary's text (see
/// `quality::score`) under each rule in [`RULES`], per meeting and in total.
/// Writes `metrics-<tag>.json`; the summary goes to timings.txt. Runs at the
/// end of a replay, or alone as stage «rescore» (no models needed).
fn rescore(root: &std::path::Path, tag: &str) {
    use crate::audio::transcription::ensemble::{compare, Dispute};
    use crate::quality::score::{queued, Tally};
    let text = std::fs::read_to_string(root.join(format!("replay-{tag}.jsonl"))).expect("replay file");
    let records: Vec<serde_json::Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let both = records.iter().any(|r| r.get("parakeet").is_some());
    let judged = records.iter().any(|r| r.get("disputes").is_some());
    let rules: Vec<&Rule> = if both { RULES.iter().filter(|r| judged || r.judged == OFF).collect() } else { RULES.iter().take(1).collect() };
    // meeting → tally per rule (and Parakeet alone, last)
    let mut per: std::collections::BTreeMap<String, Vec<Tally>> = Default::default();
    let (mut whisper_ms, mut parakeet_ms, mut judge_ms, mut agreed, mut compared) = (0.0, 0.0, 0.0, 0usize, 0usize);
    for r in &records {
        let meeting = r["meeting"].as_str().unwrap_or("?").to_string();
        let seconds = r["seconds"].as_f64().unwrap_or(0.0);
        let right = r["right"].as_str().unwrap_or("");
        let ws = r["whisper"].as_array().cloned().unwrap_or_default();
        let words: Vec<String> = ws.iter().map(|w| w["w"].as_str().unwrap_or("").to_string()).collect();
        let probs: Vec<f32> = ws.iter().map(|w| w["p"].as_f64().unwrap_or(1.0) as f32).collect();
        let own: Vec<bool> = ws.iter().enumerate().map(|(i, w)| (w["doubt"].as_bool().unwrap_or(false) || w["auto"].as_bool().unwrap_or(false)) && queued(&words[i], probs[i])).collect();
        whisper_ms += r["whisper_ms"].as_f64().unwrap_or(0.0);
        let tallies = per.entry(meeting).or_insert_with(|| vec![Tally::default(); rules.len() + 1]);
        let heard: Vec<(String, f64, f32)> = r.get("parakeet").and_then(|p| serde_json::from_value(p.clone()).ok()).unwrap_or_default();
        let c = compare(&words, &heard);
        if both {
            parakeet_ms += r["parakeet_ms"].as_f64().unwrap_or(0.0);
            agreed += c.same.iter().filter(|s| **s).count();
            compared += c.same.len();
        }
        // A word only Parakeet heard points to the Whisper word before it
        // (how sure Parakeet was of it, the surest such word if several)
        let mut gap = vec![-1.0f32; words.len()];
        for (at, w, p) in &c.only_other {
            if queued(w, 1.0) && !words.is_empty() {
                let k = at.saturating_sub(1).min(words.len() - 1);
                gap[k] = gap[k].max(*p);
            }
        }
        // Places the judge scored: how much better Parakeet's version fits,
        // for each Whisper word there (the word before, when only Parakeet
        // heard something); unjudged places count as fitting either way
        judge_ms += r["judge_ms"].as_f64().unwrap_or(0.0);
        let disputes: Vec<Dispute> = r.get("disputes").and_then(|d| serde_json::from_value(d.clone()).ok()).unwrap_or_default();
        let mut gain = vec![f32::NEG_INFINITY; words.len()];
        for d in &disputes {
            let g = d.gain.unwrap_or(f32::INFINITY);
            let content = d.first.iter().any(|&i| queued(&words[i], probs[i])) || (d.first.is_empty() && d.second.iter().any(|w| queued(w, 1.0)));
            if !content || words.is_empty() {
                continue;
            }
            let at: Vec<usize> = if d.first.is_empty() { vec![d.at.saturating_sub(1).min(words.len() - 1)] } else { d.first.clone() };
            for i in at {
                gain[i] = gain[i].max(g);
            }
        }
        for (k, rule) in rules.iter().enumerate() {
            let flags: Vec<bool> = (0..words.len())
                .map(|i| {
                    if rule.judged != OFF {
                        return gain[i] > rule.judged || (rule.whisper && own[i]);
                    }
                    // A word Parakeet has nothing for counts whatever the bar
                    let sure = c.other[i].is_none() || c.other_prob[i] >= rule.differ_sure;
                    let differs = rule.differ_sure <= 1.0 && ((!c.same[i] && queued(&words[i], probs[i]) && sure) || gap[i] >= rule.differ_sure);
                    let kept = rule.whisper && own[i] && !(c.same[i] && c.other_prob[i] >= rule.agreed_clears);
                    differs || kept
                })
                .collect();
            tallies[k].add(seconds, right, &words, &flags);
        }
        if both {
            let pw: Vec<String> = heard.iter().map(|w| w.0.clone()).collect();
            let pf: Vec<bool> = heard.iter().map(|w| w.2 < 0.85 && queued(&w.0, w.2)).collect();
            tallies[rules.len()].add(seconds, right, &pw, &pf);
        }
    }
    let mut total = vec![Tally::default(); rules.len() + 1];
    for (meeting, tallies) in &per {
        log_timing(&format!("{meeting}: {}", tallies[0].summary()));
        for (k, t) in tallies.iter().enumerate() {
            total[k].merge(t);
        }
    }
    let mut json = serde_json::Map::new();
    log_timing(&format!("«{tag}» in total, {} pieces:", records.len()));
    for (k, rule) in rules.iter().enumerate() {
        log_timing(&format!("  {}: {}", rule.name, total[k].summary()));
        json.insert(rule.name.to_string(), serde_json::to_value(&total[k]).unwrap());
    }
    if both {
        log_timing(&format!("  Parakeet alone (underlined below 0.85): {}", total[rules.len()].summary()));
        json.insert("parakeet alone".into(), serde_json::to_value(&total[rules.len()]).unwrap());
        log_timing(&format!(
            "  the two heard the same {:.1}% of words; recognition time: Whisper {:.0} s, Parakeet {:.0} s (+{:.0}%)",
            100.0 * agreed as f64 / compared.max(1) as f64,
            whisper_ms / 1000.0,
            parakeet_ms / 1000.0,
            100.0 * parakeet_ms / whisper_ms.max(1.0)
        ));
        json.insert("agreement".into(), serde_json::json!(agreed as f64 / compared.max(1) as f64));
        json.insert("whisper_s".into(), serde_json::json!(whisper_ms / 1000.0));
        json.insert("parakeet_s".into(), serde_json::json!(parakeet_ms / 1000.0));
        let terms_ms: f64 = records.iter().map(|r| r["terms_ms"].as_f64().unwrap_or(0.0)).sum();
        if terms_ms > 0.0 {
            let picked: Vec<f64> = records.iter().filter_map(|r| r["terms"].as_f64()).collect();
            log_timing(&format!(
                "  second pass with the piece's terms: {} of {} pieces, {:.1} terms on average, {:.0} s (+{:.0}% of Whisper's first pass)",
                picked.iter().filter(|n| **n > 0.0).count(),
                records.len(),
                picked.iter().sum::<f64>() / picked.len().max(1) as f64,
                terms_ms / 1000.0,
                100.0 * terms_ms / whisper_ms.max(1.0)
            ));
        }
        if judged {
            log_timing(&format!("  judge time {:.0} s (+{:.0}% of Whisper's)", judge_ms / 1000.0, 100.0 * judge_ms / whisper_ms.max(1.0)));
            json.insert("judge_s".into(), serde_json::json!(judge_ms / 1000.0));
        }
    }
    std::fs::write(root.join(format!("metrics-{tag}.json")), serde_json::to_string_pretty(&json).unwrap()).unwrap();
}

#[tokio::test]
#[ignore = "needs a real recording and downloaded models"]
async fn eval_real_recording() {
    let mut logger = env_logger::builder();
    logger.is_test(true).filter_level(log::LevelFilter::Info);
    // PROTOKOL_EVAL_DEBUG=1: raw model answers of the document generator
    if std::env::var("PROTOKOL_EVAL_DEBUG").is_ok() {
        logger.filter_module("app_lib::summary::processor", log::LevelFilter::Debug);
    }
    let _ = logger.try_init();
    // The summary code logs with `tracing`: its steps and times to stderr
    let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).with_writer(std::io::stderr).with_ansi(false).try_init();
    let stage = std::env::var("PROTOKOL_EVAL_STAGE").unwrap_or_else(|_| "all".to_string());
    if stage == "asr" || stage == "all" {
        run_asr().await;
    }
    if stage == "suggest" {
        let segments: Vec<Segment> = serde_json::from_str(&std::fs::read_to_string(out_dir().join(SEGMENTS_FILE)).unwrap()).unwrap();
        let text = segments.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join("\n");
        let list = crate::ukrainian::corrections::suggestions_from(&[text], &[], &[], &[]);
        let lines: Vec<String> = list.iter().map(|g| format!("{:?} {:?}", g.variants, g.counts)).collect();
        std::fs::write(out_dir().join("suggestions.txt"), lines.join("\n")).unwrap();
    }
    if stage == "protocol" || stage == "all" {
        run_protocol().await;
    }
    if stage == "speakers" {
        run_speakers().await;
    }
    if stage == "timeline" {
        run_timeline().await;
    }
    if stage == "words" {
        run_words().await;
    }
    if stage == "learn" {
        run_learn();
    }
    if stage == "export" {
        run_export().await;
    }
    if stage == "replay" {
        run_replay().await;
    }
    if stage == "reference" {
        run_reference().await;
    }
    if stage == "rules" {
        let tag = std::env::var("PROTOKOL_EVAL_TAG").unwrap_or_else(|_| "nocorr".to_string());
        run_rules(&out_dir(), &tag, &meeting_dates().await);
    }
    if stage == "prompt" {
        run_prompt().await;
    }
    if stage == "rescore" {
        let tag = std::env::var("PROTOKOL_EVAL_TAG").unwrap_or_else(|_| "both".to_string());
        rescore(&out_dir(), &tag);
    }
}
