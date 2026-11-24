//! Meet's captions against the secretary's checked text: would the places
//! where the captions heard something else have pointed to his corrections?
//!
//! Only meetings recorded in Protokol's Meet window with captions on have
//! them (`meet::captions`). For each checked line, the words as recognised
//! are compared with the captions said near them; a word is «flagged» where
//! the captions have another word there, and «corrected» where the secretary
//! changed or took it out. Numbers only leave this module: no texts.

use crate::meet::captions;
use crate::quality::score::{align, norm};
use crate::speakers::words::PieceWords;

/// How the captions would have done as underlines
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptionsCheck {
    /// Meetings with captions
    pub meetings: usize,
    /// Recognised words of the checked lines
    pub words: usize,
    /// Of them with captions near
    pub covered: usize,
    /// Of those, where the captions have another word
    pub flagged: usize,
    /// Words with captions near that the secretary corrected (changed or took out)
    pub corrected: usize,
    /// Of them flagged
    pub corrected_flagged: usize,
}

impl CaptionsCheck {
    pub fn add(&mut self, o: &CaptionsCheck) {
        self.meetings += o.meetings;
        self.words += o.words;
        self.covered += o.covered;
        self.flagged += o.flagged;
        self.corrected += o.corrected;
        self.corrected_flagged += o.corrected_flagged;
    }
}

/// One meeting: its checked lines (as recognised, as now, start and end in
/// seconds of the recording) and the caption words on the same clock
/// (`captions::words`)
pub fn check_lines(lines: &[(String, String, f64, f64)], caps: &[(String, f64, String)]) -> CaptionsCheck {
    let mut out = CaptionsCheck { meetings: 1, ..Default::default() };
    // Each line's words spread evenly over its time, as the captions' are
    let pieces: Vec<PieceWords> = lines
        .iter()
        .map(|(heard, _, start, end)| {
            let ws: Vec<&str> = heard.split_whitespace().collect();
            let n = ws.len();
            let words = ws
                .iter()
                .enumerate()
                .map(|(i, w)| (w.to_string(), if n > 1 { start + (end - start).max(0.0) * i as f64 / (n - 1) as f64 } else { *start }))
                .collect();
            PieceWords { text: heard.clone(), start: *start, end: *end, words, ..Default::default() }
        })
        .collect();
    let cmp = captions::compare(&pieces, caps);
    for ((heard, now, _, _), agree) in lines.iter().zip(&cmp.agree) {
        // Recognised words that count, with their places in the line
        let h: Vec<(usize, String)> = heard.split_whitespace().map(norm).enumerate().filter(|(_, w)| !w.is_empty()).collect();
        let r: Vec<String> = now.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        let hw: Vec<String> = h.iter().map(|(_, w)| w.clone()).collect();
        let right = if hw == r { vec![true; hw.len()] } else { align(&r, &hw).correct };
        for (k, (place, _)) in h.iter().enumerate() {
            out.words += 1;
            let Some(same) = agree.get(*place).copied().flatten() else { continue };
            out.covered += 1;
            let corrected = !right.get(k).copied().unwrap_or(true);
            if !same {
                out.flagged += 1;
            }
            if corrected {
                out.corrected += 1;
                if !same {
                    out.corrected_flagged += 1;
                }
            }
        }
    }
    out
}

/// The newest checked meetings (up to `limit`) that have captions, together
pub async fn check(pool: &sqlx::SqlitePool, limit: usize) -> anyhow::Result<Option<CaptionsCheck>> {
    let checked = crate::speakers::lines::CHECKED_BY_HAND;
    let sql = format!(
        "SELECT DISTINCT m.id, m.folder_path, m.created_at FROM meetings m JOIN transcripts t ON t.meeting_id = m.id WHERE {checked} AND m.folder_path IS NOT NULL ORDER BY m.created_at DESC"
    );
    let meetings: Vec<(String, String, String)> = sqlx::query_as(&sql).fetch_all(pool).await?;
    let mut total = CaptionsCheck::default();
    for (id, folder, _) in meetings {
        if total.meetings >= limit {
            break;
        }
        let folder = std::path::PathBuf::from(folder);
        let Some(call) = crate::meet::frames::load_folder(&folder).filter(|c| !c.captions.is_empty()) else { continue };
        let Some(start) = crate::meet::frames::recording_start_ms(&folder) else { continue };
        let caps = captions::words(&call.captions, start);
        let left_out = crate::speakers::lines::excluded_ids(pool, &id).await?;
        let rows: Vec<(String, Option<String>, Option<String>, Option<f64>, Option<f64>)> = sqlx::query_as(&format!(
            "SELECT id, transcript, original_transcript, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? AND {checked} ORDER BY audio_start_time"
        ))
        .bind(&id)
        .fetch_all(pool)
        .await?;
        let lines: Vec<(String, String, f64, f64)> = rows
            .into_iter()
            .filter(|r| !left_out.contains(&r.0))
            .filter_map(|(_, now, heard, start, end)| {
                let now = now.unwrap_or_default();
                let (start, end) = (start?, end?);
                Some((heard.unwrap_or_else(|| now.clone()), now, start, end))
            })
            .collect();
        if lines.is_empty() {
            continue;
        }
        total.add(&check_lines(&lines, &caps));
    }
    Ok((total.meetings > 0).then_some(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(text: &str, from: f64, to: f64) -> Vec<(String, f64, String)> {
        let c = captions::Caption { key: "a".into(), who: "Олена".into(), text: text.into(), t0: (from + 0.8) * 1000.0, t1: (to + 0.8) * 1000.0 };
        captions::words(&[c], 0.0)
    }

    #[test]
    fn corrected_words_where_the_captions_heard_otherwise_are_counted() {
        // The secretary fixed «вісімнадцять»; the captions had it right. «склад» he left; the captions misheard it.
        let lines = vec![(
            "Склад заповнено на вісімнадцять відсотків".to_string(),
            "Склад заповнено на вісімдесят відсотків".to_string(),
            1.0,
            5.0,
        )];
        let c = check_lines(&lines, &caps("Склат заповнено на вісімдесят відсотків", 1.0, 5.0));
        assert_eq!((c.meetings, c.words, c.covered), (1, 5, 5));
        assert_eq!((c.flagged, c.corrected, c.corrected_flagged), (2, 1, 1), "{c:?}");
        let mut sum = CaptionsCheck::default();
        sum.add(&c);
        sum.add(&c);
        assert_eq!((sum.meetings, sum.corrected_flagged), (2, 2));
    }

    #[test]
    fn a_line_never_edited_and_a_line_without_captions() {
        let lines = vec![
            ("Питань немає".to_string(), "Питань немає".to_string(), 1.0, 2.0),
            ("Дякую всім".to_string(), "Дякую всім".to_string(), 60.0, 61.0),
        ];
        let c = check_lines(&lines, &caps("Питань немає", 1.0, 2.0));
        assert_eq!((c.words, c.covered, c.flagged, c.corrected), (4, 2, 0, 0), "{c:?}");
    }
}
