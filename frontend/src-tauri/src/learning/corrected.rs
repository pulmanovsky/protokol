//! «Якість розпізнавання»: how many words of each checked meeting had to
//! be corrected, and which model heard it.
//!
//! A line keeps what recognition wrote (`original_transcript`) from its
//! first edit; a line never edited was right as heard. Only checked lines
//! count, as in training: read, fixed or marked «перевірено», not left out
//! of the document. Numbers only leave this module: no texts.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

/// Meetings with fewer checked words say too little to show
pub const MIN_WORDS: usize = 100;
/// The newest meetings shown
pub const SHOWN: usize = 40;

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
    let changes = super::versions::changes(models);
    let copied_at = super::versions::copied_at(models, model_file);
    let mut meetings = Vec::new();
    for (id, created_at) in rows {
        if meetings.len() >= SHOWN {
            break;
        }
        let Some(at) = parse_time(&created_at) else { continue };
        let left_out = crate::speakers::lines::excluded_ids(pool, &id).await?;
        let lines: Vec<(String, Option<String>, Option<String>)> =
            sqlx::query_as(&format!("SELECT id, transcript, original_transcript FROM transcripts WHERE meeting_id = ? AND {checked}"))
                .bind(&id)
                .fetch_all(pool)
                .await?;
        let lines: Vec<(String, Option<String>)> =
            lines.into_iter().filter(|(l, ..)| !left_out.contains(l)).map(|(_, now, heard)| (now.unwrap_or_default(), heard)).collect();
        let (words, changed) = count(&lines);
        if words < MIN_WORDS {
            continue;
        }
        meetings.push(Meeting {
            date: at.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string(),
            words,
            corrected: 100.0 * changed as f64 / words as f64,
            tuned: super::versions::tuned_by(&changes, copied_at, at),
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
