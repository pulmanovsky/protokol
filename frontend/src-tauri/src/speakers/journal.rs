//! One history of the changes made by hand in a meeting's transcript —
//! text, speakers, cuts and joins, «перевірено», names of voices — so that
//! ⌘Z undoes the last one and ⌘⇧Z does it again, whatever it was.
//!
//! Each change stores the lines it touched and the meeting's small side
//! records (speakers set by hand, cuts and joins, checks, the voices'
//! names) as they were before and after. Undo writes back «before», redo
//! «after». A new change drops what was undone; a new speaker recognition
//! (which writes every line again) starts a new history.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::collections::BTreeMap;

const ROW_COLUMNS: &str = "id, meeting_id, transcript, timestamp, summary, action_items, key_points, audio_start_time, audio_end_time, duration, speaker, original_transcript, speaker_manual, reviewed_at";

/// A transcript line with every column
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Line {
    id: String,
    transcript: String,
    timestamp: String,
    summary: Option<String>,
    action_items: Option<String>,
    key_points: Option<String>,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
    speaker: Option<String>,
    original_transcript: Option<String>,
    speaker_manual: i64,
    reviewed_at: Option<String>,
}

/// The meeting as far as changes by hand go
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    lines: BTreeMap<String, Line>,
    /// Side records, row by row (small tables, kept whole)
    side: BTreeMap<String, Vec<Value>>,
    /// Doubts on words, kept in the meeting's word file («Залишити як є»)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    words: Vec<super::words::WordMark>,
}

/// The step «Залишити як є» leaves in the history
pub const WORD_KEPT: &str = "слово підтверджено";

/// The side tables and their columns (the blob of a voice kept as hex)
const SIDE: &[(&str, &str)] = &[
    ("speaker_overrides", "start, end, speaker, previous, created_at"),
    ("segment_edits", "kind, at, created_at"),
    ("segment_reviews", "start, end, created_at"),
    ("segment_exclusions", "start, end, created_at"),
    ("meeting_speakers", "label, name, hex(centroid) AS centroid, speech_seconds, role, suggested"),
];

pub async fn capture(pool: &SqlitePool, meeting_id: &str) -> Result<State> {
    let rows = sqlx::query(&format!("SELECT {ROW_COLUMNS} FROM transcripts WHERE meeting_id = ?")).bind(meeting_id).fetch_all(pool).await?;
    let mut lines = BTreeMap::new();
    for r in rows {
        let line = Line {
            id: r.get("id"),
            transcript: r.get("transcript"),
            timestamp: r.get("timestamp"),
            summary: r.get("summary"),
            action_items: r.get("action_items"),
            key_points: r.get("key_points"),
            audio_start_time: r.get("audio_start_time"),
            audio_end_time: r.get("audio_end_time"),
            duration: r.get("duration"),
            speaker: r.get("speaker"),
            original_transcript: r.get("original_transcript"),
            speaker_manual: r.try_get::<Option<i64>, _>("speaker_manual").ok().flatten().unwrap_or(0),
            reviewed_at: r.get("reviewed_at"),
        };
        lines.insert(line.id.clone(), line);
    }
    let mut side = BTreeMap::new();
    for (table, columns) in SIDE {
        let rows = sqlx::query(&format!("SELECT {columns} FROM {table} WHERE meeting_id = ? ORDER BY rowid")).bind(meeting_id).fetch_all(pool).await?;
        let values = rows.iter().map(row_json).collect();
        side.insert(table.to_string(), values);
    }
    Ok(State { lines, side, words: Vec::new() })
}

fn row_json(r: &sqlx::sqlite::SqliteRow) -> Value {
    use sqlx::{Column, TypeInfo, ValueRef};
    let mut m = serde_json::Map::new();
    for (i, c) in r.columns().iter().enumerate() {
        let raw = r.try_get_raw(i).ok();
        let v = match raw {
            Some(v) if v.is_null() => Value::Null,
            Some(v) => match v.type_info().name() {
                "INTEGER" => r.try_get::<i64, _>(i).map(Value::from).unwrap_or(Value::Null),
                "REAL" => r.try_get::<f64, _>(i).map(Value::from).unwrap_or(Value::Null),
                _ => r.try_get::<String, _>(i).map(Value::from).unwrap_or(Value::Null),
            },
            None => Value::Null,
        };
        m.insert(c.name().to_string(), v);
    }
    Value::Object(m)
}

