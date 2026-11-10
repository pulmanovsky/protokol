//! Who spoke when: speaker diarization on this Mac.
//!
//! The models (pyannote segmentation-3.0 and a voice-embedding model) run
//! with ONNX Runtime; nothing leaves the computer.

pub(crate) mod cluster;
pub mod commands;
pub mod diarize;
pub mod download;
pub mod names;
pub mod embedding;
pub(crate) mod fbank;
pub(crate) mod onnx;
pub(crate) mod profiles;
pub mod timeline;
pub mod words;
pub mod overrides;
pub mod lines;
pub mod journal;
pub mod saved;
pub mod time_map;
pub mod aligner;

use crate::audio::vad::SpeechSegment;

pub use diarize::{Diarization, Turn};

#[cfg(test)]
mod tests {
    #[test]
    fn renamed_voices_in_documents() {
        let before = "# Нарада\n\n**Я** (00:00):\nПочнемо.\n\n**Спікер 1** (00:10):\nТекст.\n\n**Спікер 2** (01:00):\nЩе.\n\n**Спікер 1** (02:00):\nДякую.";
        let after = "# Нарада\n\n**Я** (00:00):\nПочнемо.\n\n**Олег** (00:10):\nТекст.\n\n**Спікер 2** (01:00):\nЩе.\n\n**Олег** (02:00):\nДякую.";
        assert_eq!(super::renamed_speakers(before, after), vec![("Спікер 1".to_string(), "Олег".to_string())]);
        let half = after.replacen("**Олег** (02:00)", "**Олена** (02:00)", 1);
        assert!(super::renamed_speakers(before, &half).is_empty());
        assert_eq!(super::heading_speakers("### [03:58] Спікер 3\ntext"), vec!["Спікер 3"]);
    }
}

/// Longest piece sent to Whisper at once (same as the audio import)
const MAX_PIECE_SAMPLES: usize = 25 * 16_000;
/// Audio kept on both sides of a turn so first and last words are whole
const PAD_S: f64 = 0.15;

/// One speaker's turn cut into pieces Whisper can take, each with the speaker
pub fn turn_pieces(audio: &[f32], turns: &[Turn]) -> Vec<(usize, SpeechSegment)> {
    let mut out = Vec::new();
    for t in turns {
        let start = ((t.start - PAD_S).max(0.0) * 16_000.0) as usize;
        let end = (((t.end + PAD_S) * 16_000.0) as usize).min(audio.len());
        if end <= start + 1600 {
            continue;
        }
        let seg = SpeechSegment {
            samples: audio[start..end].to_vec(),
            start_timestamp_ms: start as f64 / 16.0,
            end_timestamp_ms: end as f64 / 16.0,
            confidence: 1.0,
        };
        for piece in crate::audio::common::split_segment_at_silence(&seg, MAX_PIECE_SAMPLES) {
            out.push((t.speaker, piece));
        }
    }
    out
}

/// Speaker names in a document's report headings, in order:
/// «**Спікер 2** (03:58):» (Стенограма) and «### [03:58] Спікер 2» (Матеріали)
pub fn heading_speakers(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if let Some(rest) = l.strip_prefix("**") {
                let (name, tail) = rest.split_once("**")?;
                tail.trim_start().starts_with('(').then(|| name.trim().to_string())
            } else if let Some(rest) = l.strip_prefix("### [") {
                rest.split_once(']').map(|(_, name)| name.trim().to_string())
            } else {
                None
            }
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// Voices renamed in an edited document: a heading «Спікер N» (or a guessed
/// «Name?») that now carries another name everywhere it appears.
pub fn renamed_speakers(before: &str, after: &str) -> Vec<(String, String)> {
    let (old, new) = (heading_speakers(before), heading_speakers(after));
    if old.len() != new.len() {
        return Vec::new();
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for (a, b) in old.iter().zip(&new) {
        let renamable = a.starts_with("Спікер ") || a.ends_with('?') || a.ends_with("(?)");
        if !renamable || a == b {
            continue;
        }
        // Every heading of that voice must agree on the new name
        let consistent = old.iter().zip(&new).filter(|(x, _)| *x == a).all(|(_, y)| y == b);
        if consistent && !out.iter().any(|(x, _)| x == a) {
            out.push((a.clone(), b.clone()));
        }
    }
    out
}

/// «Спікер N» (1-based) for a diarization speaker index
pub fn speaker_label(index: usize) -> String {
    format!("Спікер {}", index + 1)
}
