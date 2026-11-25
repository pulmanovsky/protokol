//! Meet's captions against the secretary's checked text: would the places
//! where the captions heard something else have pointed to his corrections?
//!
//! Only meetings recorded in Protokol's Meet window with captions on have
//! them (`meet::captions`). For each checked line, the words as recognised
//! are compared with the captions said near them; a word is «flagged» where
//! the captions have another word there, and «corrected» where the secretary
//! changed or took it out. Counted several ways at once, to choose the one
//! worth underlining: any difference, only another word (not another
//! ending), only two such words in a row; and as counted until 0.4.7.
//! Numbers only leave this module: no texts, nothing per meeting.

use crate::meet::captions::{self, CapWord, Said};
use crate::quality::score::{align, norm};
use crate::speakers::words::PieceWords;

/// Flagged and corrected words, counted one way
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// Recognised words with captions near
    pub covered: usize,
    /// Of them flagged
    pub flagged: usize,
    /// Of them corrected by the secretary (changed or taken out)
    pub corrected: usize,
    /// Corrected and flagged
    pub corrected_flagged: usize,
}

impl Flags {
    pub fn add(&mut self, o: &Flags) {
        self.covered += o.covered;
        self.flagged += o.flagged;
        self.corrected += o.corrected;
        self.corrected_flagged += o.corrected_flagged;
    }

    fn count(&mut self, flagged: bool, corrected: bool) {
        self.covered += 1;
        self.flagged += flagged as usize;
        self.corrected += corrected as usize;
        self.corrected_flagged += (flagged && corrected) as usize;
    }
}

/// How the captions would have done as underlines
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptionsCheck {
    /// Recordings with captions
    pub meetings: usize,
    /// Recognised words of the checked lines
    pub words: usize,
    /// Their lines' length together (s), for underlines per hour
    pub seconds: usize,
    /// Any difference: another word, another ending, nothing there
    pub any: Flags,
    /// Only another word or nothing there (not «в» for «у» or another ending)
    pub other: Flags,
    /// Only where two or more words in a row are another word
    pub runs: Flags,
    /// As counted until 0.4.7: captions on the folder's clock, a fixed window
    pub before: Flags,
    /// Recordings whose captions were in Ukrainian, in another language,
    /// or whose language was not kept (recorded before 0.4.8)
    pub lang_uk: usize,
    pub lang_other: usize,
    pub lang_unknown: usize,
    /// Recordings whose captions had to be moved by more than `SHIFTED_S` to
    /// fit, and with a jump of more than `JUMP_S` inside (a pause)
    pub shifted: usize,
    pub jumped: usize,
    /// Recordings that did not fit at all (too few words in common)
    pub unfitted: usize,
    /// Recordings where the captions differ on most words (`MOSTLY`)
    pub mostly_different: usize,
}

const SHIFTED_S: f64 = 3.0;
const JUMP_S: f64 = 10.0;
const MOSTLY: f64 = 0.6;

impl CaptionsCheck {
    pub fn add(&mut self, o: &CaptionsCheck) {
        self.meetings += o.meetings;
        self.words += o.words;
        self.seconds += o.seconds;
        self.any.add(&o.any);
        self.other.add(&o.other);
        self.runs.add(&o.runs);
        self.before.add(&o.before);
        self.lang_uk += o.lang_uk;
        self.lang_other += o.lang_other;
        self.lang_unknown += o.lang_unknown;
        self.shifted += o.shifted;
        self.jumped += o.jumped;
        self.unfitted += o.unfitted;
        self.mostly_different += o.mostly_different;
    }
}

