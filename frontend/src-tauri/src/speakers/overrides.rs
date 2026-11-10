//! The speaker of a line set by hand («це говорив не він, а …»). Kept by
//! time in `speaker_overrides`, so a new speaker recognition puts it back;
//! the line is marked «змінено вручну» and can be undone.

use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

/// Which lines a change applies to
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// This line only
    Line,
    /// This line and the next ones until the voice changes
    Following,
}

/// The lines (id, start, end, speaker) the scope covers
pub(crate) fn lines_in_scope(rows: &[(String, f64, f64, Option<String>)], segment_id: &str, scope: Scope) -> Vec<(String, f64, f64, Option<String>)> {
    let Some(i) = rows.iter().position(|r| r.0 == segment_id) else { return Vec::new() };
    match scope {
        Scope::Line => vec![rows[i].clone()],
        Scope::Following => rows[i..].iter().take_while(|r| r.3 == rows[i].3).cloned().collect(),
    }
}

async fn rows_of(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<(String, f64, f64, Option<String>)>> {
    let rows: Vec<(String, Option<f64>, Option<f64>, Option<String>)> =
        sqlx::query_as("SELECT id, audio_start_time, audio_end_time, speaker FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time")
            .bind(meeting_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(id, a, b, s)| (id, a.unwrap_or(0.0), b.or(a).unwrap_or(0.0), s)).collect())
}

