//! «Якість розпізнавання»: how many words of each checked meeting had to
//! be corrected, and which model heard it.
//!
//! A line keeps what recognition wrote (`original_transcript`) from its
//! first edit; a line never edited was right as heard. Only checked lines
//! count, as in training: read, fixed or marked «перевірено», not left out
//! of the document. Numbers only leave this module: no texts.
//!
//! Each meeting's corrections are also sorted by kind ([`Edits`]): what the
//! secretary fixes most is where recognition should improve next.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

/// Meetings with fewer checked words say too little to show
pub const MIN_WORDS: usize = 100;
/// The newest meetings shown
pub const SHOWN: usize = 40;
/// The newest meetings whose corrections by kind go into the report
pub const REPORTED: usize = 10;

/// One checked meeting
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Meeting {
    /// Local date, `YYYY-MM-DD`
    pub date: String,
    /// Words of the checked lines
    pub words: usize,
    /// Of them corrected: wrong, missing or extra words (per 100 words)
    pub corrected: f64,
    /// A fine-tuned model heard it
    pub tuned: bool,
    /// The corrections by kind
    pub edits: Edits,
    /// A Meet call heard stream by stream (`speakers::slots`)
    pub streams: bool,
    /// Recorded in the Meet window
    pub meet: bool,
    /// Meet's separate streams are kept with the recording, whether or not
    /// its text came from them
    pub streams_kept: bool,
}

/// Corrections of checked lines by kind: words, and whole lines
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Edits {
    /// A number heard wrong, missing or written another way
    pub numbers: usize,
    /// A word of the dictionary (names, titles, abbreviations)
    pub names: usize,
    /// The right word with another ending
    pub endings: usize,
    /// Another word in place of the one heard
    pub replaced: usize,
    /// A word recognition missed, added by hand
    pub added: usize,
    /// A filler taken out («е-е», «ну», «значить»)
    pub fillers: usize,
    /// Another word recognition wrote, taken out
    pub removed: usize,
    /// Lines changed only in punctuation or capitals
    pub punctuation_lines: usize,
    /// Lines given to another speaker by hand
    pub speaker_lines: usize,
    /// Of `names`: words of the dictionary written small in the checked text
    /// (words of longer terms, not names; nothing fixes them by itself)
    pub names_lower: usize,
    /// Of `names`: the same change made earlier in the same meeting
    pub names_repeats: usize,
    /// Corrected words of any kind whose exact change (heard → right) was
    /// made earlier in the same meeting: what fixing the rest of a meeting
    /// at once would save
    pub repeats: usize,
}

impl Edits {
    pub fn add(&mut self, o: &Edits) {
        self.numbers += o.numbers;
        self.names += o.names;
        self.endings += o.endings;
        self.replaced += o.replaced;
        self.added += o.added;
        self.fillers += o.fillers;
        self.removed += o.removed;
        self.punctuation_lines += o.punctuation_lines;
        self.speaker_lines += o.speaker_lines;
        self.names_lower += o.names_lower;
        self.names_repeats += o.names_repeats;
        self.repeats += o.repeats;
    }

    /// Corrected words (the same count as [`count`])
    pub fn words(&self) -> usize {
        self.numbers + self.names + self.endings + self.replaced + self.added + self.fillers + self.removed
    }
}

/// Words people say while thinking, which minutes leave out
const FILLERS: &[&str] = &[
    "е", "ее", "еее", "е-е", "е-е-е", "ем", "еме", "м", "мм", "ммм", "гм", "хм", "а-а", "ну", "от", "ось", "значить", "типу", "коротше", "так", "вот", "ага", "угу",
    "загалом", "власне", "скажімо", "якби", "мабуть",
];

fn is_filler(w: &str) -> bool {
    FILLERS.contains(&w)
}

