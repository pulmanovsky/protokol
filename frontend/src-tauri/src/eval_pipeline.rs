//! End-to-end quality check on a real recording, using the app's own pipeline.
//!
//! Ignored by default. Run with:
//!   PROTOKOL_EVAL_AUDIO=/path/meeting.m4a PROTOKOL_EVAL_OUT=/path/out \
//!   PROTOKOL_EVAL_STAGE=asr|protocol|all \
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

    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).expect("engine");
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
        None,
        None,
        None,
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
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).expect("engine");
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
    let engine = crate::whisper_engine::WhisperEngine::new_with_models_dir(Some(data_dir.join("models"))).expect("engine");
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
}