/// Gives lines to `speaker`; returns the lines changed (id, start, end)
pub async fn reassign(pool: &SqlitePool, meeting_id: &str, segment_id: &str, speaker: &str, scope: Scope) -> Result<Vec<(String, f64, f64)>> {
    let speaker = speaker.trim();
    if speaker.is_empty() {
        return Err(anyhow!("Вкажіть, хто говорив"));
    }
    let chosen = lines_in_scope(&rows_of(pool, meeting_id).await?, segment_id, scope);
    if chosen.is_empty() {
        return Err(anyhow!("Рядок не знайдено"));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    for (id, start, end, previous) in &chosen {
        sqlx::query("INSERT INTO speaker_overrides (meeting_id, start, end, speaker, previous, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(meeting_id)
            .bind(start)
            .bind(end)
            .bind(speaker)
            .bind(previous)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE transcripts SET speaker = ?, speaker_manual = 1 WHERE id = ?").bind(speaker).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    super::saved::unhide(pool, speaker).await?;
    ensure_listed(pool, meeting_id, speaker, chosen.iter().map(|c| (c.2 - c.1).max(0.0)).sum()).await?;
    Ok(chosen.into_iter().map(|(id, a, b, _)| (id, a, b)).collect())
}

/// A person new to the meeting is listed among its voices (no voice of its
/// own: the lines were given by hand)
async fn ensure_listed(pool: &SqlitePool, meeting_id: &str, speaker: &str, seconds: f64) -> Result<()> {
    let known: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_speakers WHERE meeting_id = ? AND (label = ? OR name = ?)")
        .bind(meeting_id)
        .bind(speaker)
        .bind(speaker)
        .fetch_one(pool)
        .await?;
    if known == 0 {
        sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, name, centroid, speech_seconds) VALUES (?, ?, ?, ?, ?)")
            .bind(meeting_id)
            .bind(speaker)
            .bind(speaker)
            .bind(Vec::<u8>::new())
            .bind(seconds)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Puts back what recognition said for a line changed by hand
pub async fn undo(pool: &SqlitePool, meeting_id: &str, segment_id: &str) -> Result<()> {
    let row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as("SELECT audio_start_time, audio_end_time FROM transcripts WHERE id = ? AND meeting_id = ?")
        .bind(segment_id)
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?;
    let Some((Some(a), b)) = row else { return Ok(()) };
    let b = b.unwrap_or(a);
    let previous: Option<Option<String>> = sqlx::query_scalar("SELECT previous FROM speaker_overrides WHERE meeting_id = ? AND start < ? AND end > ? ORDER BY id DESC LIMIT 1")
        .bind(meeting_id)
        .bind(b.max(a + 0.01))
        .bind(a)
        .fetch_optional(pool)
        .await?;
    sqlx::query("DELETE FROM speaker_overrides WHERE meeting_id = ? AND start < ? AND end > ?").bind(meeting_id).bind(b.max(a + 0.01)).bind(a).execute(pool).await?;
    sqlx::query("UPDATE transcripts SET speaker = ?, speaker_manual = 0 WHERE id = ?").bind(previous.flatten()).bind(segment_id).execute(pool).await?;
    Ok(())
}

/// After a new speaker recognition: lines that overlap a manual change by
/// at least half get that speaker again
pub async fn apply(pool: &SqlitePool, meeting_id: &str) -> Result<usize> {
    let overrides: Vec<(f64, f64, String)> = sqlx::query_as("SELECT start, end, speaker FROM speaker_overrides WHERE meeting_id = ? ORDER BY id").bind(meeting_id).fetch_all(pool).await?;
    if overrides.is_empty() {
        return Ok(0);
    }
    let mut changed = 0;
    let mut seconds: std::collections::HashMap<String, f64> = Default::default();
    for (id, a, b, _) in rows_of(pool, meeting_id).await? {
        let length = (b - a).max(0.01);
        if let Some((_, _, who)) = overrides.iter().rev().find(|(s, e, _)| (b.min(*e) - a.max(*s)).max(0.0) >= length / 2.0) {
            sqlx::query("UPDATE transcripts SET speaker = ?, speaker_manual = 1 WHERE id = ?").bind(who).bind(&id).execute(pool).await?;
            *seconds.entry(who.clone()).or_default() += b - a;
            changed += 1;
        }
    }
    for (who, s) in seconds {
        ensure_listed(pool, meeting_id, &who, s).await?;
    }
    Ok(changed)
}

/// Shortest line whose voice is added to a person's profile
pub const LEARN_MIN_SECONDS: f64 = 3.0;

/// The voice of lines given by hand to a person who has a voice profile is
/// added to that profile (lines of at least 3 s; at most 6 lines, so one
/// change cannot outweigh earlier meetings). Nothing is added to anyone else.
pub async fn learn_voice(pool: &SqlitePool, models: &std::path::Path, meeting_id: &str, speaker: &str, lines: &[(f64, f64)]) -> Result<usize> {
    use super::profiles::{from_blob, to_blob, updated_embedding};
    let lines: Vec<(f64, f64)> = lines.iter().copied().filter(|(a, b)| b - a >= LEARN_MIN_SECONDS).take(6).collect();
    if lines.is_empty() {
        return Ok(0);
    }
    let model = super::commands::model_id(models);
    // A person named by hand who has no voice yet gets one from these lines
    // (before, such fixes taught nothing: whole turns given to a new name)
    let known = sqlx::query_as::<_, (Vec<u8>, i64)>("SELECT embedding, meetings FROM voice_profiles WHERE name = ? AND model = ?").bind(speaker).bind(&model).fetch_optional(pool).await?;
    let unnamed = speaker.trim().is_empty() || speaker.starts_with("Спікер ") || speaker == super::commands::ME_LABEL || speaker.ends_with('?');
    if known.is_none() && unnamed {
        return Ok(0);
    }
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(pool).await?.flatten();
    let folder = std::path::PathBuf::from(folder.ok_or_else(|| anyhow!("Для цього запису немає папки зі звуком"))?);
    let audio = crate::audio::retranscription::find_audio_file(&folder)?;
    let emb_model = models.join(super::commands::EMBEDDING_MODEL);
    let found: Vec<Vec<f32>> = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
        let samples = super::commands::decode_16k(&audio)?;
        let mut embedder = super::embedding::Embedder::open(&emb_model)?;
        let mut out = Vec::new();
        for (a, b) in lines {
            let (i, j) = ((a * 16000.0) as usize, ((b * 16000.0) as usize).min(samples.len()));
            if j > i {
                if let Some(e) = embedder.embed(&samples[i..j])? {
                    out.push(e);
                }
            }
        }
        Ok(out)
    })
    .await??;
    if found.is_empty() {
        return Ok(0);
    }
    let mut mean = vec![0.0f32; found[0].len()];
    for e in &found {
        for (m, v) in mean.iter_mut().zip(e) {
            *m += v / found.len() as f32;
        }
    }
    match known {
        Some((embedding, meetings)) => {
            let updated = updated_embedding(&from_blob(&embedding), meetings, &mean);
            sqlx::query("UPDATE voice_profiles SET embedding = ?, updated_at = ? WHERE name = ? AND model = ?")
                .bind(to_blob(&updated))
                .bind(chrono::Utc::now().to_rfc3339())
                .bind(speaker)
                .bind(&model)
                .execute(pool)
                .await?;
        }
        None => {
            sqlx::query("INSERT INTO voice_profiles (name, embedding, model, meetings, updated_at) VALUES (?, ?, ?, 1, ?)")
                .bind(speaker)
                .bind(to_blob(&mean))
                .bind(&model)
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(pool)
                .await?;
            log::info!("New voice profile from lines given by hand");
        }
    }
    crate::learned_events::note("voice", "", speaker, 1, Some(meeting_id));
    Ok(found.len())
}

/// People to choose from: the meeting's voices first, then everyone with a
/// voice profile or a name in earlier meetings
pub async fn people(pool: &SqlitePool, meeting_id: &str) -> Result<(Vec<String>, Vec<String>)> {
    let here: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT speaker FROM transcripts WHERE meeting_id = ? AND speaker IS NOT NULL AND TRIM(speaker) <> '' ORDER BY audio_start_time",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    let mut seen = std::collections::HashSet::new();
    // «Репліки» (short remarks of several voices) is not a person
    let here: Vec<String> = here.into_iter().filter(|s| s != super::commands::REMARKS && seen.insert(s.clone())).collect();
    let known: Vec<String> = sqlx::query_scalar(
        // Not the names set aside in «Усі збережені»
        "SELECT name FROM (SELECT name FROM voice_profiles UNION SELECT name FROM meeting_speakers WHERE name IS NOT NULL)
         WHERE TRIM(name) <> '' AND name NOT IN (SELECT name FROM hidden_speaker_names) ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    let known = known.into_iter().filter(|n| !here.contains(n) && !n.ends_with('?')).collect();
    Ok((here, known))
}

#[derive(serde::Serialize)]
pub struct People {
    pub here: Vec<String>,
    pub known: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct Reassigned {
    /// The lines changed (to undo them together)
    pub lines: Vec<String>,
}

#[tauri::command]
pub async fn get_speaker_choices<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String) -> Result<People, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let (here, known) = people(&pool, &meeting_id).await.map_err(|e| e.to_string())?;
    Ok(People { here, known })
}

/// «Змінити спікера» of a line; `scope` = "line" | "following". The voice
/// is learned in the background.
#[tauri::command]
pub async fn reassign_line_speaker<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_id: String, speaker: String, scope: Scope) -> Result<Reassigned, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let label = format!("спікер: {}", speaker.trim());
    let lines = crate::speakers::journal::journaled(&pool, &meeting_id, &label, || reassign_and_learn(&app, &pool, &meeting_id, &segment_id, &speaker, scope)).await?;
    Ok(Reassigned { lines })
}