/// The corrections of checked lines by kind: (text now, text as recognised
/// when the line was edited), in the order they were said; `dictionary` as
/// [`super::measure::dictionary_words`] makes it
pub fn edits(lines: &[(String, Option<String>)], dictionary: &std::collections::HashSet<String>) -> Edits {
    use super::measure::{kind_of, Kind};
    use crate::quality::score::{align, norm};
    let mut e = Edits::default();
    // Changes already made in this meeting: (heard, right), "" for a word added or taken out
    let mut made: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut seen = |heard: &str, right: &str| !made.insert((heard.to_string(), right.to_string()));
    for (now, heard) in lines {
        let Some(heard) = heard else { continue };
        if now == heard {
            continue;
        }
        // Words as written (for capitals) and as compared
        let written: Vec<(&str, String)> = now.split_whitespace().map(|w| (w, norm(w))).filter(|(_, n)| !n.is_empty()).collect();
        let r: Vec<String> = written.iter().map(|(_, n)| n.clone()).collect();
        let h: Vec<String> = heard.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        if r == h {
            e.punctuation_lines += 1;
            continue;
        }
        let small = |i: usize| written[i].0.chars().find(|c| c.is_alphabetic()).is_some_and(|c| c.is_lowercase());
        let a = align(&r, &h);
        let name = |e: &mut Edits, i: usize, repeat: bool| {
            e.names += 1;
            if small(i) {
                e.names_lower += 1;
            }
            if repeat {
                e.names_repeats += 1;
            }
        };
        for &(i, j) in &a.wrong {
            let repeat = seen(&h[j], &r[i]);
            if repeat {
                e.repeats += 1;
            }
            match kind_of(&r[i], Some(&h[j]), dictionary) {
                Kind::Number => e.numbers += 1,
                Kind::Dictionary => name(&mut e, i, repeat),
                Kind::Ending => e.endings += 1,
                Kind::Other => e.replaced += 1,
            }
        }
        for &i in &a.missed {
            let repeat = seen("", &r[i]);
            if repeat {
                e.repeats += 1;
            }
            match kind_of(&r[i], None, dictionary) {
                Kind::Number => e.numbers += 1,
                Kind::Dictionary => name(&mut e, i, repeat),
                _ => e.added += 1,
            }
        }
        let paired: std::collections::HashSet<usize> = a.wrong.iter().map(|&(_, j)| j).collect();
        for (j, w) in h.iter().enumerate() {
            if a.correct.get(j).copied().unwrap_or(false) || paired.contains(&j) {
                continue;
            }
            if seen(w, "") {
                e.repeats += 1;
            }
            if is_filler(w) {
                e.fillers += 1;
            } else if kind_of(w, None, dictionary) == Kind::Number {
                e.numbers += 1;
            } else {
                e.removed += 1;
            }
        }
    }
    e
}

/// What «Якість розпізнавання» shows
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quality {
    /// Oldest first
    pub meetings: Vec<Meeting>,
    /// Local dates (`YYYY-MM-DD`) of accepted runs, oldest first
    pub trainings: Vec<String>,
}

/// Words and corrected words of checked lines: (text now, text as recognised
/// when the line was edited)
pub fn count(lines: &[(String, Option<String>)]) -> (usize, usize) {
    use crate::quality::score::{align, norm};
    let mut words = 0;
    let mut changed = 0;
    for (now, heard) in lines {
        let r: Vec<String> = now.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
        words += r.len();
        if let Some(heard) = heard {
            let h: Vec<String> = heard.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect();
            if r != h {
                let a = align(&r, &h);
                changed += a.substituted + a.deleted + a.inserted;
            }
        }
    }
    (words, changed)
}