/// Only what differs: the lines changed, added or removed, and side tables that changed
fn diff(before: &State, after: &State) -> (State, State) {
    let mut b = State::default();
    let mut a = State::default();
    let ids: std::collections::BTreeSet<&String> = before.lines.keys().chain(after.lines.keys()).collect();
    for id in ids {
        let (x, y) = (before.lines.get(id), after.lines.get(id));
        if x != y {
            if let Some(x) = x {
                b.lines.insert(id.clone(), x.clone());
            }
            if let Some(y) = y {
                a.lines.insert(id.clone(), y.clone());
            }
            // A line that did not exist on one side is marked so it is removed there
            if x.is_none() {
                b.side.entry("absent".into()).or_default().push(Value::from(id.clone()));
            }
            if y.is_none() {
                a.side.entry("absent".into()).or_default().push(Value::from(id.clone()));
            }
        }
    }
    for (table, _) in SIDE {
        let (x, y) = (before.side.get(*table), after.side.get(*table));
        if x != y {
            b.side.insert(table.to_string(), x.cloned().unwrap_or_default());
            a.side.insert(table.to_string(), y.cloned().unwrap_or_default());
        }
    }
    (b, a)
}

/// Records a change made since `before` was captured; nothing when nothing changed
pub async fn record(pool: &SqlitePool, meeting_id: &str, label: &str, before: State) -> Result<bool> {
    let after = capture(pool, meeting_id).await?;
    let (b, a) = diff(&before, &after);
    if b == a {
        return Ok(false);
    }
    insert_step(pool, meeting_id, label, &b, &a).await?;
    Ok(true)
}

/// A word confirmed as it is: one step of the history, with no line changed
pub async fn record_word(pool: &SqlitePool, meeting_id: &str, before: super::words::WordMark, after: super::words::WordMark) -> Result<()> {
    let b = State { words: vec![before], ..State::default() };
    let a = State { words: vec![after], ..State::default() };
    insert_step(pool, meeting_id, WORD_KEPT, &b, &a).await
}

/// A word confirmed and what that did to its line (checked): one step
pub async fn record_word_on_line(pool: &SqlitePool, meeting_id: &str, mark_before: super::words::WordMark, mark_after: super::words::WordMark, before: State) -> Result<()> {
    let after = capture(pool, meeting_id).await?;
    let (mut b, mut a) = diff(&before, &after);
    b.words = vec![mark_before];
    a.words = vec![mark_after];
    insert_step(pool, meeting_id, WORD_KEPT, &b, &a).await
}