/// [`reassign`], then the voice learned in the background; the ids changed
pub async fn reassign_and_learn<R: tauri::Runtime>(app: &tauri::AppHandle<R>, pool: &SqlitePool, meeting_id: &str, segment_id: &str, speaker: &str, scope: Scope) -> Result<Vec<String>, String> {
    let changed = reassign(pool, meeting_id, segment_id, speaker, scope).await.map_err(|e| e.to_string())?;
    super::commands::refresh_speaker_terms(pool).await;
    if let Some(models) = super::commands::models_dir(app) {
        let pool = pool.clone();
        let meeting_id = meeting_id.to_string();
        let lines: Vec<(f64, f64)> = changed.iter().map(|c| (c.1, c.2)).collect();
        let who = speaker.trim().to_string();
        tauri::async_runtime::spawn(async move {
            match learn_voice(&pool, &models, &meeting_id, &who, &lines).await {
                Ok(n) if n > 0 => log::info!("Voice profile updated from {n} line(s) given by hand"),
                Ok(_) => {}
                Err(e) => log::warn!("Could not learn the voice of lines given by hand: {e}"),
            }
        });
    }
    Ok(changed.into_iter().map(|c| c.0).collect())
}

#[tauri::command]
pub async fn undo_line_speaker<R: tauri::Runtime>(app: tauri::AppHandle<R>, meeting_id: String, segment_id: String) -> Result<(), String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    crate::speakers::journal::journaled(&pool, &meeting_id, "розпізнаний спікер", || async { undo(&pool, &meeting_id, &segment_id).await.map_err(|e| e.to_string()) }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<(String, f64, f64, Option<String>)> {
        vec![
            ("a".into(), 0.0, 10.0, Some("Спікер 1".into())),
            ("b".into(), 10.0, 20.0, Some("Тарас".into())),
            ("c".into(), 20.0, 30.0, Some("Тарас".into())),
            ("d".into(), 30.0, 40.0, Some("Спікер 1".into())),
        ]
    }

    #[test]
    fn scopes() {
        assert_eq!(lines_in_scope(&rows(), "b", Scope::Line).len(), 1);
        assert_eq!(lines_in_scope(&rows(), "b", Scope::Following).iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), vec!["b", "c"]);
        assert!(lines_in_scope(&rows(), "x", Scope::Line).is_empty());
    }

    #[tokio::test]
    async fn a_line_given_to_another_person_stays_after_recognition_and_can_be_undone() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(db.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        for (id, a, b, s) in rows() {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES (?, 'm1', 'текст', 'x', ?, ?, ?)")
                .bind(id)
                .bind(a)
                .bind(b)
                .bind(s)
                .execute(&pool)
                .await
                .unwrap();
        }
        let changed = reassign(&pool, "m1", "c", "Остап", Scope::Line).await.unwrap();
        assert_eq!(changed.len(), 1);
        let who: (String, i64) = sqlx::query_as("SELECT speaker, speaker_manual FROM transcripts WHERE id = 'c'").fetch_one(&pool).await.unwrap();
        assert_eq!(who, ("Остап".to_string(), 1));
        // The new person is listed among the meeting's voices
        let listed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_speakers WHERE name = 'Остап'").fetch_one(&pool).await.unwrap();
        assert_eq!(listed, 1);
        // A new recognition writes the line again (other ids, slightly other times): the change comes back
        sqlx::query("DELETE FROM transcripts").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES ('n1', 'm1', 'текст', 'x', 19.5, 29.0, 'Спікер 4')")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(apply(&pool, "m1").await.unwrap(), 1);
        let who: String = sqlx::query_scalar("SELECT speaker FROM transcripts WHERE id = 'n1'").fetch_one(&pool).await.unwrap();
        assert_eq!(who, "Остап");
        undo(&pool, "m1", "n1").await.unwrap();
        let back: (Option<String>, i64) = sqlx::query_as("SELECT speaker, speaker_manual FROM transcripts WHERE id = 'n1'").fetch_one(&pool).await.unwrap();
        assert_eq!(back, (Some("Тарас".to_string()), 0));
        assert_eq!(apply(&pool, "m1").await.unwrap(), 0);
    }

    /// On a COPY of the app database: the line at PROTOKOL_REASSIGN_AT
    /// seconds of PROTOKOL_SPEAKERS_MEETING is given to PROTOKOL_REASSIGN_TO;
    /// a new recognition is imitated and the voice is learned
    /// (PROTOKOL_SPEAKERS_DB, PROTOKOL_SPEAKERS_MODELS)
    #[tokio::test]
    #[ignore = "needs a database copy, a recording and the models"]
    async fn reassign_on_a_database_copy() {
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        assert!(!db.contains("Application Support"), "only a copy");
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let at: f64 = std::env::var("PROTOKOL_REASSIGN_AT").unwrap().parse().unwrap();
        let to = std::env::var("PROTOKOL_REASSIGN_TO").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let rows = rows_of(&pool, &meeting).await.unwrap();
        let line = rows.iter().find(|r| r.1 <= at && at < r.2.max(r.1 + 0.5)).expect("a line at that time").clone();
        println!("line {:.1}-{:.1}s was: {:?}", line.1, line.2, line.3);
        let changed = reassign(&pool, &meeting, &line.0, &to, Scope::Line).await.unwrap();
        println!("changed {} line(s)", changed.len());
        // A new recognition: the speaker of the line is overwritten, then applied again
        sqlx::query("UPDATE transcripts SET speaker = ?, speaker_manual = 0 WHERE id = ?").bind(&line.3).bind(&line.0).execute(&pool).await.unwrap();
        println!("re-applied: {}", apply(&pool, &meeting).await.unwrap());
        let now: String = sqlx::query_scalar("SELECT speaker FROM transcripts WHERE id = ?").bind(&line.0).fetch_one(&pool).await.unwrap();
        assert_eq!(now, to);
        if let Ok(models) = std::env::var("PROTOKOL_SPEAKERS_MODELS") {
            let started = std::time::Instant::now();
            let before: Option<Vec<u8>> = sqlx::query_scalar("SELECT embedding FROM voice_profiles WHERE name = ?").bind(&to).fetch_optional(&pool).await.unwrap();
            let n = learn_voice(&pool, std::path::Path::new(&models), &meeting, &to, &[(line.1, line.2)]).await.unwrap();
            let after: Option<Vec<u8>> = sqlx::query_scalar("SELECT embedding FROM voice_profiles WHERE name = ?").bind(&to).fetch_optional(&pool).await.unwrap();
            println!("voice learned from {n} line(s) in {:.1}s; profile {}", started.elapsed().as_secs_f64(), match (&before, &after) {
                (None, _) => "absent (nothing learned)".to_string(),
                (Some(b), Some(a)) => format!("changed: {}", a != b),
                _ => "?".into(),
            });
        }
    }
}

