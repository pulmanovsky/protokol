//! Tauri commands for the glossary.

#[tauri::command]
pub async fn api_get_glossary() -> Result<Vec<String>, String> {
    Ok(super::glossary())
}

#[tauri::command]
pub async fn api_set_glossary(terms: Vec<String>) -> Result<Vec<String>, String> {
    super::set_glossary(terms)
}

/// Names, nicknames and units from recent meetings that are not in the
/// glossary yet, grouped when they look like spellings of the same word
#[tauri::command]
pub async fn api_glossary_suggestions(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<super::corrections::Suggestion>, String> {
    let texts = recent_meeting_texts(state.db_manager.pool()).await?;
    Ok(super::corrections::suggestions_from(
        &[texts.join("\n")],
        &super::glossary_terms(),
        &super::corrections::corrections(),
        &super::corrections::dismissed(),
    ))
}

/// Confirms the right spelling: adds it to the glossary and corrects the others
#[tauri::command]
pub async fn api_accept_glossary_suggestion(right: String, variants: Vec<String>) -> Result<(), String> {
    super::corrections::accept(&right, &variants)
}

/// Hides a suggestion that is not a name or term
#[tauri::command]
pub async fn api_dismiss_glossary_suggestion(variants: Vec<String>) -> Result<(), String> {
    super::corrections::dismiss(&variants)
}

/// Transcripts of the last few meetings, one string per meeting
pub(crate) async fn recent_meeting_texts(pool: &sqlx::SqlitePool) -> Result<Vec<String>, String> {
    meeting_texts(pool, 8).await
}

/// Transcripts of all meetings (or the last `limit`), one string per meeting
pub(crate) async fn meeting_texts(pool: &sqlx::SqlitePool, limit: i64) -> Result<Vec<String>, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT t.meeting_id, t.transcript FROM transcripts t JOIN (SELECT id FROM meetings ORDER BY created_at DESC LIMIT ?) m ON t.meeting_id = m.id ORDER BY t.meeting_id, COALESCE(t.audio_start_time, 0)",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut by_meeting: Vec<(String, String)> = Vec::new();
    for (id, text) in rows {
        match by_meeting.iter_mut().find(|(m, _)| *m == id) {
            Some((_, all)) => {
                all.push('\n');
                all.push_str(&text);
            }
            None => by_meeting.push((id, text)),
        }
    }
    Ok(by_meeting.into_iter().map(|(_, t)| t).collect())
}

/// What the dictionary learned (from edits and from repeated names)
#[tauri::command]
pub async fn api_learned_terms() -> Result<Vec<super::learning::LearnedTerm>, String> {
    Ok(super::learning::learned_terms())
}

/// Removes a learned entry
#[tauri::command]
pub async fn api_forget_term(term: String) -> Result<(), String> {
    super::learning::forget(&term)
}

/// Scans all meetings and fills the dictionary on its own. Returns how many
/// meetings were read and how many entries were added.
#[tauri::command]
pub async fn api_learn_from_meetings(state: tauri::State<'_, crate::state::AppState>) -> Result<serde_json::Value, String> {
    let texts = meeting_texts(state.db_manager.pool(), -1).await?;
    let learned = tokio::task::spawn_blocking({
        let texts = texts.clone();
        move || super::learning::auto_learn(&texts)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "meetings": texts.len(), "learned": learned.len() }))
}

// ---------------------------------------------------------------------------
// Corrected transcript lines and what the app learns from them
// ---------------------------------------------------------------------------

/// A replacement found in a corrected line, offered for remembering
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct CorrectionPair {
    pub wrong: String,
    pub right: String,
    /// Made for the second time: remembered without asking
    #[serde(default)]
    pub remembered: bool,
}

