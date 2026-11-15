//! What a run learns from and what it is checked on.
//!
//! Only checked lines count (read, fixed or marked «перевірено», not left
//! out of the document), cut into pieces of the recording by
//! [`crate::quality::pairs`]. The newest checked meetings are held out:
//! the run never sees them, and the new model is judged on them. Pieces the
//! user rewrote rather than corrected (more than half the words differ from
//! what was recognised) are left out of training, as in the pilot.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Held-out speech wanted (minutes) and the most meetings held out for it
pub const HELD_MINUTES: f64 = 30.0;
pub const MAX_HELD_MEETINGS: usize = 2;
/// Speech to train on (hours): less is not worth a run; more does not fit a night
pub const MIN_TRAIN_HOURS: f64 = 1.5;
pub const MAX_TRAIN_HOURS: f64 = 10.0;
/// A piece whose checked text differs from the recognised one in more than
/// this share of words was rewritten, not corrected
pub const MAX_REWRITE: f64 = 0.5;

/// A meeting with checked speech
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    /// RFC 3339; orders the meetings
    pub created_at: String,
    /// Checked speech (seconds)
    pub checked_s: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub train: Vec<String>,
    pub held: Vec<String>,
    pub train_hours: f64,
    pub held_hours: f64,
}

/// The newest meetings held out, the rest (newest first, up to
/// [`MAX_TRAIN_HOURS`]) trained on; `None` when there is too little
pub fn split(meetings: &[Meeting]) -> Option<Split> {
    let mut m: Vec<&Meeting> = meetings.iter().filter(|m| m.checked_s > 0.0).collect();
    m.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut held = Vec::new();
    let mut held_s = 0.0;
    let mut k = 0;
    while k < m.len() && held.len() < MAX_HELD_MEETINGS && held_s < HELD_MINUTES * 60.0 {
        held.push(m[k].id.clone());
        held_s += m[k].checked_s;
        k += 1;
    }
    let mut train = Vec::new();
    let mut train_s = 0.0;
    for x in &m[k..] {
        if train_s >= MAX_TRAIN_HOURS * 3600.0 {
            break;
        }
        train.push(x.id.clone());
        train_s += x.checked_s;
    }
    (train_s >= MIN_TRAIN_HOURS * 3600.0).then_some(Split { train, held, train_hours: train_s / 3600.0, held_hours: held_s / 3600.0 })
}

/// Share of the checked words that recognition got wrong (wrong, extra or missing)
pub fn changed_share(checked: &str, recognised: &str) -> f64 {
    use crate::quality::score::{align, norm};
    let r: Vec<String> = checked.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
    let h: Vec<String> = recognised.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let a = align(&r, &h);
    (a.substituted + a.deleted + a.inserted) as f64 / r.len() as f64
}

/// Whether a piece is fit to learn from
pub fn keep_for_training(checked: &str, recognised: &str) -> bool {
    changed_share(checked, recognised) <= MAX_REWRITE
}

/// One piece written for the trainer or the check
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PieceFile {
    pub file: PathBuf,
    pub text: String,
    pub seconds: f64,
}

/// Cuts a meeting's checked pieces out of its recording into `dir`
/// (`NNNNN.wav`), as the measurement does
pub fn write_pieces(samples: &[f32], pieces: &[crate::quality::pairs::Piece], dir: &Path) -> Result<Vec<PieceFile>> {
    std::fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    for (k, p) in pieces.iter().enumerate() {
        let a = ((p.start * 16_000.0) as usize).min(samples.len());
        let b = ((p.end * 16_000.0) as usize).min(samples.len());
        // Under 0.1 s: nothing to hear
        if b <= a + 1600 {
            continue;
        }
        let file = dir.join(format!("{k:05}.wav"));
        crate::quality::wav::write(&file, &samples[a..b])?;
        out.push(PieceFile { file, text: p.text.clone(), seconds: (b - a) as f64 / 16_000.0 });
    }
    Ok(out)
}

/// Pieces as the trainer reads them: one `{"file", "text"}` per line
pub fn write_jsonl(path: &Path, pieces: &[PieceFile]) -> Result<()> {
    let mut s = String::new();
    for p in pieces {
        s.push_str(&serde_json::json!({ "file": p.file, "text": p.text }).to_string());
        s.push('\n');
    }
    std::fs::write(path, s)?;
    Ok(())
}

