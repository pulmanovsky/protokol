//! Checked lines of a meeting as pieces of the recording with their right
//! text: what a measurement compares recognition with, and what a training
//! run learns from.
//!
//! A line counts when the secretary has checked it himself (read, listened
//! through, fixed or marked «перевірено»; not marked all at once) and it is
//! not left out of the document. Consecutive checked
//! lines are joined into pieces of up to 25 s; a longer line is cut between
//! its words. A line that is not checked breaks the run, so a piece never
//! holds text nobody looked at. Where two lines sound at once (a call heard
//! stream by stream keeps people who talk over each other in lines of their
//! own) the recording holds both voices, so those words are left out and a
//! piece never runs across them.

use std::collections::HashSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Longest piece (s): Whisper hears 30 s at once
pub const MAX_PIECE_S: f64 = 25.0;
/// Lines further apart than this are not joined (s)
const JOIN_GAP_S: f64 = 1.0;
/// Sound kept before the first word and after the last (s), within the
/// neighbouring lines
const PAD_BEFORE_S: f64 = 0.15;
const PAD_AFTER_S: f64 = 0.3;
/// Shorter pieces say too little to measure or learn from (s)
const MIN_PIECE_S: f64 = 0.5;
/// A word counts as said over another line when its middle is this far
/// inside that line (s): lines that only touch at their ends do not count
const OVER_S: f64 = 0.15;

/// One line of a meeting with its words and where they sound (seconds in
/// the recording)
#[derive(Debug, Clone)]
pub struct Line {
    pub id: String,
    pub checked: bool,
    pub words: Vec<(String, f64, f64)>,
}

/// A piece of the recording and the right text for it
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// The lines it comes from
    pub lines: Vec<String>,
}

/// Lines that only mark the microphone going off or on
fn is_marker(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('—') && t.ends_with('—') && t.contains("Мікрофон")
}

/// Pieces of up to [`MAX_PIECE_S`] from the checked lines, in order
pub fn pieces_of(lines: &[Line]) -> Vec<Piece> {
    // Each checked line cut into parts that fit (a part: index of its line, words)
    let mut runs: Vec<Vec<(usize, Vec<(String, f64, f64)>)>> = vec![Vec::new()];
    // Where each line sounds, to find the words another line sounds over
    let spans: Vec<Option<(f64, f64)>> = lines.iter().map(|l| Some((l.words.first()?.1, l.words.last()?.2))).collect();
    let talked_over = |k: usize, w: &(String, f64, f64)| {
        let mid = (w.1 + w.2) / 2.0;
        spans.iter().enumerate().any(|(j, s)| j != k && s.is_some_and(|(a, b)| mid > a + OVER_S && mid < b - OVER_S))
    };
    for (k, line) in lines.iter().enumerate() {
        let text: String = line.words.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ");
        if !line.checked || line.words.is_empty() || is_marker(&text) {
            if !runs.last().unwrap().is_empty() {
                runs.push(Vec::new());
            }
            continue;
        }
        let mut part: Vec<(String, f64, f64)> = Vec::new();
        for w in &line.words {
            if talked_over(k, w) {
                if !part.is_empty() {
                    runs.last_mut().unwrap().push((k, std::mem::take(&mut part)));
                }
                if !runs.last().unwrap().is_empty() {
                    runs.push(Vec::new());
                }
                continue;
            }
            if let Some(first) = part.first() {
                if w.2 - first.1 > MAX_PIECE_S {
                    runs.last_mut().unwrap().push((k, std::mem::take(&mut part)));
                }
            }
            part.push(w.clone());
        }
        if !part.is_empty() {
            runs.last_mut().unwrap().push((k, part));
        }
    }
    // Where each line's sound begins and ends, to keep padding off the neighbours
    let first_of = |k: usize| lines[k].words.first().map(|w| w.1);
    let last_of = |k: usize| lines[k].words.last().map(|w| w.2);
    let prev_end = |k: usize| (0..k).filter_map(last_of).fold(0.0, f64::max);
    let next_start = |k: usize| (k + 1..lines.len()).find_map(first_of).unwrap_or(f64::INFINITY);

    let mut out = Vec::new();
    for run in runs {
        // Parts joined while they are close and the piece stays short
        let mut joined: Vec<(Vec<usize>, Vec<(String, f64, f64)>)> = Vec::new();
        for (k, words) in run {
            if let Some((ks, acc)) = joined.last_mut() {
                let gap = words[0].1 - acc.last().unwrap().2;
                let length = words.last().unwrap().2 - acc[0].1;
                if gap <= JOIN_GAP_S && length <= MAX_PIECE_S {
                    if ks.last() != Some(&k) {
                        ks.push(k);
                    }
                    acc.extend(words);
                    continue;
                }
            }
            joined.push((vec![k], words));
        }
        for (ks, words) in joined {
            let (first, last) = (ks[0], *ks.last().unwrap());
            let (a, b) = (words[0].1, words.last().unwrap().2.max(words[0].1));
            // Within the neighbouring lines; a piece inside a long line (cut
            // between its words) keeps no padding on that side
            let before = if words[0].1 > first_of(first).unwrap_or(a) + 1e-6 { a } else { (a - PAD_BEFORE_S).max(prev_end(first)).max(0.0).min(a) };
            let after = if b < last_of(last).unwrap_or(b) - 1e-6 { b } else { (b + PAD_AFTER_S).min(next_start(last)).max(b) };
            if after - before < MIN_PIECE_S {
                continue;
            }
            out.push(Piece {
                start: before,
                end: after,
                text: words.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" "),
                lines: ks.iter().map(|&k| lines[k].id.clone()).collect(),
            });
        }
    }
    out
}