#[cfg(test)]
mod replay_tests {
    /// His speaker fixes taught again on a COPY (PROTOKOL_SPEAKERS_DB with the
    /// fixes in PROTOKOL_FIXES_DB, PROTOKOL_SPEAKERS_MEETING, PROTOKOL_SPEAKERS_MODELS). Counts only.
    #[tokio::test]
    #[ignore = "needs database copies, a recording and the models"]
    async fn replay_hand_fixes_into_voice_profiles() {
        let db = std::env::var("PROTOKOL_SPEAKERS_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let fixes_db = std::env::var("PROTOKOL_FIXES_DB").unwrap();
        let meeting = std::env::var("PROTOKOL_SPEAKERS_MEETING").unwrap();
        let models = std::path::PathBuf::from(std::env::var("PROTOKOL_SPEAKERS_MODELS").unwrap());
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let src = sqlx::SqlitePool::connect(&format!("sqlite:{fixes_db}?mode=ro")).await.unwrap();
        let fixes: Vec<(f64, f64, String)> = sqlx::query_as("SELECT start, end, speaker FROM speaker_overrides WHERE meeting_id = ? ORDER BY id").bind(&meeting).fetch_all(&src).await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM voice_profiles").fetch_one(&pool).await.unwrap();
        let mut learned = 0;
        for (a, b, who) in &fixes {
            learned += super::learn_voice(&pool, &models, &meeting, who, &[(*a, *b)]).await.unwrap();
        }
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM voice_profiles").fetch_one(&pool).await.unwrap();
        println!("REPLAY fixes {}, lines learned {learned}, profiles {before} -> {after}", fixes.len());
    }
}