/// Lines of a meeting the user rewrote rather than corrected: the text now
/// differs from what was recognised (`original_transcript`, kept from the
/// first edit) in more than [`MAX_REWRITE`] of the words. Pieces touching
/// them are not trained on.
pub async fn rewritten_lines(pool: &sqlx::SqlitePool, meeting_id: &str) -> Result<std::collections::HashSet<String>> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, transcript, original_transcript FROM transcripts WHERE meeting_id = ? AND original_transcript IS NOT NULL",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().filter(|(_, now, recognised)| !keep_for_training(now, recognised)).map(|(id, ..)| id).collect())
}

/// A meeting's recording, 16 kHz mono
pub async fn recording(pool: &sqlx::SqlitePool, meeting_id: &str) -> Result<Vec<f32>> {
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(pool).await?.flatten();
    let folder = folder.ok_or_else(|| anyhow::anyhow!("the meeting has no recording folder"))?;
    let audio = crate::audio::retranscription::find_audio_file(Path::new(&folder))?;
    // Decoding an hour of audio takes a while: off the async threads
    tokio::task::spawn_blocking(move || crate::speakers::commands::decode_16k(&audio)).await?
}

/// Every meeting with checked lines and how much checked speech it has
pub async fn meetings(pool: &sqlx::SqlitePool) -> Result<Vec<Meeting>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT m.id, m.created_at FROM meetings m JOIN transcripts t ON t.meeting_id = m.id WHERE t.reviewed_at IS NOT NULL ORDER BY m.created_at",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for (id, created_at) in rows {
        let pieces = crate::quality::pairs::checked_pieces(pool, &id).await?;
        let checked_s: f64 = pieces.iter().map(|p| p.end - p.start).sum();
        if checked_s > 0.0 {
            out.push(Meeting { id, created_at, checked_s });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, day: u32, minutes: f64) -> Meeting {
        Meeting { id: id.into(), created_at: format!("2026-11-{day:02}T09:00:00Z"), checked_s: minutes * 60.0 }
    }

    #[test]
    fn the_newest_meetings_are_held_out() {
        // the pilot: 13 meetings, 3.75 h; the last ones of 14.11 held out
        let ms: Vec<Meeting> = (0..13).map(|k| m(&format!("m{k}"), 2 + k, 17.3)).collect();
        let s = split(&ms).unwrap();
        assert_eq!(s.held, vec!["m12", "m11"]);
        assert_eq!(s.train.len(), 11);
        assert!(!s.train.iter().any(|t| s.held.contains(t)));
        assert!((s.held_hours - 2.0 * 17.3 / 60.0).abs() < 1e-9);
    }

    #[test]
    fn one_long_meeting_is_enough_to_hold_out() {
        let ms = vec![m("old", 1, 60.0), m("mid", 2, 60.0), m("new", 3, 45.0)];
        let s = split(&ms).unwrap();
        assert_eq!(s.held, vec!["new"]);
        assert_eq!(s.train, vec!["mid", "old"]);
    }

    #[test]
    fn too_little_speech_means_no_run_and_too_much_is_capped() {
        assert_eq!(split(&[m("a", 1, 40.0), m("b", 2, 40.0), m("c", 3, 40.0)]), None);
        let many: Vec<Meeting> = (0..28).map(|k| m(&format!("m{k:02}"), 1 + k, 60.0)).collect();
        let s = split(&many).unwrap();
        assert!(s.train_hours >= MAX_TRAIN_HOURS && s.train_hours <= MAX_TRAIN_HOURS + 1.0, "{}", s.train_hours);
        assert_eq!(s.train[0], "m26", "the newest meetings are trained on first");
    }

    #[test]
    fn a_rewritten_piece_is_left_out() {
        assert!(keep_for_training("Доповідає керівник першого відділу", "доповідає керівник першого відділу"));
        assert!(keep_for_training("Доповідає керівник першого відділу", "доповідав керівник першого відділі"));
        assert!(!keep_for_training("Коротко: усе за планом", "ну значить там у нас все як і планували"));
        assert_eq!(changed_share("", ""), 0.0);
    }
}