async fn meeting_folder(pool: &sqlx::SqlitePool, meeting_id: &str) -> Option<std::path::PathBuf> {
    sqlx::query_scalar::<_, Option<String>>("SELECT folder_path FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .map(std::path::PathBuf::from)
}

/// Keeps the saved word times in step with a corrected line (see
/// `speakers::words::replace_line`)
fn sync_word_times(folder: Option<&std::path::Path>, old: &str, new: &str, near: f64) {
    let Some(folder) = folder else { return };
    if let Some(mut pieces) = crate::speakers::words::load(folder) {
        if crate::speakers::words::replace_line(&mut pieces, old, new, near) {
            let _ = crate::speakers::words::save(folder, &pieces);
        } else {
            log::info!("Corrected line not found in the word times; they no longer match");
        }
    }
}

/// Saves a line the user corrected by hand. The recognised text is kept (to
/// undo). Returns the replacements found, to offer remembering them.
/// When a fix typed in the text becomes a rule
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InlineLearning {
    /// The same fix made twice
    SecondFix,
    /// The first small fix (with «Додано в пам'ять» and «Скасувати»)
    FirstFix,
}

/// The one switch for how fixes typed in the text teach the app
pub const INLINE_LEARNING: InlineLearning = InlineLearning::FirstFix;

/// Test builds can try the other policy (PROTOKOL_TEST_INLINE_FIRST=1); release builds cannot
fn inline_learning() -> InlineLearning {
    #[cfg(feature = "test-hooks")]
    if std::env::var("PROTOKOL_TEST_INLINE_FIRST").as_deref() == Ok("1") {
        return InlineLearning::FirstFix;
    }
    INLINE_LEARNING
}

/// Short function words teach nothing (as in «Перевірте»)
const FUNCTION_WORDS: &[&str] = &["й", "і", "у", "в", "з", "а", "та", "чи", "же", "ж", "би", "б", "не", "ні", "що", "як", "це"];

fn is_function_word(w: &str) -> bool {
    let bare = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    FUNCTION_WORDS.contains(&bare.as_str())
}

/// A substitution of one or two words; a rewritten sentence teaches nothing
fn small_fix(pairs: &[(String, String)]) -> bool {
    !pairs.is_empty() && pairs.len() <= 2 && pairs.iter().all(|(w, r)| w.split_whitespace().count() <= 2 && r.split_whitespace().count() <= 2)
}

/// Which pairs of an edit become rules now
fn learn_now(policy: InlineLearning, pairs: &[(String, String)], again: &[bool], refused: &dyn Fn(&str, &str) -> bool) -> Vec<bool> {
    let small = small_fix(pairs);
    pairs
        .iter()
        .zip(again)
        .map(|((w, r), a)| {
            if refused(w, r) {
                return false;
            }
            *a || (policy == InlineLearning::FirstFix && small && !is_function_word(w) && !is_function_word(r))
        })
        .collect()
}

pub async fn edit_line_with(pool: &sqlx::SqlitePool, meeting_id: &str, segment_id: &str, text: &str) -> Result<Vec<CorrectionPair>, String> {
    edit_line(pool, meeting_id, segment_id, text, true, None).await
}

/// Saves a line. `finished` is false for the save while typing pauses: a
/// half-typed word must not teach anything; the save when the line is left
/// (even with the text already stored by then) does the learning.
///
/// `before` is the line as it was when this edit began: words already fixed
/// by an earlier edit of the line are not learned again.
pub async fn edit_line(pool: &sqlx::SqlitePool, meeting_id: &str, segment_id: &str, text: &str, finished: bool, before: Option<&str>) -> Result<Vec<CorrectionPair>, String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("Рядок не може бути порожнім".into());
    }
    let row: Option<(String, Option<String>, Option<f64>)> =
        sqlx::query_as("SELECT transcript, original_transcript, audio_start_time FROM transcripts WHERE id = ? AND meeting_id = ?")
            .bind(segment_id)
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    let Some((current, original, start)) = row else { return Err("Рядок не знайдено".into()) };
    if current == text && !finished {
        return Ok(Vec::new());
    }
    if current != text {
        sqlx::query("UPDATE transcripts SET transcript = ?, original_transcript = COALESCE(original_transcript, ?) WHERE id = ?")
            .bind(&text)
            .bind(&current)
            .bind(segment_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        sync_word_times(meeting_folder(pool, meeting_id).await.as_deref(), &current, &text, start.unwrap_or(0.0));
    }
    if !finished {
        return Ok(Vec::new());
    }
    // Learn against what was recognised, not an earlier hand edit
    let mut pairs = super::learning::edit_pairs(original.as_deref().unwrap_or(&current), &text);
    if let Some(b) = before {
        pairs.retain(|(_, right)| !b.contains(right.as_str()));
    }
    // The same correction a second time (or, by the switch, a first small one): remember it
    let again = super::learning::note_seen(&pairs);
    let learned = learn_now(inline_learning(), &pairs, &again, &super::learning::refused);
    let auto: Vec<(String, String)> = pairs.iter().zip(&learned).filter(|(_, a)| **a).map(|(p, _)| p.clone()).collect();
    let had_term = |t: &str| super::glossary().iter().any(|l| super::term_of(l) == t);
    let new_terms: Vec<String> = auto.iter().filter(|(_, r)| !had_term(r)).map(|(_, r)| r.clone()).collect();
    if !auto.is_empty() && super::learning::remember(&auto, &text).is_ok() {
        for t in &new_terms {
            super::learning::mark_added_by_rule(t);
        }
        for (wrong, right) in &auto {
            crate::learned_events::note("word", wrong, right, 1, Some(meeting_id));
        }
    }
    Ok(pairs.into_iter().zip(learned).map(|((wrong, right), remembered)| CorrectionPair { wrong, right, remembered }).collect())
}

