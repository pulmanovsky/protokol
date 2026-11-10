//! «Спікери → Усі збережені»: every person the app knows, by voice or only
//! by name, and what is done with them: stop recognising the voice, stop
//! offering the name, leave one recording out of the voice, join two
//! entries of one person, forget people (with «Скасувати»).
//!
//! Recordings already made are never changed here: their transcripts and
//! documents keep the names they have.

use anyhow::{anyhow, Result};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

use super::embedding::cosine;
use super::profiles::{from_blob, to_blob};

/// Two saved voices this alike are offered as one person
const SAME_VOICE: f32 = 0.70;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedSpeaker {
    pub name: String,
    pub role: Option<String>,
    /// A saved voice exists
    pub has_voice: bool,
    /// The voice names this person in new recordings
    pub recognize: bool,
    /// The name is offered in hints and in «Хто говорив?»
    pub suggest: bool,
    /// Recordings the voice was gathered from
    pub voice_meetings: i64,
    /// Recordings where the name appears
    pub meetings: i64,
    /// The latest of those recordings (its date)
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SamePerson {
    pub keep: String,
    pub merge: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSpeakers {
    pub people: Vec<SavedSpeaker>,
    pub same: Vec<SamePerson>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSample {
    pub meeting_id: String,
    pub title: String,
    pub date: String,
    /// The recording's sound file, when it is still there
    pub audio_path: Option<String>,
    pub clip_start: Option<f64>,
    pub clip_end: Option<f64>,
    /// Left out of the voice («Це не цей голос»)
    pub excluded: bool,
}

/// Not a person: an unnamed voice, «Я», short remarks, an unconfirmed name
fn is_person(name: &str) -> bool {
    let n = name.trim();
    !(n.is_empty() || n.starts_with("Спікер") || n == super::commands::ME_LABEL || n == super::commands::REMARKS || n.ends_with('?'))
}

/// A name given by hand again is offered again
pub async fn unhide(pool: &SqlitePool, name: &str) -> Result<()> {
    sqlx::query("DELETE FROM hidden_speaker_names WHERE name = ?").bind(name.trim()).execute(pool).await?;
    Ok(())
}

async fn hide(pool: &SqlitePool, name: &str, forgotten: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO hidden_speaker_names (name, forgotten, hidden_at) VALUES (?, ?, ?)
         ON CONFLICT(name) DO UPDATE SET forgotten = MAX(forgotten, excluded.forgotten)",
    )
    .bind(name)
    .bind(forgotten as i64)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Words of a name that tell people apart («Коваль А.» → «коваль»)
fn name_words(name: &str) -> Vec<String> {
    name.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-').to_lowercase())
        .filter(|w| w.chars().count() >= 3)
        .collect()
}

/// Pairs that look like one person: a shared surname or name, or voices this alike
pub(crate) fn same_people(people: &[SavedSpeaker], voices: &HashMap<String, Vec<f32>>) -> Vec<SamePerson> {
    let mut out = Vec::new();
    for (i, a) in people.iter().enumerate() {
        for b in &people[i + 1..] {
            let (wa, wb) = (name_words(&a.name), name_words(&b.name));
            let shared = wa.iter().any(|w| wb.contains(w));
            let alike = match (voices.get(&a.name), voices.get(&b.name)) {
                (Some(x), Some(y)) if x.len() == y.len() && !x.is_empty() => cosine(x, y) >= SAME_VOICE,
                _ => false,
            };
            if shared || alike {
                // The one heard in more recordings keeps its name
                let (keep, merge) = if (b.meetings, b.voice_meetings) > (a.meetings, a.voice_meetings) { (b, a) } else { (a, b) };
                out.push(SamePerson { keep: keep.name.clone(), merge: merge.name.clone() });
            }
        }
    }
    out.truncate(5);
    out
}

pub async fn list(pool: &SqlitePool) -> Result<SavedSpeakers> {
    let profiles = sqlx::query("SELECT name, embedding, meetings, role, recognize FROM voice_profiles").fetch_all(pool).await?;
    let hidden: HashMap<String, bool> = sqlx::query_as::<_, (String, i64)>("SELECT name, forgotten FROM hidden_speaker_names")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(n, f)| (n, f != 0))
        .collect();
    // Where each name appears: its voice in a recording, or lines given to it
    let seen: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT who, COUNT(DISTINCT x.meeting_id), MAX(m.created_at) FROM (
             SELECT name AS who, meeting_id FROM meeting_speakers WHERE name IS NOT NULL
             UNION SELECT speaker AS who, meeting_id FROM transcripts WHERE speaker IS NOT NULL
         ) x JOIN meetings m ON m.id = x.meeting_id GROUP BY who",
    )
    .fetch_all(pool)
    .await?;
    let seen: HashMap<String, (i64, Option<String>)> = seen.into_iter().map(|(n, c, d)| (n, (c, d))).collect();
    let roles: HashMap<String, String> = sqlx::query_as::<_, (String, String)>(
        "SELECT name, role FROM meeting_speakers WHERE name IS NOT NULL AND role IS NOT NULL AND TRIM(role) <> ''",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();

    let mut people: HashMap<String, SavedSpeaker> = HashMap::new();
    let mut voices: HashMap<String, Vec<f32>> = HashMap::new();
    for r in &profiles {
        let name: String = r.get("name");
        if !is_person(&name) {
            continue;
        }
        voices.insert(name.clone(), from_blob(r.get::<Vec<u8>, _>("embedding").as_slice()));
        people.insert(
            name.clone(),
            SavedSpeaker {
                role: r.get::<Option<String>, _>("role").or_else(|| roles.get(&name).cloned()),
                has_voice: true,
                recognize: r.get::<i64, _>("recognize") != 0,
                suggest: true,
                voice_meetings: r.get("meetings"),
                meetings: 0,
                last_seen: None,
                name,
            },
        );
    }
    // Names known from earlier recordings only (what the hints offer)
    let named: Vec<String> = sqlx::query_scalar("SELECT DISTINCT name FROM meeting_speakers WHERE name IS NOT NULL").fetch_all(pool).await?;
    for name in named {
        let name = name.trim().to_string();
        if !is_person(&name) || people.contains_key(&name) {
            continue;
        }
        people.insert(
            name.clone(),
            SavedSpeaker { role: roles.get(&name).cloned(), has_voice: false, recognize: false, suggest: true, voice_meetings: 0, meetings: 0, last_seen: None, name },
        );
    }
    let mut people: Vec<SavedSpeaker> = people
        .into_values()
        .filter(|p| hidden.get(&p.name) != Some(&true))
        .map(|mut p| {
            p.suggest = !hidden.contains_key(&p.name);
            if let Some((c, d)) = seen.get(&p.name) {
                p.meetings = *c;
                p.last_seen = d.clone();
            }
            p.meetings = p.meetings.max(p.voice_meetings.min(1));
            p
        })
        .collect();
    people.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let same = same_people(&people, &voices);
    Ok(SavedSpeakers { people, same })
}

/// Recordings with this person's voice, newest first, each with a short clip
pub async fn samples(pool: &SqlitePool, name: &str) -> Result<Vec<VoiceSample>> {
    let rows = sqlx::query(
        "SELECT DISTINCT m.id, m.title, m.created_at, m.folder_path FROM meeting_speakers s JOIN meetings m ON m.id = s.meeting_id
         WHERE s.name = ? ORDER BY m.created_at DESC LIMIT 12",
    )
    .bind(name)
    .fetch_all(pool)
    .await?;
    let excluded: Vec<String> = sqlx::query_scalar("SELECT meeting_id FROM voice_profile_exclusions WHERE name = ?").bind(name).fetch_all(pool).await?;
    let mut out = Vec::new();
    for r in rows {
        let id: String = r.get("id");
        let voice = super::commands::speakers_of(pool, &id).await?.into_iter().find(|s| s.name.as_deref() == Some(name));
        let folder: Option<String> = r.get("folder_path");
        let audio_path = folder.and_then(|f| crate::audio::retranscription::find_audio_file(std::path::Path::new(&f)).ok()).map(|p| p.to_string_lossy().to_string());
        out.push(VoiceSample {
            excluded: excluded.contains(&id),
            title: r.get("title"),
            date: r.get("created_at"),
            clip_start: voice.as_ref().and_then(|v| v.clip_start),
            clip_end: voice.as_ref().and_then(|v| v.clip_end),
            audio_path,
            meeting_id: id,
        });
    }
    Ok(out)
}

/// The voice made again from its recordings, without the ones left out.
/// No recording left: the voice is removed, the name stays.
pub async fn rebuild_voice(pool: &SqlitePool, name: &str) -> Result<()> {
    let profile = sqlx::query("SELECT embedding, model FROM voice_profiles WHERE name = ?").bind(name).fetch_optional(pool).await?;
    let centroids: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT centroid FROM meeting_speakers WHERE name = ? AND meeting_id NOT IN (SELECT meeting_id FROM voice_profile_exclusions WHERE name = ?)",
    )
    .bind(name)
    .bind(name)
    .fetch_all(pool)
    .await?;
    let dim = profile.as_ref().map(|p| from_blob(p.get::<Vec<u8>, _>("embedding").as_slice()).len());
    let vectors: Vec<Vec<f32>> = centroids.iter().map(|c| from_blob(c)).filter(|v| !v.is_empty() && dim.map_or(true, |d| v.len() == d)).collect();
    if vectors.is_empty() {
        sqlx::query("DELETE FROM voice_profiles WHERE name = ?").bind(name).execute(pool).await?;
        return Ok(());
    }
    let mut mean = vec![0f32; vectors[0].len()];
    for v in &vectors {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        mean.iter_mut().zip(v).for_each(|(m, x)| *m += x / n);
    }
    let n = mean.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    mean.iter_mut().for_each(|x| *x /= n);
    let now = chrono::Utc::now().to_rfc3339();
    if profile.is_some() {
        sqlx::query("UPDATE voice_profiles SET embedding = ?, meetings = ?, updated_at = ? WHERE name = ?")
            .bind(to_blob(&mean))
            .bind(vectors.len() as i64)
            .bind(&now)
            .bind(name)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// «Це не цей голос»: one recording left out of a person's voice, or back in
pub async fn set_excluded(pool: &SqlitePool, name: &str, meeting_id: &str, excluded: bool) -> Result<()> {
    if excluded {
        sqlx::query("INSERT OR IGNORE INTO voice_profile_exclusions (name, meeting_id) VALUES (?, ?)").bind(name).bind(meeting_id).execute(pool).await?;
    } else {
        sqlx::query("DELETE FROM voice_profile_exclusions WHERE name = ? AND meeting_id = ?").bind(name).bind(meeting_id).execute(pool).await?;
    }
    rebuild_voice(pool, name).await
}

/// «Впізнавати за голосом» and «Пропонувати ім'я»
pub async fn set_flags(pool: &SqlitePool, name: &str, recognize: Option<bool>, suggest: Option<bool>) -> Result<()> {
    if let Some(on) = recognize {
        sqlx::query("UPDATE voice_profiles SET recognize = ? WHERE name = ?").bind(on as i64).bind(name).execute(pool).await?;
    }
    match suggest {
        Some(true) => unhide(pool, name).await?,
        Some(false) => hide(pool, name, false).await?,
        None => {}
    }
    Ok(())
}

/// The role of a person: for new recordings (old ones keep theirs)
pub async fn set_role(pool: &SqlitePool, name: &str, role: &str) -> Result<()> {
    let role = role.trim();
    sqlx::query("UPDATE voice_profiles SET role = ? WHERE name = ?").bind((!role.is_empty()).then_some(role)).bind(name).execute(pool).await?;
    Ok(())
}

/// Two entries of one person become `keep`: the voices joined (weighted by
/// recordings), `merge` no longer listed or offered. Old recordings keep
/// the name they have. A new name for one person is the same step.
pub async fn merge(pool: &SqlitePool, keep: &str, merge: &str) -> Result<()> {
    let (keep, merge) = (keep.trim(), merge.trim());
    if keep.is_empty() || keep == merge {
        return Err(anyhow!("Вкажіть інше ім'я"));
    }
    let row = |n: &str| sqlx::query("SELECT embedding, model, meetings, role, recognize FROM voice_profiles WHERE name = ?").bind(n.to_string());
    let a = row(keep).fetch_optional(pool).await?;
    let b = row(merge).fetch_optional(pool).await?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    match (a, b) {
        (Some(a), Some(b)) => {
            let (va, vb) = (from_blob(a.get::<Vec<u8>, _>("embedding").as_slice()), from_blob(b.get::<Vec<u8>, _>("embedding").as_slice()));
            let (na, nb): (i64, i64) = (a.get("meetings"), b.get("meetings"));
            let joined = if va.len() == vb.len() {
                let mut v: Vec<f32> = va.iter().zip(&vb).map(|(x, y)| x * na.max(1) as f32 + y * nb.max(1) as f32).collect();
                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                v.iter_mut().for_each(|x| *x /= n);
                v
            } else {
                va
            };
            let role: Option<String> = a.get::<Option<String>, _>("role").or_else(|| b.get("role"));
            sqlx::query("UPDATE voice_profiles SET embedding = ?, meetings = ?, role = ?, updated_at = ? WHERE name = ?")
                .bind(to_blob(&joined))
                .bind(na + nb)
                .bind(role)
                .bind(&now)
                .bind(keep)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM voice_profiles WHERE name = ?").bind(merge).execute(&mut *tx).await?;
        }
        (None, Some(_)) => {
            sqlx::query("UPDATE voice_profiles SET name = ?, updated_at = ? WHERE name = ?").bind(keep).bind(&now).bind(merge).execute(&mut *tx).await?;
        }
        _ => {}
    }
    sqlx::query("UPDATE OR IGNORE voice_profile_exclusions SET name = ? WHERE name = ?").bind(keep).bind(merge).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM voice_profile_exclusions WHERE name = ?").bind(merge).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM hidden_speaker_names WHERE name = ?").bind(keep).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO hidden_speaker_names (name, forgotten, hidden_at) VALUES (?, 1, ?) ON CONFLICT(name) DO UPDATE SET forgotten = 1",
    )
    .bind(merge)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// People forgotten: voices removed, names not listed or offered any more.
/// Returns the batch that «Скасувати» brings back.
pub async fn forget(pool: &SqlitePool, names: &[String]) -> Result<String> {
    let batch = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    // Only the last «Забути» can be taken back
    sqlx::query("DELETE FROM forgotten_speakers").execute(&mut *tx).await?;
    for name in names {
        let hidden: Option<i64> = sqlx::query_scalar("SELECT forgotten FROM hidden_speaker_names WHERE name = ?").bind(name).fetch_optional(&mut *tx).await?;
        let p = sqlx::query("SELECT embedding, model, meetings, updated_at, role, recognize FROM voice_profiles WHERE name = ?").bind(name).fetch_optional(&mut *tx).await?;
        sqlx::query(
            "INSERT OR REPLACE INTO forgotten_speakers (batch, name, embedding, model, meetings, updated_at, role, recognize, hidden_before, forgotten_before)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&batch)
        .bind(name)
        .bind(p.as_ref().map(|p| p.get::<Vec<u8>, _>("embedding")))
        .bind(p.as_ref().map(|p| p.get::<String, _>("model")))
        .bind(p.as_ref().map(|p| p.get::<i64, _>("meetings")))
        .bind(p.as_ref().map(|p| p.get::<String, _>("updated_at")))
        .bind(p.as_ref().and_then(|p| p.get::<Option<String>, _>("role")))
        .bind(p.as_ref().map(|p| p.get::<i64, _>("recognize")))
        .bind(hidden.is_some() as i64)
        .bind(hidden.unwrap_or(0))
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM voice_profiles WHERE name = ?").bind(name).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO hidden_speaker_names (name, forgotten, hidden_at) VALUES (?, 1, ?) ON CONFLICT(name) DO UPDATE SET forgotten = 1",
        )
        .bind(name)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(batch)
}

/// «Скасувати» after «Забути»: everyone of the batch as they were
pub async fn restore(pool: &SqlitePool, batch: &str) -> Result<usize> {
    let rows = sqlx::query("SELECT * FROM forgotten_speakers WHERE batch = ?").bind(batch).fetch_all(pool).await?;
    let mut tx = pool.begin().await?;
    for r in &rows {
        let name: String = r.get("name");
        if let Some(embedding) = r.get::<Option<Vec<u8>>, _>("embedding") {
            sqlx::query(
                "INSERT OR REPLACE INTO voice_profiles (name, embedding, model, meetings, updated_at, role, recognize) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&name)
            .bind(embedding)
            .bind(r.get::<Option<String>, _>("model").unwrap_or_default())
            .bind(r.get::<Option<i64>, _>("meetings").unwrap_or(1))
            .bind(r.get::<Option<String>, _>("updated_at").unwrap_or_default())
            .bind(r.get::<Option<String>, _>("role"))
            .bind(r.get::<Option<i64>, _>("recognize").unwrap_or(1))
            .execute(&mut *tx)
            .await?;
        }
        if r.get::<i64, _>("hidden_before") != 0 {
            sqlx::query("UPDATE hidden_speaker_names SET forgotten = ? WHERE name = ?").bind(r.get::<i64, _>("forgotten_before")).bind(&name).execute(&mut *tx).await?;
        } else {
            sqlx::query("DELETE FROM hidden_speaker_names WHERE name = ?").bind(&name).execute(&mut *tx).await?;
        }
    }
    sqlx::query("DELETE FROM forgotten_speakers WHERE batch = ?").bind(batch).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(rows.len())
}

// ── Commands ──

#[tauri::command]
pub async fn get_saved_speakers<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<SavedSpeakers, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    list(&pool).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_saved_speaker_samples<R: tauri::Runtime>(app: tauri::AppHandle<R>, name: String) -> Result<Vec<VoiceSample>, String> {
    use tauri::Manager;
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let found = samples(&pool, &name).await.map_err(|e| e.to_string())?;
    // The sound files may be played from the page
    for s in &found {
        if let Some(p) = &s.audio_path {
            let _ = app.asset_protocol_scope().allow_file(p);
        }
    }
    Ok(found)
}

#[tauri::command]
pub async fn set_saved_speaker<R: tauri::Runtime>(app: tauri::AppHandle<R>, name: String, recognize: Option<bool>, suggest: Option<bool>, role: Option<String>) -> Result<(), String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    set_flags(&pool, &name, recognize, suggest).await.map_err(|e| e.to_string())?;
    if let Some(role) = role {
        set_role(&pool, &name, &role).await.map_err(|e| e.to_string())?;
    }
    super::commands::refresh_speaker_terms(&pool).await;
    Ok(())
}

#[tauri::command]
pub async fn set_voice_sample_excluded<R: tauri::Runtime>(app: tauri::AppHandle<R>, name: String, meeting_id: String, excluded: bool) -> Result<(), String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    set_excluded(&pool, &name, &meeting_id, excluded).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn merge_saved_speakers<R: tauri::Runtime>(app: tauri::AppHandle<R>, keep: String, merge: String) -> Result<(), String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    self::merge(&pool, &keep, &merge).await.map_err(|e| e.to_string())?;
    super::commands::refresh_speaker_terms(&pool).await;
    Ok(())
}

#[tauri::command]
pub async fn forget_saved_speakers<R: tauri::Runtime>(app: tauri::AppHandle<R>, names: Vec<String>) -> Result<String, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let batch = forget(&pool, &names).await.map_err(|e| e.to_string())?;
    super::commands::refresh_speaker_terms(&pool).await;
    Ok(batch)
}

#[tauri::command]
pub async fn restore_forgotten_speakers<R: tauri::Runtime>(app: tauri::AppHandle<R>, batch: String) -> Result<usize, String> {
    let pool = super::commands::pool(&app).map_err(|e| e.to_string())?;
    let n = restore(&pool, &batch).await.map_err(|e| e.to_string())?;
    super::commands::refresh_speaker_terms(&pool).await;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(path.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        for (id, day) in [("m1", "01"), ("m2", "02"), ("m3", "03")] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'Нарада', ?, ?)")
                .bind(id)
                .bind(format!("2026-11-{day}T08:00:00Z"))
                .bind(format!("2026-11-{day}T08:00:00Z"))
                .execute(&pool)
                .await
                .unwrap();
        }
        let voice = |v: [f32; 3]| to_blob(&v);
        for (m, label, name, c) in [
            ("m1", "Спікер 1", Some("Олена"), voice([1.0, 0.0, 0.0])),
            ("m2", "Спікер 1", Some("Олена"), voice([0.0, 1.0, 0.0])),
            ("m3", "Спікер 1", Some("Олена"), voice([1.0, 0.0, 0.0])),
            ("m1", "Спікер 2", Some("ведучий"), voice([0.0, 0.0, 1.0])),
            ("m2", "Спікер 2", Some("Коваль А."), voice([0.0, 0.0, 1.0])),
            ("m3", "Спікер 2", None, voice([0.0, 0.0, 1.0])),
        ] {
            sqlx::query("INSERT INTO meeting_speakers (meeting_id, label, name, centroid, speech_seconds) VALUES (?, ?, ?, ?, 30)")
                .bind(m)
                .bind(label)
                .bind(name)
                .bind(c)
                .execute(&pool)
                .await
                .unwrap();
        }
        for (name, v, n) in [("Олена", voice([0.8, 0.6, 0.0]), 3), ("Андрій Коваль", voice([0.0, 0.1, 0.99]), 4)] {
            sqlx::query("INSERT INTO voice_profiles (name, embedding, model, meetings, updated_at) VALUES (?, ?, 'test', ?, 'x')").bind(name).bind(v).bind(n).execute(&pool).await.unwrap();
        }
        (dir, pool)
    }

    fn names(s: &SavedSpeakers) -> Vec<&str> {
        s.people.iter().map(|p| p.name.as_str()).collect()
    }

    #[tokio::test]
    async fn everyone_known_is_listed_and_one_person_twice_is_noticed() {
        let (_d, pool) = db().await;
        let s = list(&pool).await.unwrap();
        assert_eq!(names(&s), vec!["Андрій Коваль", "ведучий", "Коваль А.", "Олена"]);
        let olena = s.people.iter().find(|p| p.name == "Олена").unwrap();
        assert!(olena.has_voice && olena.recognize && olena.suggest);
        assert_eq!(olena.meetings, 3);
        assert_eq!(olena.last_seen.as_deref(), Some("2026-11-03T08:00:00Z"));
        let host = s.people.iter().find(|p| p.name == "ведучий").unwrap();
        assert!(!host.has_voice);
        assert_eq!(host.meetings, 1);
        assert_eq!(s.same, vec![SamePerson { keep: "Андрій Коваль".into(), merge: "Коваль А.".into() }]);
    }

    #[tokio::test]
    async fn forgotten_people_are_not_offered_or_recognised_and_come_back_on_undo() {
        let (_d, pool) = db().await;
        let batch = forget(&pool, &["ведучий".into(), "Олена".into()]).await.unwrap();
        assert_eq!(names(&list(&pool).await.unwrap()), vec!["Андрій Коваль", "Коваль А."]);
        let (_, known) = super::super::overrides::people(&pool, "m3").await.unwrap();
        assert!(!known.contains(&"ведучий".to_string()) && !known.contains(&"Олена".to_string()));
        assert!(super::super::commands::profiles(&pool, "test").await.iter().all(|p| p.name != "Олена"));
        // Old recordings keep the name
        let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_speakers WHERE name = 'ведучий'").fetch_one(&pool).await.unwrap();
        assert_eq!(kept, 1);
        assert_eq!(restore(&pool, &batch).await.unwrap(), 2);
        assert_eq!(names(&list(&pool).await.unwrap()), vec!["Андрій Коваль", "ведучий", "Коваль А.", "Олена"]);
        assert_eq!(super::super::commands::profiles(&pool, "test").await.iter().filter(|p| p.name == "Олена").count(), 1);
    }

    #[tokio::test]
    async fn switches_stop_recognising_and_offering_and_a_name_given_again_is_offered() {
        let (_d, pool) = db().await;
        set_flags(&pool, "Олена", Some(false), Some(false)).await.unwrap();
        assert!(super::super::commands::profiles(&pool, "test").await.iter().all(|p| p.name != "Олена"));
        let (_, known) = super::super::overrides::people(&pool, "m3").await.unwrap();
        assert!(!known.contains(&"Олена".to_string()));
        // Still listed, with both switches off
        let olena = list(&pool).await.unwrap().people.into_iter().find(|p| p.name == "Олена").unwrap();
        assert!(!olena.recognize && !olena.suggest);
        unhide(&pool, "Олена").await.unwrap();
        let (_, known) = super::super::overrides::people(&pool, "m3").await.unwrap();
        assert!(known.contains(&"Олена".to_string()));
    }

    #[tokio::test]
    async fn a_recording_left_out_rebuilds_the_voice_from_the_others() {
        let (_d, pool) = db().await;
        set_excluded(&pool, "Олена", "m2", true).await.unwrap();
        let (v, n): (Vec<u8>, i64) = sqlx::query_as("SELECT embedding, meetings FROM voice_profiles WHERE name = 'Олена'").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 2);
        let v = from_blob(&v);
        assert!((v[0] - 1.0).abs() < 1e-5 && v[1].abs() < 1e-5);
        assert!(samples(&pool, "Олена").await.unwrap().iter().any(|s| s.meeting_id == "m2" && s.excluded));
        set_excluded(&pool, "Олена", "m2", false).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT meetings FROM voice_profiles WHERE name = 'Олена'").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 3);
    }

    #[tokio::test]
    async fn two_entries_of_one_person_become_one() {
        let (_d, pool) = db().await;
        merge(&pool, "Андрій Коваль", "Коваль А.").await.unwrap();
        assert_eq!(names(&list(&pool).await.unwrap()), vec!["Андрій Коваль", "ведучий", "Олена"]);
        // A new name for a voice: the same step
        merge(&pool, "Олена Ткач", "Олена").await.unwrap();
        let s = list(&pool).await.unwrap();
        let renamed = s.people.iter().find(|p| p.name == "Олена Ткач").unwrap();
        assert!(renamed.has_voice);
        assert_eq!(renamed.voice_meetings, 3);
        assert!(!names(&s).contains(&"Олена"));
    }
}