async fn insert_step(pool: &SqlitePool, meeting_id: &str, label: &str, b: &State, a: &State) -> Result<()> {
    let mut tx = pool.begin().await?;
    // A new change: what was undone cannot be done again
    sqlx::query("DELETE FROM edit_journal WHERE meeting_id = ? AND undone = 1").bind(meeting_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO edit_journal (meeting_id, label, before_state, after_state, created_at) VALUES (?, ?, ?, ?, ?)")
        .bind(meeting_id)
        .bind(label)
        .bind(serde_json::to_string(b)?)
        .bind(serde_json::to_string(a)?)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
    // A long history is not needed
    sqlx::query("DELETE FROM edit_journal WHERE meeting_id = ? AND id NOT IN (SELECT id FROM edit_journal WHERE meeting_id = ? ORDER BY id DESC LIMIT 200)")
        .bind(meeting_id)
        .bind(meeting_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A new speaker recognition wrote every line again: the history starts over
pub async fn clear(pool: &SqlitePool, meeting_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM edit_journal WHERE meeting_id = ?").bind(meeting_id).execute(pool).await?;
    Ok(())
}

/// Writes a state back: its lines (removing those absent in it) and side tables
async fn restore(pool: &SqlitePool, meeting_id: &str, state: &State) -> Result<Vec<(String, String, f64)>> {
    let mut texts = Vec::new(); // (old text, new text, start) for the word times
    let mut tx = pool.begin().await?;
    if let Some(absent) = state.side.get("absent") {
        for id in absent.iter().filter_map(|v| v.as_str()) {
            sqlx::query("DELETE FROM transcripts WHERE id = ? AND meeting_id = ?").bind(id).bind(meeting_id).execute(&mut *tx).await?;
        }
    }
    for line in state.lines.values() {
        let current: Option<String> = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = ?").bind(&line.id).fetch_optional(&mut *tx).await?;
        if let Some(c) = current.filter(|c| c != &line.transcript) {
            texts.push((c, line.transcript.clone(), line.audio_start_time.unwrap_or(0.0)));
        }
        sqlx::query(&format!(
            "INSERT OR REPLACE INTO transcripts ({ROW_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(&line.id)
        .bind(meeting_id)
        .bind(&line.transcript)
        .bind(&line.timestamp)
        .bind(&line.summary)
        .bind(&line.action_items)
        .bind(&line.key_points)
        .bind(line.audio_start_time)
        .bind(line.audio_end_time)
        .bind(line.duration)
        .bind(&line.speaker)
        .bind(&line.original_transcript)
        .bind(line.speaker_manual)
        .bind(&line.reviewed_at)
        .execute(&mut *tx)
        .await?;
    }
    for (table, columns) in SIDE {
        let Some(rows) = state.side.get(*table) else { continue };
        if *table == "meeting_speakers" {
            // Voices: names and roles back; a voice merged away comes back too
            sqlx::query("DELETE FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).execute(&mut *tx).await?;
            for r in rows {
                let hex = r["centroid"].as_str().unwrap_or("");
                let centroid: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect();
                sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, name, centroid, speech_seconds, role, suggested) VALUES (?, ?, ?, ?, ?, ?, ?)")
                    .bind(meeting_id)
                    .bind(r["label"].as_str())
                    .bind(r["name"].as_str())
                    .bind(centroid)
                    .bind(r["speech_seconds"].as_f64().unwrap_or(0.0))
                    .bind(r["role"].as_str())
                    .bind(r["suggested"].as_str())
                    .execute(&mut *tx)
                    .await?;
            }
            continue;
        }
        sqlx::query(&format!("DELETE FROM {table} WHERE meeting_id = ?")).bind(meeting_id).execute(&mut *tx).await?;
        let names: Vec<&str> = columns.split(',').map(|c| c.trim()).collect();
        for r in rows {
            let sql = format!("INSERT INTO {table} (meeting_id, {}) VALUES (?, {})", names.join(", "), vec!["?"; names.len()].join(", "));
            let mut q = sqlx::query(&sql).bind(meeting_id);
            for n in &names {
                q = match &r[*n] {
                    Value::Null => q.bind(None::<String>),
                    Value::Number(x) if x.is_i64() => q.bind(x.as_i64()),
                    Value::Number(x) => q.bind(x.as_f64()),
                    v => q.bind(v.as_str().map(str::to_string)),
                };
            }
            q.execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(texts)
}

/// What ⌘Z and ⌘⇧Z would do now (labels), for the buttons' hints
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct History {
    pub undo: Option<String>,
    pub redo: Option<String>,
}

pub async fn history(pool: &SqlitePool, meeting_id: &str) -> Result<History> {
    let undo: Option<String> = sqlx::query_scalar("SELECT label FROM edit_journal WHERE meeting_id = ? AND undone = 0 ORDER BY id DESC LIMIT 1").bind(meeting_id).fetch_optional(pool).await?;
    let redo: Option<String> = sqlx::query_scalar("SELECT label FROM edit_journal WHERE meeting_id = ? AND undone = 1 ORDER BY id ASC LIMIT 1").bind(meeting_id).fetch_optional(pool).await?;
    Ok(History { undo, redo })
}

async fn step(pool: &SqlitePool, meeting_id: &str, undo: bool) -> Result<Option<String>> {
    let sql = if undo {
        "SELECT id, label, before_state FROM edit_journal WHERE meeting_id = ? AND undone = 0 ORDER BY id DESC LIMIT 1"
    } else {
        "SELECT id, label, after_state FROM edit_journal WHERE meeting_id = ? AND undone = 1 ORDER BY id ASC LIMIT 1"
    };
    let Some((id, label, state)) = sqlx::query_as::<_, (i64, String, String)>(sql).bind(meeting_id).fetch_optional(pool).await? else { return Ok(None) };
    let state: State = serde_json::from_str(&state)?;
    let texts = restore(pool, meeting_id, &state).await?;
    sqlx::query("UPDATE edit_journal SET undone = ? WHERE id = ?").bind(undo as i64).bind(id).execute(pool).await?;
    // Doubts on words live in the meeting's word file
    if !state.words.is_empty() {
        let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(pool).await?.flatten();
        if let Some(folder) = folder.map(std::path::PathBuf::from) {
            if let Some(mut pieces) = super::words::load(&folder) {
                let mut changed = false;
                for m in &state.words {
                    changed |= super::words::set_mark(&mut pieces, m);
                }
                if changed {
                    super::words::save(&folder, &pieces)?;
                }
            }
        }
    }
    // The word times follow the text
    if !texts.is_empty() {
        let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(pool).await?.flatten();
        if let Some(folder) = folder.map(std::path::PathBuf::from) {
            if let Some(mut pieces) = super::words::load(&folder) {
                let mut changed = false;
                for (old, new, near) in &texts {
                    changed |= super::words::replace_line(&mut pieces, old, new, *near);
                }
                if changed {
                    let _ = super::words::save(&folder, &pieces);
                }
            }
        }
    }
    Ok(Some(label))
}

pub async fn undo_last(pool: &SqlitePool, meeting_id: &str) -> Result<Option<String>> {
    step(pool, meeting_id, true).await
}

pub async fn redo_last(pool: &SqlitePool, meeting_id: &str) -> Result<Option<String>> {
    step(pool, meeting_id, false).await
}

/// Runs a change and records it under `label`
pub async fn journaled<T, F, Fut>(pool: &SqlitePool, meeting_id: &str, label: &str, change: F) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let before = capture(pool, meeting_id).await.map_err(|e| e.to_string());
    let out = change().await?;
    if let Ok(before) = before {
        if let Err(e) = record(pool, meeting_id, label, before).await {
            log::warn!("Could not record the change for undo: {e}");
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn undo_edit<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<Option<String>, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    undo_last(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn redo_edit<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<Option<String>, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    redo_last(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_edit_history<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<History, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    history(&pool, &meeting_id).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn meeting() -> (tempfile::TempDir, crate::database::manager::DatabaseManager) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(db.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES
             ('a', 'm1', 'Добрий ранок, починаємо. Склад отримав генератори', 'x', 0.0, 10.0, 'Тарас'),
             ('b', 'm1', 'Дякую, далі зв''язок', 'x', 10.0, 14.0, 'Остап')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, name, centroid, speech_seconds) VALUES ('m1', 'Спікер 1', 'Тарас', x'0102', 10), ('m1', 'Спікер 2', 'Остап', x'0304', 4)")
            .execute(&pool)
            .await
            .unwrap();
        (dir, manager)
    }

    async fn ok<T>(r: Result<T, anyhow::Error>) -> Result<T, String> {
        r.map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn a_word_confirmed_once_is_gone_and_one_undo_brings_it_back() {
        use crate::speakers::words;
        let (dir, manager) = meeting().await;
        let pool = manager.pool().clone();
        let folder = dir.path().join("rec");
        std::fs::create_dir_all(&folder).unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'm1'").bind(folder.to_str().unwrap()).execute(&pool).await.unwrap();
        let pieces = vec![words::PieceWords {
            text: "Дякую, далі зв'язок".into(),
            start: 10.0,
            end: 14.0,
            words: vec![("Дякую,".into(), 10.0), ("далі".into(), 11.0), ("зв'язок".into(), 12.0)],
            probs: vec![0.3, 0.99, 0.4],
            ..serde_json::from_value(serde_json::json!({"text": "", "start": 0.0, "end": 0.0, "words": []})).unwrap()
        }];
        words::save(&folder, &pieces).unwrap();
        let rows = vec![("b".to_string(), "Дякую, далі зв'язок".to_string())];
        let doubts = || words::doubts(&words::load(&folder).unwrap(), &rows, &[]);
        assert_eq!(doubts().len(), 2);
        // Confirmed once: one less, and asking again changes nothing and adds no step
        crate::ukrainian::commands::resolve_doubt(&pool, "m1", 10.0, "Дякую,", None).await.unwrap();
        assert_eq!(doubts().len(), 1);
        assert!(doubts().iter().all(|d| d.word != "Дякую,"));
        crate::ukrainian::commands::resolve_doubt(&pool, "m1", 10.0, "Дякую,", None).await.unwrap();
        assert_eq!(doubts().len(), 1);
        assert_eq!(history(&pool, "m1").await.unwrap().undo.as_deref(), Some(WORD_KEPT));
        // One undo brings exactly that word back; redo confirms it again
        assert_eq!(undo_last(&pool, "m1").await.unwrap().as_deref(), Some(WORD_KEPT));
        assert_eq!(doubts().len(), 2);
        assert_eq!(history(&pool, "m1").await.unwrap().undo, None, "only one step");
        assert_eq!(redo_last(&pool, "m1").await.unwrap().as_deref(), Some(WORD_KEPT));
        assert_eq!(doubts().len(), 1);
        assert!(doubts().iter().all(|d| d.word != "Дякую,"));
    }

    #[tokio::test]
    async fn a_word_kept_checks_its_line_and_undo_clears_it() {
        use crate::speakers::words;
        let (dir, manager) = meeting().await;
        let pool = manager.pool().clone();
        let folder = dir.path().join("rec");
        std::fs::create_dir_all(&folder).unwrap();
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'm1'").bind(folder.to_str().unwrap()).execute(&pool).await.unwrap();
        let pieces = vec![words::PieceWords {
            text: "Дякую, далі зв'язок".into(),
            start: 10.0,
            end: 14.0,
            words: vec![("Дякую,".into(), 10.0), ("далі".into(), 11.0), ("зв'язок".into(), 12.0)],
            probs: vec![0.3, 0.99, 0.4],
            ..serde_json::from_value(serde_json::json!({"text": "", "start": 0.0, "end": 0.0, "words": []})).unwrap()
        }];
        words::save(&folder, &pieces).unwrap();
        let reviewed = |pool: SqlitePool| async move { sqlx::query_scalar::<_, Option<String>>("SELECT reviewed_at FROM transcripts WHERE id = 'b'").fetch_one(&pool).await.unwrap().is_some() };
        assert!(!reviewed(pool.clone()).await);
        crate::ukrainian::commands::resolve_doubt(&pool, "m1", 10.0, "Дякую,", Some("b")).await.unwrap();
        assert!(reviewed(pool.clone()).await);
        // One step: the word and the mark go back together, and come again together
        assert_eq!(undo_last(&pool, "m1").await.unwrap().as_deref(), Some(WORD_KEPT));
        assert!(!reviewed(pool.clone()).await);
        assert_eq!(history(&pool, "m1").await.unwrap().undo, None);
        assert_eq!(redo_last(&pool, "m1").await.unwrap().as_deref(), Some(WORD_KEPT));
        assert!(reviewed(pool.clone()).await);
    }

    #[tokio::test]
    async fn everything_marked_is_one_step_and_keeps_earlier_marks() {
        let (_dir, manager) = meeting().await;
        let pool = manager.pool().clone();
        crate::speakers::lines::set_reviewed(&pool, "m1", &["a".to_string()], true).await.unwrap();
        let earlier: Option<String> = sqlx::query_scalar("SELECT reviewed_at FROM transcripts WHERE id = 'a'").fetch_one(&pool).await.unwrap();
        journaled(&pool, "m1", "усе", || async { crate::speakers::lines::mark_all_reviewed(&pool, "m1").await.map_err(|e| e.to_string()) }).await.unwrap();
        let progress = crate::speakers::lines::progress(&pool, "m1").await.unwrap();
        assert_eq!(progress.reviewed, progress.total);
        let kept: Option<String> = sqlx::query_scalar("SELECT reviewed_at FROM transcripts WHERE id = 'a'").fetch_one(&pool).await.unwrap();
        assert_eq!(kept, earlier);
        assert_eq!(undo_last(&pool, "m1").await.unwrap().as_deref(), Some("усе"));
        let progress = crate::speakers::lines::progress(&pool, "m1").await.unwrap();
        assert_eq!(progress.reviewed, 1);
        assert_eq!(history(&pool, "m1").await.unwrap().undo, None);
    }

    #[tokio::test]
    async fn every_kind_of_change_is_undone_and_done_again() {
        let (_d, m) = meeting().await;
        let pool = m.pool().clone();
        let start = capture(&pool, "m1").await.unwrap();

        // «Це сказав…»: the start of a line cut off and given to another person
        let id = journaled(&pool, "m1", "слова → Остап", || async {
            let id = ok(crate::speakers::lines::cut_out(&pool, "m1", "a", 0, 3).await).await?;
            ok(crate::speakers::overrides::reassign(&pool, "m1", &id, "Остап", crate::speakers::overrides::Scope::Line).await).await?;
            Ok(id)
        })
        .await
        .unwrap();
        assert_eq!(id, "a");
        let cut = capture(&pool, "m1").await.unwrap();
        assert_eq!(cut.lines.len(), 3);
        // Checked, then a text fix
        journaled(&pool, "m1", "позначка «перевірено»", || async { ok(crate::speakers::lines::set_reviewed(&pool, "m1", &["b".to_string()], true).await).await }).await.unwrap();
        journaled(&pool, "m1", "виправлення тексту", || async { crate::ukrainian::commands::edit_line_with(&pool, "m1", "b", "Дякую, далі зв'язок і пальне").await.map(|_| ()) }).await.unwrap();
        // Two voices made one
        journaled(&pool, "m1", "об'єднання голосів", || async { ok(crate::speakers::commands::merge_with(&pool, "m1", "Тарас", "Остап").await).await }).await.unwrap();
        let end = capture(&pool, "m1").await.unwrap();
        assert_eq!(history(&pool, "m1").await.unwrap().undo.as_deref(), Some("об'єднання голосів"));

        // ⌘Z four times: back to the start, exactly
        for label in ["об'єднання голосів", "виправлення тексту", "позначка «перевірено»", "слова → Остап"] {
            assert_eq!(undo_last(&pool, "m1").await.unwrap().as_deref(), Some(label));
        }
        assert_eq!(undo_last(&pool, "m1").await.unwrap(), None);
        assert_eq!(capture(&pool, "m1").await.unwrap(), start);
        // ⌘⇧Z: forward again to the end
        for _ in 0..4 {
            assert!(redo_last(&pool, "m1").await.unwrap().is_some());
        }
        assert_eq!(capture(&pool, "m1").await.unwrap(), end);
        // Undo one, then a new change: what was undone cannot be redone
        undo_last(&pool, "m1").await.unwrap();
        journaled(&pool, "m1", "розрізання рядка", || async { ok(crate::speakers::lines::split(&pool, "m1", "b", 1).await).await }).await.unwrap();
        let h = history(&pool, "m1").await.unwrap();
        assert_eq!((h.undo.as_deref(), h.redo), (Some("розрізання рядка"), None));
        // Nothing changed: nothing recorded
        assert!(!record(&pool, "m1", "нічого", capture(&pool, "m1").await.unwrap()).await.unwrap());
        // A new recognition starts over
        clear(&pool, "m1").await.unwrap();
        assert_eq!(history(&pool, "m1").await.unwrap(), History::default());
        let _ = cut;
    }
}