#[tauri::command]
pub async fn api_edit_transcript_line(
    state: tauri::State<'_, crate::state::AppState>,
    meeting_id: String,
    segment_id: String,
    text: String,
    finished: Option<bool>,
    before: Option<String>,
    reviewed: Option<bool>,
) -> Result<Vec<CorrectionPair>, String> {
    let pool = state.db_manager.pool();
    crate::speakers::journal::journaled(pool, &meeting_id, "виправлення тексту", || async {
        let pairs = edit_line(pool, &meeting_id, &segment_id, &text, finished.unwrap_or(true), before.as_deref()).await?;
        // Fixed in the word strip: the line counts as checked, in the same step
        if reviewed == Some(true) {
            crate::speakers::lines::set_reviewed(pool, &meeting_id, std::slice::from_ref(&segment_id), true).await.map_err(|e| e.to_string())?;
        }
        Ok(pairs)
    })
    .await
}

/// Puts back the recognised text of a corrected line
#[tauri::command]
pub async fn api_undo_transcript_edit(state: tauri::State<'_, crate::state::AppState>, meeting_id: String, segment_id: String) -> Result<(), String> {
    let pool = state.db_manager.pool();
    crate::speakers::journal::journaled(pool, &meeting_id, "розпізнаний текст рядка", || undo_edit_with(pool, &meeting_id, &segment_id)).await
}