/// One recording: its checked lines (as recognised, as now, start and end in
/// seconds of the recording), the caption words on the folder's clock
/// (`captions::words`) and the captions' language as Meet showed it
pub fn check_recording(lines: &[(String, String, f64, f64)], caps: Vec<CapWord>, lang: &str) -> CaptionsCheck {
    let seconds = lines.iter().map(|l| (l.3 - l.2).max(0.0)).sum::<f64>().round() as usize;
    let mut out = CaptionsCheck { meetings: 1, seconds, ..Default::default() };
    match lang.to_lowercase() {
        l if l.is_empty() => out.lang_unknown = 1,
        l if l.contains("ukr") || l.contains("укр") => out.lang_uk = 1,
        _ => out.lang_other = 1,
    }
    // Each line's words spread evenly over its time, as Whisper's lines keep no word times
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
    let before = captions::compare_as_before(&pieces, &caps);
    let heard_times: Vec<(String, f64)> = pieces.iter().flat_map(|p| p.words.iter().map(|(w, t)| (norm(w), *t))).collect();
    let mut fitted = caps;
    let shifts = captions::fit_clock(&mut fitted, &heard_times);
    if shifts.is_empty() {
        out.unfitted = 1;
    } else {
        let mut sorted = shifts.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        if sorted[sorted.len() / 2].abs() > SHIFTED_S {
            out.shifted = 1;
        }
        if sorted[sorted.len() - 1] - sorted[0] > JUMP_S {
            out.jumped = 1;
        }
    }
    let now = captions::compare(&pieces, &fitted);
    for (((heard, right_text, _, _), said), said_before) in lines.iter().zip(&now.said).zip(&before.said) {
        // Recognised words that count, with their places in the line
        let h: Vec<(usize, String)> = heard.split_whitespace().map(norm).enumerate().filter(|(_, w)| !w.is_empty()).collect();
        let r: Vec<String> = right_text.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        let hw: Vec<String> = h.iter().map(|(_, w)| w.clone()).collect();
        let right = if hw == r { vec![true; hw.len()] } else { align(&r, &hw).correct };
        let other = |k: usize| matches!(h.get(k).and_then(|(place, _)| said.get(*place).copied().flatten()), Some(Said::Other | Said::Nothing));
        for (k, (place, _)) in h.iter().enumerate() {
            out.words += 1;
            let corrected = !right.get(k).copied().unwrap_or(true);
            if let Some(s) = said_before.get(*place).copied().flatten() {
                out.before.count(s != Said::Same, corrected);
            }
            let Some(s) = said.get(*place).copied().flatten() else { continue };
            out.any.count(s != Said::Same, corrected);
            out.other.count(other(k), corrected);
            out.runs.count(other(k) && ((k > 0 && other(k - 1)) || other(k + 1)), corrected);
        }
    }
    if out.any.covered > 0 && out.any.flagged as f64 > MOSTLY * out.any.covered as f64 {
        out.mostly_different = 1;
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
        let lang = call.captions_lang.clone();
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
        total.add(&check_recording(&lines, caps, &lang));
    }
    Ok((total.meetings > 0).then_some(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(text: &str, from: f64, to: f64) -> Vec<CapWord> {
        let c = captions::Caption { key: "a".into(), who: "Олена".into(), text: text.into(), t0: (from + 0.8) * 1000.0, t1: (to + 0.8) * 1000.0, grew: Vec::new() };
        captions::words(&[c], 0.0)
    }

    fn line(heard: &str, now: &str, start: f64, end: f64) -> (String, String, f64, f64) {
        (heard.to_string(), now.to_string(), start, end)
    }

    #[test]
    fn corrected_words_where_the_captions_heard_otherwise_are_counted() {
        // The secretary fixed «вісімнадцять»; the captions had it right. «склад» he left; the captions misheard it a little.
        let lines = vec![line("Склад заповнено на вісімнадцять відсотків", "Склад заповнено на вісімдесят відсотків", 1.0, 5.0)];
        let c = check_recording(&lines, caps("Склат заповнено на вісімдесят відсотків", 1.0, 5.0), "Ukrainian (Ukraine)");
        assert_eq!((c.meetings, c.words, c.any.covered), (1, 5, 5));
        assert_eq!((c.any.flagged, c.any.corrected, c.any.corrected_flagged), (2, 1, 1), "{c:?}");
        // «склат» is nearly «склад»: only the other word is flagged then
        assert_eq!((c.other.flagged, c.other.corrected_flagged), (1, 1), "{c:?}");
        assert_eq!(c.runs.flagged, 0, "a single word is not a run");
        assert_eq!((c.before.covered, c.before.flagged), (5, 2), "{c:?}");
        assert_eq!((c.lang_uk, c.lang_other, c.lang_unknown), (1, 0, 0));
        let mut sum = CaptionsCheck::default();
        sum.add(&c);
        sum.add(&c);
        assert_eq!((sum.meetings, sum.any.corrected_flagged, sum.lang_uk), (2, 2, 2));
    }

    #[test]
    fn a_line_never_edited_and_a_line_without_captions() {
        let lines = vec![line("Питань немає", "Питань немає", 1.0, 2.0), line("Дякую всім", "Дякую всім", 60.0, 61.0)];
        let c = check_recording(&lines, caps("Питань немає", 1.0, 2.0), "");
        assert_eq!((c.words, c.any.covered, c.any.flagged, c.any.corrected), (4, 2, 0, 0), "{c:?}");
        assert_eq!(c.lang_unknown, 1);
        assert_eq!(c.unfitted, 1, "two words in common are too few to fit the clock");
    }

    #[test]
    fn late_captions_are_put_back_before_comparing() {
        // Fifty lines of eight words, all heard right; Meet showed every
        // word 7 s late, beyond the window counted until 0.4.7
        let syl = ["ба", "ве", "гі", "до", "жу", "за", "ки", "ло", "ми", "но"];
        let word = |i: usize| format!("{}{}{}к", syl[i % 10], syl[i / 10 % 10], syl[i / 100 % 10]);
        let mut lines = Vec::new();
        let mut all = Vec::new();
        for l in 0..50 {
            let ws: Vec<String> = (0..8).map(|k| word(l * 8 + k)).collect();
            let text = ws.join(" ");
            let start = l as f64 * 4.0;
            lines.push(line(&text, &text, start, start + 2.8));
            all.extend(ws.into_iter().enumerate().map(|(k, w)| (w, start + 0.4 * k as f64)));
        }
        let late: Vec<CapWord> = all.iter().enumerate().map(|(i, (w, t))| CapWord { word: w.clone(), t: t + 7.0, who: "Олена".into(), slack: 0.0, block: i / 8 }).collect();
        let c = check_recording(&lines, late, "Ukrainian (Ukraine)");
        assert_eq!(c.words, 400);
        assert!(c.before.flagged * 4 > c.before.covered, "as before: {:?}", c.before);
        assert!(c.any.flagged * 50 < c.any.covered, "now: {:?}", c.any);
        assert_eq!((c.shifted, c.jumped, c.unfitted, c.mostly_different), (1, 0, 0, 0));
    }
}