/// The checked lines of a meeting with where their words sound
pub async fn lines_of(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<Line>> {
    let words = crate::speakers::lines::line_words(pool, meeting_id).await?;
    // Only lines the secretary checked himself: a line marked all at once is
    // what recognition wrote, and would teach and judge in its favour
    let sql = format!("SELECT id FROM transcripts WHERE meeting_id = ? AND {}", crate::speakers::lines::CHECKED_BY_HAND);
    let checked: HashSet<String> = sqlx::query_scalar::<_, String>(&sql)
        .bind(meeting_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    let left_out = crate::speakers::lines::excluded_ids(pool, meeting_id).await?;
    Ok(words
        .into_iter()
        .map(|l| Line {
            checked: checked.contains(&l.id) && !left_out.contains(&l.id),
            words: l.words.into_iter().map(|w| (w.word, w.start, w.end)).collect(),
            id: l.id,
        })
        .collect())
}

/// Pieces of a meeting's checked lines
pub async fn checked_pieces(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<Piece>> {
    Ok(pieces_of(&lines_of(pool, meeting_id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, checked: bool, words: &[(&str, f64)]) -> Line {
        let w = words
            .iter()
            .enumerate()
            .map(|(i, (t, s))| (t.to_string(), *s, words.get(i + 1).map(|n| n.1).unwrap_or(s + 0.4)))
            .collect();
        Line { id: id.into(), checked, words: w }
    }

    #[test]
    fn checked_lines_close_together_make_one_piece() {
        let lines = vec![line("a", true, &[("Добрий", 1.0), ("день.", 1.4)]), line("b", true, &[("Почнемо", 2.2), ("нараду.", 2.7)])];
        let p = pieces_of(&lines);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].text, "Добрий день. Почнемо нараду.");
        assert_eq!(p[0].lines, vec!["a", "b"]);
        assert!((p[0].start - 0.85).abs() < 1e-9);
    }

    #[test]
    fn an_unchecked_line_breaks_the_run_and_bounds_the_padding() {
        let lines = vec![
            line("a", true, &[("Перше", 1.0), ("речення.", 1.5)]),
            line("b", false, &[("Не", 2.0), ("перевірено.", 2.3)]),
            line("c", true, &[("Третє", 2.75), ("речення.", 3.2)]),
        ];
        let p = pieces_of(&lines);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].text, "Перше речення.");
        assert!(p[0].end <= 2.0 + 1e-9, "{p:?}");
        assert!(p[1].start >= 2.7 - 1e-9, "{p:?}");
        assert!(!p.iter().any(|x| x.lines.contains(&"b".to_string())));
    }

    #[test]
    fn words_two_people_say_at_once_are_left_out_and_never_bridged() {
        // A long report; a remark said over its middle; the next speaker
        // starts before the report ends
        let report: Vec<(String, f64)> = (0..20).map(|i| (format!("звіт{i}"), i as f64)).collect();
        let refs: Vec<(&str, f64)> = report.iter().map(|(w, t)| (w.as_str(), *t)).collect();
        let next: Vec<(String, f64)> = (0..6).map(|i| (format!("далі{i}"), 18.0 + i as f64)).collect();
        let next_refs: Vec<(&str, f64)> = next.iter().map(|(w, t)| (w.as_str(), *t)).collect();
        let lines = vec![line("a", true, &refs), line("b", true, &[("Так,", 8.0), ("так.", 8.6)]), line("c", true, &next_refs)];
        let p = pieces_of(&lines);
        let text: Vec<&str> = p.iter().map(|x| x.text.as_str()).collect();
        // The remark is gone whole, the report without the words under it
        // and under the next speaker, the next speaker from where he is alone
        assert!(!text.iter().any(|t| t.contains("Так") || t.contains("так.")), "{text:?}");
        assert!(!text.iter().any(|t| t.contains("звіт8 ") || t.contains("звіт18") || t.contains("звіт19") || t.contains("далі0")), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with("звіт0 ") && t.ends_with("звіт7")), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with("звіт9 ") && t.ends_with("звіт17")), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with("далі1 ") && t.ends_with("далі5")), "{text:?}");
        assert_eq!(p.len(), 3, "{text:?}");
        // No piece holds sound of the stretches left out
        let over = [(8.0, 9.0), (18.0, 19.0)];
        for x in &p {
            for (a, b) in over {
                assert!(x.end <= a + 1e-6 || x.start >= b - 1e-6, "{x:?} over {a}–{b}");
            }
        }
    }

    #[test]
    fn a_long_line_is_cut_between_words() {
        let words: Vec<(String, f64)> = (0..120).map(|i| (format!("слово{i}"), i as f64 * 0.5)).collect();
        let refs: Vec<(&str, f64)> = words.iter().map(|(w, t)| (w.as_str(), *t)).collect();
        let p = pieces_of(&[line("a", true, &refs)]);
        assert!(p.len() >= 3, "{}", p.len());
        assert!(p.iter().all(|x| x.end - x.start <= MAX_PIECE_S + PAD_BEFORE_S + PAD_AFTER_S + 1e-6));
        let all: Vec<String> = p.iter().flat_map(|x| x.text.split(' ').map(str::to_string).collect::<Vec<_>>()).collect();
        assert_eq!(all.len(), 120);
    }

    #[test]
    fn microphone_markers_and_left_out_lines_are_skipped() {
        let lines = vec![line("m", true, &[("—", 0.0), ("Мікрофон", 0.1), ("вимкнено", 0.2), ("—", 0.3)]), line("x", false, &[("Пропущено", 1.0)])];
        assert!(pieces_of(&lines).is_empty());
    }
}