async fn undo_edit_with(pool: &sqlx::SqlitePool, meeting_id: &str, segment_id: &str) -> Result<(), String> {
    let row: Option<(String, Option<String>, Option<f64>)> =
        sqlx::query_as("SELECT transcript, original_transcript, audio_start_time FROM transcripts WHERE id = ? AND meeting_id = ?")
            .bind(&segment_id)
            .bind(&meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
    let Some((current, Some(original), start)) = row else { return Ok(()) };
    sqlx::query("UPDATE transcripts SET transcript = ?, original_transcript = NULL WHERE id = ?")
        .bind(&original)
        .bind(&segment_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    sync_word_times(meeting_folder(pool, &meeting_id).await.as_deref(), &current, &original, start.unwrap_or(0.0));
    Ok(())
}

/// Remembers confirmed corrections: applied to new recordings from now on,
/// and the right spellings help Whisper (they join «Словник»)
#[tauri::command]
pub async fn api_remember_corrections(pairs: Vec<CorrectionPair>, example: String, meeting_id: Option<String>) -> Result<usize, String> {
    let pairs: Vec<(String, String)> = pairs.into_iter().map(|p| (p.wrong, p.right)).collect();
    let n = super::learning::remember(&pairs, &example)?;
    for (wrong, right) in &pairs {
        crate::learned_events::note("word", wrong, right, 1, meeting_id.as_deref());
    }
    Ok(n)
}

/// «Скасувати» on a rule learned from a fix in the text: the rule goes, and the
/// fix in the text stays; the same fix does not teach it again
#[tauri::command]
pub async fn api_forget_inline_rule(wrong: String, right: String) -> Result<(), String> {
    super::corrections::remove_correction(&wrong)?;
    super::learning::refuse(&wrong, &right);
    let still_used = super::corrections::all_corrections().iter().any(|c| c.right == right);
    if term_goes_with_rule(super::learning::added_by_rule(&right), still_used) {
        super::learning::forget(&right)?;
        super::learning::clear_added_by_rule(&right);
    }
    Ok(())
}

/// A cancelled rule takes its dictionary term along (so it leaves the Whisper
/// prompt) only if the rule added it and no other rule still writes that word
fn term_goes_with_rule(added_by_rule: bool, still_used_by_another_rule: bool) -> bool {
    added_by_rule && !still_used_by_another_rule
}

/// The stenogram of a meeting as a document: no model, the words as they stand
/// with the learned corrections applied
#[tauri::command]
pub async fn api_stenogram_markdown(state: tauri::State<'_, crate::state::AppState>, meeting_id: String) -> Result<serde_json::Value, String> {
    use crate::summary::materials::stenogram_markdown;
    let (title, created, lines) = stenogram_lines(state.db_manager.pool(), &meeting_id).await?;
    let date = chrono::DateTime::parse_from_rfc3339(&created).ok().map(|d| d.with_timezone(&chrono::Local).format("%d.%m.%Y").to_string());
    Ok(serde_json::json!({ "title": format!("Стенограма — {title}"), "markdown": stenogram_markdown(&title, date.as_deref(), &lines) }))
}

/// The whole stenogram as plain text with speakers and times
#[tauri::command]
pub async fn api_stenogram_plain(state: tauri::State<'_, crate::state::AppState>, meeting_id: String) -> Result<serde_json::Value, String> {
    let (_, created, lines) = stenogram_lines(state.db_manager.pool(), &meeting_id).await?;
    let when = chrono::DateTime::parse_from_rfc3339(&created).ok().map(|d| d.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M").to_string()).unwrap_or_default();
    let (text, turns) = crate::summary::materials::stenogram_plain(format!("Нарада {when}").trim(), &lines);
    Ok(serde_json::json!({ "text": text, "turns": turns }))
}

async fn stenogram_lines(pool: &sqlx::SqlitePool, meeting_id: &str) -> Result<(String, String, Vec<crate::summary::materials::StenoLine>), String> {
    use crate::summary::materials::StenoLine;
    let (title, created): (String, String) = sqlx::query_as("SELECT title, CAST(created_at AS TEXT) FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<(Option<String>, String, Option<f64>, Option<f64>)> =
        sqlx::query_as("SELECT speaker, transcript, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    let lines: Vec<StenoLine> = rows
        .into_iter()
        .map(|(speaker, text, a, b)| StenoLine { speaker: speaker.unwrap_or_default(), text: super::corrections::apply_corrections(&text), start: a.unwrap_or(0.0), end: b.or(a).unwrap_or(0.0) })
        .collect();
    Ok((title, created, lines))
}

/// Learned corrections for «Словник»
#[tauri::command]
pub async fn api_corrections() -> Result<Vec<super::corrections::Correction>, String> {
    Ok(super::corrections::all_corrections())
}

#[tauri::command]
pub async fn api_set_correction_enabled(wrong: String, enabled: bool) -> Result<(), String> {
    super::corrections::set_correction_enabled(&wrong, enabled)
}

#[tauri::command]
pub async fn api_delete_correction(wrong: String) -> Result<(), String> {
    super::corrections::remove_correction(&wrong)
}

/// Applies the learned corrections to a meeting's existing text; returns how
/// many lines changed (each keeps its recognised text for undo)
pub async fn apply_to_meeting_with(pool: &sqlx::SqlitePool, meeting_id: &str) -> Result<usize, String> {
    apply_rules(pool, meeting_id, None).await
}

/// The rules to apply: all of them, or only those just made (one fix must not
/// touch lines that other, older rules would change)
fn rules_to_apply(all: Vec<(String, String)>, only: Option<&[String]>) -> Vec<(String, String)> {
    match only {
        Some(wrong) => all.into_iter().filter(|(w, _)| wrong.iter().any(|x| x.to_lowercase() == w.to_lowercase())).collect(),
        None => all,
    }
}

async fn apply_rules(pool: &sqlx::SqlitePool, meeting_id: &str, only: Option<&[String]>) -> Result<usize, String> {
    let pairs = rules_to_apply(super::corrections::corrections(), only);
    if pairs.is_empty() {
        return Ok(0);
    }
    let rows: Vec<(String, String, Option<f64>)> =
        sqlx::query_as("SELECT id, transcript, audio_start_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    let folder = meeting_folder(pool, meeting_id).await;
    let mut changed = 0;
    let mut hits = Vec::new();
    for (id, text, start) in rows {
        let fixed = super::corrections::apply_corrections_with(&text, &pairs);
        if fixed != text {
            hits.extend(super::corrections::applied_rules(&text, &pairs));
            sqlx::query("UPDATE transcripts SET transcript = ?, original_transcript = COALESCE(original_transcript, ?) WHERE id = ?")
                .bind(&fixed)
                .bind(&text)
                .bind(&id)
                .execute(pool)
                .await
                .map_err(|e| e.to_string())?;
            sync_word_times(folder.as_deref(), &text, &fixed, start.unwrap_or(0.0));
            changed += 1;
        }
    }
    crate::learned_events::record_auto_fixes(meeting_id, &hits);
    Ok(changed)
}

#[tauri::command]
pub async fn api_apply_corrections_to_meeting(state: tauri::State<'_, crate::state::AppState>, meeting_id: String, only: Option<Vec<String>>) -> Result<usize, String> {
    let pool = state.db_manager.pool();
    crate::speakers::journal::journaled(pool, &meeting_id, "виправлення зі словника", || apply_rules(pool, &meeting_id, only.as_deref())).await
}

/// Options for word `word_index` of a line («Текст» mode): what recognition
/// heard there before a fix, earlier corrections, dictionary sound-alikes
#[tauri::command]
pub async fn api_word_alternatives(state: tauri::State<'_, crate::state::AppState>, meeting_id: String, segment_id: String, word_index: usize) -> Result<Vec<super::doubt::Alternative>, String> {
    let pool = state.db_manager.pool();
    let row: Option<(String, Option<String>)> = sqlx::query_as("SELECT transcript, original_transcript FROM transcripts WHERE id = ? AND meeting_id = ?")
        .bind(&segment_id)
        .bind(&meeting_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    let Some((text, original)) = row else { return Ok(Vec::new()) };
    let words: Vec<&str> = text.split_whitespace().collect();
    let Some(word) = words.get(word_index) else { return Ok(Vec::new()) };
    // What was heard at the same place, when the line was corrected word for word
    let heard = original.as_deref().and_then(|o| {
        let ow: Vec<&str> = o.split_whitespace().collect();
        (ow.len() == words.len()).then(|| ow[word_index].to_string()).filter(|h| h != word)
    });
    let corrections: Vec<(String, String)> = super::corrections::all_corrections().into_iter().filter(|c| c.enabled).map(|c| (c.wrong, c.right)).collect();
    Ok(super::doubt::alternatives(word, heard.as_deref(), &super::glossary_terms(), &corrections))
}

#[cfg(test)]
mod edit_tests {
    #[test]
    fn what_a_fix_in_the_text_teaches() {
        use super::{learn_now, InlineLearning::*};
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        let no = |_: &str, _: &str| false;
        let one = [pair("репеті", "ЦНАП")];
        // Today: only the second time
        assert_eq!(learn_now(SecondFix, &one, &[false], &no), [false]);
        assert_eq!(learn_now(SecondFix, &one, &[true], &no), [true]);
        // By the switch: the first small fix
        assert_eq!(learn_now(FirstFix, &one, &[false], &no), [true]);
        assert_eq!(learn_now(FirstFix, &[pair("вузол два", "Вузол-2")], &[false], &no), [true]);
        // Three changes in a line, or a long replacement: a rewrite teaches nothing the first time
        let many = [pair("один", "раз"), pair("дві", "двоє"), pair("три", "трійка")];
        assert_eq!(learn_now(FirstFix, &many, &[false, false, false], &no), [false, false, false]);
        assert_eq!(learn_now(FirstFix, &[pair("це було дуже", "так вийшло")], &[false], &no), [false]);
        // Short function words do not
        assert_eq!(learn_now(FirstFix, &[pair("й", "і")], &[false], &no), [false]);
        assert_eq!(learn_now(FirstFix, &[pair("що", "щоб")], &[false], &no), [false]);
        // Re-correcting after one fix uses only that rule
        let all = vec![pair("прат", "ПрАТ"), pair("Репеті", "ЦНАП")];
        assert_eq!(super::rules_to_apply(all.clone(), Some(&["репеті".to_string()])), vec![pair("Репеті", "ЦНАП")]);
        assert_eq!(super::rules_to_apply(all.clone(), None).len(), 2);
        // A cancelled rule's dictionary term: only if it added it and nothing else uses it
        assert!(super::term_goes_with_rule(true, false));
        assert!(!super::term_goes_with_rule(false, false), "it was in the dictionary before");
        assert!(!super::term_goes_with_rule(true, true), "another rule still writes it");
        // «Скасувати» holds, even for a repeated fix
        let refused = |w: &str, _: &str| w == "репеті";
        assert_eq!(learn_now(FirstFix, &one, &[true], &refused), [false]);
        assert_eq!(learn_now(SecondFix, &one, &[true], &refused), [false]);
    }

    #[tokio::test]
    async fn a_corrected_line_keeps_the_recognised_text_and_offers_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(db.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time) VALUES ('t1', 'm1', 'Доповідає вузол два, прат на місці.', 'x', 1.0)")
            .execute(&pool)
            .await
            .unwrap();
        let pairs = super::edit_line_with(&pool, "m1", "t1", "Доповідає Вузол-2,  ПрАТ на місці.").await.unwrap();
        let got: Vec<(String, String)> = pairs.into_iter().map(|p| (p.wrong, p.right)).collect();
        assert_eq!(got, vec![("вузол два".to_string(), "Вузол-2".to_string()), ("прат".to_string(), "ПрАТ".to_string())]);
        let (text, original): (String, Option<String>) =
            sqlx::query_as("SELECT transcript, original_transcript FROM transcripts WHERE id = 't1'").fetch_one(&pool).await.unwrap();
        assert_eq!(text, "Доповідає Вузол-2, ПрАТ на місці.");
        assert_eq!(original.as_deref(), Some("Доповідає вузол два, прат на місці."));
        // A second edit still learns against the recognised text and keeps it
        super::edit_line_with(&pool, "m1", "t1", "Доповідає Вузол-2, ПрАТ на ділянці.").await.unwrap();
        let original: Option<String> = sqlx::query_scalar("SELECT original_transcript FROM transcripts WHERE id = 't1'").fetch_one(&pool).await.unwrap();
        assert_eq!(original.as_deref(), Some("Доповідає вузол два, прат на місці."));
        assert!(super::edit_line_with(&pool, "m1", "t1", "  ").await.is_err());
        // While typing pauses: stored, nothing offered; when the line is left, the pairs come
        assert!(super::edit_line(&pool, "m1", "t1", "Доповідає Вузол-2, ПрАТ на ділянцііі.", false, None).await.unwrap().is_empty());
        let t: String = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = 't1'").fetch_one(&pool).await.unwrap();
        assert_eq!(t, "Доповідає Вузол-2, ПрАТ на ділянцііі.");
        assert!(!super::edit_line(&pool, "m1", "t1", "Доповідає Вузол-2, ПрАТ на ділянцііі.", true, None).await.unwrap().is_empty());
        // Words fixed by an earlier edit of the line are not learned again
        let got = super::edit_line(&pool, "m1", "t1", "Доповідає Вузол-2, ПрАТ на місцях.", true, Some("Доповідає Вузол-2, ПрАТ на ділянцііі.")).await.unwrap();
        assert!(got.iter().all(|p| p.right != "Вузол-2" && p.right != "ПрАТ"), "{got:?}");
    }
}

// ---------------------------------------------------------------------------
// «Перевірте»: words Whisper was not sure of
// ---------------------------------------------------------------------------

/// Doubtful and auto-replaced words of a meeting, in order
#[tauri::command]
pub async fn api_get_doubts(state: tauri::State<'_, crate::state::AppState>, meeting_id: String) -> Result<Vec<crate::speakers::words::Doubt>, String> {
    let pool = state.db_manager.pool();
    let Some(folder) = meeting_folder(pool, &meeting_id).await else { return Ok(Vec::new()) };
    let Some(pieces) = crate::speakers::words::load(&folder) else { return Ok(Vec::new()) };
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, transcript FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
        .bind(&meeting_id)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(crate::speakers::words::doubts(&pieces, &rows, &super::glossary_terms()))
}

/// The user checked a doubtful word and left it as it is
#[tauri::command]
pub async fn api_resolve_doubt(state: tauri::State<'_, crate::state::AppState>, meeting_id: String, seconds: f64, word: String, segment_id: Option<String>) -> Result<(), String> {
    resolve_doubt(state.db_manager.pool(), &meeting_id, seconds, &word, segment_id.as_deref()).await
}

/// One word confirmed: gone from the doubts for good, and one step of ⌘Z
pub(crate) async fn resolve_doubt(pool: &sqlx::SqlitePool, meeting_id: &str, seconds: f64, word: &str, segment_id: Option<&str>) -> Result<(), String> {
    use crate::speakers::words;
    let Some(folder) = meeting_folder(pool, meeting_id).await else { return Ok(()) };
    if let Some(mut pieces) = words::load(&folder) {
        let before = words::mark_of(&pieces, seconds, word);
        let lines_before = if segment_id.is_some() { crate::speakers::journal::capture(pool, meeting_id).await.ok() } else { None };
        if words::resolve(&mut pieces, seconds, word) {
            words::save(&folder, &pieces).map_err(|e| e.to_string())?;
            if let (Some(b), Some(a)) = (before, words::mark_of(&pieces, seconds, word)) {
                if b != a {
                    let recorded = match (segment_id, lines_before) {
                        // The line it is in counts as checked, in the same step
                        (Some(id), Some(state)) => {
                            crate::speakers::lines::set_reviewed(pool, meeting_id, &[id.to_string()], true).await.map_err(|e| e.to_string())?;
                            crate::speakers::journal::record_word_on_line(pool, meeting_id, b, a, state).await
                        }
                        _ => crate::speakers::journal::record_word(pool, meeting_id, b, a).await,
                    };
                    if let Err(e) = recorded {
                        log::warn!("Could not record the confirmation for undo: {e}");
                    }
                }
            }
        }
    }
    Ok(())
}