/// A meeting's `created_at` as the database keeps it
pub fn parse_time(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    if let Ok(t) = chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z") {
        return Some(t.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok().map(|t| t.and_utc())
}

/// The local date of a run id (`2026-11-15T01-30`)
pub fn run_date(id: &str) -> Option<String> {
    chrono::NaiveDateTime::parse_from_str(id, "%Y-%m-%dT%H-%M").ok().map(|t| t.format("%Y-%m-%d").to_string())
}

pub async fn quality(pool: &sqlx::SqlitePool, models: &Path, model_file: &str) -> Result<Quality> {
    let checked = crate::speakers::lines::CHECKED_BY_HAND;
    let sql = format!("SELECT DISTINCT m.id, m.created_at FROM meetings m JOIN transcripts t ON t.meeting_id = m.id WHERE {checked} ORDER BY m.created_at DESC");
    let rows: Vec<(String, String)> = sqlx::query_as(&sql)
    .fetch_all(pool)
    .await?;
    let dictionary = super::measure::dictionary_words(&crate::ukrainian::glossary_terms());
    let changes = super::versions::changes(models);
    let copied_at = super::versions::copied_at(models, model_file);
    let mut meetings = Vec::new();
    for (id, created_at) in rows {
        if meetings.len() >= SHOWN {
            break;
        }
        let Some(at) = parse_time(&created_at) else { continue };
        let left_out = crate::speakers::lines::excluded_ids(pool, &id).await?;
        let lines: Vec<(String, Option<String>, Option<String>, i64)> =
            sqlx::query_as(&format!("SELECT id, transcript, original_transcript, speaker_manual FROM transcripts WHERE meeting_id = ? AND {checked} ORDER BY audio_start_time"))
                .bind(&id)
                .fetch_all(pool)
                .await?;
        let lines: Vec<(String, Option<String>, Option<String>, i64)> = lines.into_iter().filter(|(l, ..)| !left_out.contains(l)).collect();
        let speaker_lines = lines.iter().filter(|l| l.3 != 0).count();
        let lines: Vec<(String, Option<String>)> = lines.into_iter().map(|(_, now, heard, _)| (now.unwrap_or_default(), heard)).collect();
        let (words, changed) = count(&lines);
        let mut kinds = edits(&lines, &dictionary);
        kinds.speaker_lines = speaker_lines;
        if words < MIN_WORDS {
            continue;
        }
        let folder = crate::speakers::lines::folder_of(pool, &id).await;
        let streams = folder.as_ref().is_some_and(|f| crate::speakers::words::load(f).is_some_and(|w| w.iter().any(|p| p.who.is_some())));
        let meet = folder.as_ref().is_some_and(|f| f.join(crate::meet::frames::FOLDER_FILE).exists());
        let streams_kept = folder.as_ref().is_some_and(|f| f.join(crate::speakers::slots::FOLDER_FILE).exists());
        meetings.push(Meeting {
            date: at.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string(),
            words,
            corrected: 100.0 * changed as f64 / words as f64,
            tuned: super::versions::tuned_by(&changes, copied_at, at),
            edits: kinds,
            streams,
            meet,
            streams_kept,
        });
    }
    meetings.reverse();
    let mut trainings: Vec<String> = super::versions::history(models)
        .iter()
        .filter(|r| r.outcome == super::versions::Outcome::Accepted)
        .filter_map(|r| run_date(&r.id))
        .collect();
    trainings.reverse();
    Ok(Quality { meetings, trainings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrected_words_are_counted_against_what_was_heard() {
        let lines = vec![
            // never edited: right as heard
            ("Доповідає керівник першого відділу".to_string(), None),
            // one word fixed, one added
            ("Завдання виконано повністю".to_string(), Some("Завдання виконана".to_string())),
            // edited back to what was heard
            ("Питань немає".to_string(), Some("питань немає".to_string())),
        ];
        assert_eq!(count(&lines), (9, 2));
        assert_eq!(count(&[]), (0, 0));
    }

    #[test]
    fn corrections_are_sorted_by_kind() {
        let dictionary = super::super::measure::dictionary_words(&["Гончаренко".to_string()]);
        let lines = vec![
            // a name, a number and an ending
            ("Гончаренко доповів про 12 машин".to_string(), Some("Кончаренко доповів про 20 машина".to_string())),
            // fillers out, a missed word in, another word out
            ("Склад відкрито сьогодні".to_string(), Some("Ну склад е відкрито вже".to_string())),
            // capitals and punctuation only
            ("Питань немає.".to_string(), Some("питань немає".to_string())),
            // never edited
            ("Усе за планом".to_string(), None),
        ];
        let e = edits(&lines, &dictionary);
        assert_eq!((e.names, e.numbers, e.endings), (1, 1, 1), "{e:?}");
        assert_eq!((e.fillers, e.punctuation_lines), (2, 1), "{e:?}");
        assert_eq!(e.added + e.replaced + e.removed, 1, "«вже» became «сьогодні»: {e:?}");
        let (_, changed) = count(&lines);
        assert_eq!(e.words(), changed, "kinds add up to the corrected words");
    }

    #[test]
    fn names_written_small_and_changes_made_again_are_counted() {
        let dictionary = super::super::measure::dictionary_words(&["Гончаренко".to_string(), "відділ кадрів".to_string()]);
        let lines = vec![
            ("Гончаренко прибув".to_string(), Some("Кончаренко прибув".to_string())),
            // the same change again, and a word of a longer term (written small)
            ("Доповідає Гончаренко з відділу кадрів".to_string(), Some("Доповідає Кончаренко з відділу кадр".to_string())),
            // the same name heard another way: not a repeat
            ("Гончаренко погодив".to_string(), Some("Гончарено погодив".to_string())),
            // an extra word taken out twice
            ("Склад відкрито".to_string(), Some("Склад вже відкрито".to_string())),
            ("Машини готові".to_string(), Some("Машини вже готові".to_string())),
        ];
        let e = edits(&lines, &dictionary);
        assert_eq!(e.names, 4, "{e:?}");
        assert_eq!(e.names_lower, 1, "«кадрів» is a word of a longer term: {e:?}");
        assert_eq!(e.names_repeats, 1, "{e:?}");
        assert_eq!(e.repeats, 2, "the second «Кончаренко» and the second «вже»: {e:?}");
        let mut sum = Edits::default();
        sum.add(&e);
        sum.add(&e);
        assert_eq!((sum.names_lower, sum.names_repeats, sum.repeats), (2, 2, 4));
    }

    #[test]
    fn times_as_the_database_keeps_them() {
        assert!(parse_time("2026-11-15T09:30:00Z").is_some());
        assert!(parse_time("2026-11-15T09:30:00.123+00:00").is_some());
        assert!(parse_time("2026-11-15 09:30:00.123+00:00").is_some());
        assert!(parse_time("2026-11-15 09:30:00").is_some());
        assert!(parse_time("вчора").is_none());
        assert_eq!(run_date("2026-11-15T01-30").as_deref(), Some("2026-11-15"));
        assert_eq!(run_date("base"), None);
    }
}
