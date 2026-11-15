//! What the app asks the trainer to do, and what the trainer reports back.
//!
//! The trainer is a separate program (`scripts/learning-engine`, on MLX).
//! The app writes `job.json` into a work folder and starts the trainer with
//! that path. The trainer prints
//! one JSON object per line on stdout ([`Progress`]); anything else it
//! prints is only logged. It writes the finished model, already in the
//! whisper.cpp format, to `ggml_out`.
//!
//! The trainer must stop by itself when its stdin closes: the app quit or
//! crashed, and nobody will read its output.
//!
//! The pieces files are JSON lines: `{"file": "<wav path>", "text": "<checked text>"}`,
//! 16 kHz mono WAV, at most 30 s each.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const JOB_VERSION: u32 = 1;

/// How the trainer holds the model's weights while training
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    /// The decoder in full precision: up to 11 GB with large-v3 (15.11:
    /// 15.3% mistakes on the pilot's test meetings, 36 min)
    #[default]
    Full,
    /// All of it in half precision, as the file stores it (the added weights
    /// still in full): up to 7 GB, for Macs with less than 24 GB (15.5%, 27 min)
    Half,
}

impl Precision {
    pub fn for_mac(memory_gb: f64) -> Self {
        if memory_gb >= 23.5 {
            Precision::Full
        } else {
            Precision::Half
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub version: u32,
    /// The original whisper.cpp model (`ggml-large-v3.bin`): the trainer
    /// starts from it and writes the new model as a copy of it with the
    /// trained weights replaced
    pub base_model: PathBuf,
    pub train: PathBuf,
    /// A few pieces of the training meetings to watch the loss on (not the held-out meetings)
    pub valid: PathBuf,
    /// The prompt recognition uses (dictionary words), so the model learns with it
    pub prompt: String,
    pub language: String,
    /// What and how much is trained: small added weights (LoRA) on the decoder
    pub lora_rank: u32,
    pub learning_rate: f64,
    pub epochs: u32,
    #[serde(default)]
    pub precision: Precision,
    /// Where the trainer keeps its own files, and the finished model
    pub work_dir: PathBuf,
    pub ggml_out: PathBuf,
}

impl Job {
    /// The settings of v5, the pilot model that beat the original (15.11)
    pub fn like_v5(base_model: PathBuf, work_dir: PathBuf, prompt: String) -> Self {
        Job {
            version: JOB_VERSION,
            base_model,
            train: work_dir.join("train.jsonl"),
            valid: work_dir.join("valid.jsonl"),
            prompt,
            language: "uk".into(),
            lora_rank: 32,
            learning_rate: 2e-4,
            epochs: 2,
            precision: Precision::Full,
            ggml_out: work_dir.join("ggml-model.bin"),
            work_dir,
        }
    }
}

/// One line the trainer prints
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Progress {
    Started { pieces: usize, epochs: u32 },
    Step { epoch: u32, step: usize, of: usize, loss: f64, seconds_per_piece: f64, memory_gb: f64 },
    Epoch { epoch: u32, valid_loss: f64 },
    Converting,
    Done { minutes: f64, peak_memory_gb: f64 },
    Failed { reason: String },
}

/// `None` for lines that are not progress (the trainer's own log)
pub fn parse_line(line: &str) -> Option<Progress> {
    let t = line.trim();
    if !t.starts_with('{') {
        return None;
    }
    serde_json::from_str(t).ok()
}

/// Share of the run done, 0..1, from a step line (training is most of the run)
pub fn share_done(p: &Progress, epochs: u32) -> Option<f64> {
    match p {
        Progress::Step { epoch, step, of, .. } => {
            let per = 1.0 / epochs.max(1) as f64;
            Some(((*epoch as f64 - 1.0) * per + per * (*step as f64 / (*of).max(1) as f64)).clamp(0.0, 1.0))
        }
        Progress::Converting | Progress::Done { .. } => Some(1.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_are_read_and_the_rest_is_ignored() {
        assert_eq!(
            parse_line(r#"{"event":"step","epoch":1,"step":120,"of":601,"loss":0.41,"seconds_per_piece":1.85,"memory_gb":9.2}"#),
            Some(Progress::Step { epoch: 1, step: 120, of: 601, loss: 0.41, seconds_per_piece: 1.85, memory_gb: 9.2 })
        );
        assert_eq!(parse_line(r#"{"event":"done","minutes":50.0,"peak_memory_gb":14.0}"#), Some(Progress::Done { minutes: 50.0, peak_memory_gb: 14.0 }));
        assert_eq!(parse_line("loading weights..."), None);
        assert_eq!(parse_line(r#"{"event":"something new"}"#), None);
    }

    #[test]
    fn the_job_round_trips_as_json() {
        let j = Job::like_v5("/m/base".into(), "/w".into(), "Слова: ...".into());
        let text = serde_json::to_string(&j).unwrap();
        assert_eq!(serde_json::from_str::<Job>(&text).unwrap(), j);
        assert_eq!(j.train, PathBuf::from("/w/train.jsonl"));
        assert!(text.contains(r#""precision":"full""#));
    }

    #[test]
    fn smaller_macs_train_in_half_precision() {
        assert_eq!(Precision::for_mac(24.0), Precision::Full);
        assert_eq!(Precision::for_mac(64.0), Precision::Full);
        assert_eq!(Precision::for_mac(18.0), Precision::Half);
        assert_eq!(Precision::for_mac(16.0), Precision::Half);
    }

    #[test]
    fn share_done_counts_both_epochs() {
        let step = |epoch, step| Progress::Step { epoch, step, of: 600, loss: 0.4, seconds_per_piece: 1.8, memory_gb: 9.0 };
        assert_eq!(share_done(&step(1, 300), 2), Some(0.25));
        assert_eq!(share_done(&step(2, 600), 2), Some(1.0));
        assert_eq!(share_done(&Progress::Converting, 2), Some(1.0));
    }
}
