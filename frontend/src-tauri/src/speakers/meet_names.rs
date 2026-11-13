//! Google Meet accounts and the saved people behind them.
//!
//! A meeting's voice that came from a Meet participant keeps the account's
//! name (`meeting_speakers.meet_name`). Once the secretary confirms who it is
//! (by naming the voice in «Спікери», by «Прив'язати позначені», or by hand in
//! Налаштування → Пам'ять), the account is linked to that person
//! (`meet_accounts`) and later meetings name it at once. An account several
//! people speak from (a shared device) is not linked to anyone.

use std::collections::HashMap;

use anyhow::Result;
use log::info;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Runtime};

use super::commands::{model_id, models_dir, pool, profiles, rename_with, ME_LABEL};
use super::embedding::cosine;
use super::profiles::{from_blob, VoiceProfile, NAME_SIMILARITY, UNSURE_SIMILARITY};

/// Where a voice's shown name came from
pub const VOICE: &str = "voice";
pub const LINK: &str = "meet_link";
pub const LINK_DOUBT: &str = "meet_link_doubt";
pub const SAME_NAME: &str = "meet_same_name";
pub const MEET: &str = "meet";

/// Names for the voices of a meeting that came from Meet participants.
/// `known`: names found by voice (changed in place). Returns where each name
/// came from, and the names offered for confirmation («Name (?)»).
pub async fn resolve(
    pool: &SqlitePool,
    known: &mut [Option<String>],
    meet_names: &[Option<String>],
    shared: &[bool],
    centroids: &[Vec<f32>],
    profiles: &[VoiceProfile],
) -> (HashMap<usize, &'static str>, HashMap<usize, String>) {
    let links: HashMap<String, String> = sqlx::query_as::<_, (String, String)>("SELECT meet_name, person FROM meet_accounts WHERE person IS NOT NULL AND shared = 0")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        // The name typed in a person's card may differ in case from the tile
        .map(|(m, p)| (m.to_lowercase(), p))
        .collect();
    let mut source: HashMap<usize, &'static str> = HashMap::new();
    let mut suggested: HashMap<usize, String> = HashMap::new();
    for k in 0..known.len() {
        let meet = meet_names.get(k).cloned().flatten();
        let by_voice = known[k].is_some();
        let Some(m) = meet else {
            if by_voice {
                source.insert(k, VOICE);
            }
            continue;
        };
        if shared.get(k).copied().unwrap_or(false) {
            // Several people speak from this account: only their voices name them
            if by_voice {
                source.insert(k, VOICE);
            }
            continue;
        }
        if let Some(person) = links.get(&m.to_lowercase()) {
            let centroid = centroids.get(k).map(|c| c.as_slice()).unwrap_or(&[]);
            let similarity = profiles.iter().find(|p| &p.name == person).filter(|p| !centroid.is_empty() && p.embedding.len() == centroid.len()).map(|p| cosine(centroid, &p.embedding));
            if similarity.is_some_and(|s| s < UNSURE_SIMILARITY) {
                // Someone else at that device: the name waits for confirmation
                known[k] = None;
                suggested.insert(k, person.clone());
                source.insert(k, LINK_DOUBT);
            } else {
                known[k] = Some(person.clone());
                source.insert(k, LINK);
            }
            continue;
        }
        match known[k].as_deref() {
            Some(n) if !n.ends_with('?') => {
                source.insert(k, VOICE);
            }
            _ => {
                // A new account: the name on its tile, or the saved person
                // of that name, offered until confirmed
                known[k] = None;
                let same = profiles.iter().find(|p| p.name.to_lowercase() == m.to_lowercase());
                suggested.insert(k, same.map(|p| p.name.clone()).unwrap_or(m));
                source.insert(k, if same.is_some() { SAME_NAME } else { MEET });
            }
        }
    }
    (source, suggested)
}

/// Remembers the accounts a recording heard
pub async fn seen(pool: &SqlitePool, meet_names: &[Option<String>], shared: &[bool]) {
    let mut accounts: HashMap<String, bool> = HashMap::new();
    for (k, m) in meet_names.iter().enumerate() {
        if let Some(m) = m {
            *accounts.entry(m.clone()).or_default() |= shared.get(k).copied().unwrap_or(false);
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    for (m, sh) in accounts {
        let _ = sqlx::query(
            "INSERT INTO meet_accounts (meet_name, shared, last_seen) VALUES (?, ?, ?)
             ON CONFLICT(meet_name) DO UPDATE SET last_seen = excluded.last_seen, shared = MAX(shared, excluded.shared)",
        )
        .bind(&m)
        .bind(sh as i64)
        .bind(&now)
        .execute(pool)
        .await;
    }
}

/// Links an account to a person (None: unlinks it)
pub async fn link(pool: &SqlitePool, meet_name: &str, person: Option<&str>) -> Result<()> {
    let meet_name = meet_name.trim();
    if meet_name.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO meet_accounts (meet_name, person, shared, linked_at) VALUES (?, ?, 0, ?)
         ON CONFLICT(meet_name) DO UPDATE SET person = excluded.person, linked_at = excluded.linked_at, shared = CASE WHEN excluded.person IS NULL THEN shared ELSE 0 END",
    )
    .bind(meet_name)
    .bind(person)
    .bind(person.map(|_| now))
    .execute(pool)
    .await?;
    info!("Meet account {}", if person.is_some() { "linked to a person" } else { "unlinked" });
    Ok(())
}

/// A meeting's voice was named: its Meet account (if one person's) is that person now
pub async fn on_named(pool: &SqlitePool, meeting_id: &str, label: &str, name: &str) {
    if label == ME_LABEL || name == ME_LABEL || name.ends_with('?') {
        return;
    }
    let meet: Option<String> = sqlx::query_scalar("SELECT meet_name FROM meeting_speakers WHERE meeting_id = ? AND label = ?")
        .bind(meeting_id)
        .bind(label)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten();
    let Some(meet) = meet else { return };
    let shared: Option<i64> = sqlx::query_scalar("SELECT shared FROM meet_accounts WHERE meet_name = ?").bind(&meet).fetch_optional(pool).await.ok().flatten();
    if shared == Some(1) {
        return;
    }
    if link(pool, &meet, Some(name)).await.is_ok() {
        let _ = sqlx::query("UPDATE meeting_speakers SET name_source = ? WHERE meeting_id = ? AND label = ?").bind(LINK).bind(meeting_id).bind(label).execute(pool).await;
    }
}

/// People renamed or joined in «Збережені люди»: their accounts follow
pub async fn person_renamed(pool: &SqlitePool, from: &str, to: &str) {
    let _ = sqlx::query("UPDATE meet_accounts SET person = ? WHERE person = ?").bind(to).bind(from).execute(pool).await;
}

/// People forgotten: their accounts are no one's again
pub async fn people_forgotten(pool: &SqlitePool, names: &[String]) {
    for n in names {
        let _ = sqlx::query("UPDATE meet_accounts SET person = NULL, linked_at = NULL WHERE person = ?").bind(n).execute(pool).await;
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetAccount {
    pub meet_name: String,
    pub person: Option<String>,
    pub shared: bool,
    pub linked_at: Option<String>,
    pub last_seen: Option<String>,
    /// Not linked yet: the saved person whose voice the account's last voice
    /// is like (None: no one in memory is like it)
    pub voice_like: Option<String>,
}

pub async fn accounts(pool: &SqlitePool) -> Result<Vec<MeetAccount>> {
    let rows = sqlx::query("SELECT meet_name, person, shared, linked_at, last_seen FROM meet_accounts ORDER BY person IS NULL, LOWER(meet_name)").fetch_all(pool).await?;
    Ok(rows
        .iter()
        .map(|r| MeetAccount {
            meet_name: r.get("meet_name"),
            person: r.get("person"),
            shared: r.get::<i64, _>("shared") != 0,
            linked_at: r.get("linked_at"),
            last_seen: r.get("last_seen"),
            voice_like: None,
        })
        .collect())
}

/// The accounts, with a hint for those not linked yet: whose voice the
/// account's voice in its latest meeting is like
pub async fn accounts_with_hints(pool: &SqlitePool, model: &str) -> Result<Vec<MeetAccount>> {
    let mut list = accounts(pool).await?;
    if list.iter().all(|a| a.person.is_some() || a.shared) {
        return Ok(list);
    }
    let people = profiles(pool, model).await;
    for a in list.iter_mut().filter(|a| a.person.is_none() && !a.shared) {
        let blob: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT s.centroid FROM meeting_speakers s JOIN meetings m ON m.id = s.meeting_id
             WHERE s.meet_name = ? ORDER BY m.created_at DESC LIMIT 1",
        )
        .bind(&a.meet_name)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        let centroid = blob.map(|b| from_blob(&b)).unwrap_or_default();
        a.voice_like = closest(&centroid, &people).into_iter().next().filter(|(_, s)| *s >= UNSURE_SIMILARITY).map(|(n, _)| n);
    }
    Ok(list)
}

/// One row of «Прив'язати облікові записи Google Meet до людей у пам'яті»
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkCandidate {
    pub meet_name: String,
    /// The voice's label in this meeting and the name it is shown with
    pub label: String,
    pub shown: String,
    pub speech_seconds: f64,
    /// The person offered
    pub person: Option<String>,
    /// voice (голос збігся), name (ім'я і голос збіглися), maybe (голос
    /// схожий), none (збігу немає), shared (кілька людей)
    pub reason: String,
    /// Ticked at first
    pub checked: bool,
}

/// Saved people closest to a voice, most similar first
pub fn closest(centroid: &[f32], profiles: &[VoiceProfile]) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = profiles
        .iter()
        .filter(|p| !centroid.is_empty() && p.embedding.len() == centroid.len())
        .map(|p| (p.name.clone(), cosine(centroid, &p.embedding)))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// The offer for one account: a sure voice, the same name (voice not
/// against it), a similar voice, or no one
pub fn offer(meet_name: &str, centroid: &[f32], profiles: &[VoiceProfile]) -> (Option<String>, &'static str, bool) {
    let near = closest(centroid, profiles);
    let same = profiles.iter().find(|p| p.name.to_lowercase() == meet_name.to_lowercase());
    let sim_of = |name: &str| near.iter().find(|(n, _)| n == name).map(|(_, s)| *s);
    if let Some((best, s)) = near.first() {
        if *s >= NAME_SIMILARITY {
            let both = same.is_some_and(|p| &p.name == best);
            return (Some(best.clone()), if both { "name" } else { "voice" }, true);
        }
    }
    if let Some(p) = same {
        return match sim_of(&p.name) {
            Some(s) if s < UNSURE_SIMILARITY => (Some(p.name.clone()), "maybe", false),
            _ => (Some(p.name.clone()), "name", true),
        };
    }
    match near.first() {
        Some((best, s)) if *s >= UNSURE_SIMILARITY => (Some(best.clone()), "maybe", false),
        _ => (None, "none", false),
    }
}

pub async fn link_candidates(pool: &SqlitePool, model: &str, meeting_id: &str) -> Result<Vec<LinkCandidate>> {
    let people = profiles(pool, model).await;
    let rows = sqlx::query(
        "SELECT s.label, s.name, s.suggested, s.centroid, s.speech_seconds, s.meet_name, a.person, COALESCE(a.shared, 0) AS shared
         FROM meeting_speakers s LEFT JOIN meet_accounts a ON a.meet_name = s.meet_name
         WHERE s.meeting_id = ? AND s.meet_name IS NOT NULL ORDER BY s.speech_seconds DESC",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    let mut listed: Vec<String> = Vec::new();
    for r in rows {
        let meet_name: String = r.get("meet_name");
        let person: Option<String> = r.get("person");
        if person.is_some() || listed.contains(&meet_name) {
            continue;
        }
        listed.push(meet_name.clone());
        let label: String = r.get("label");
        let name: Option<String> = r.get("name");
        let suggested: Option<String> = r.get("suggested");
        let shown = name.clone().or_else(|| suggested.as_deref().map(super::names::unconfirmed)).unwrap_or_else(|| label.clone());
        let speech_seconds: f64 = r.get::<Option<f64>, _>("speech_seconds").unwrap_or(0.0);
        if r.get::<i64, _>("shared") != 0 {
            out.push(LinkCandidate { meet_name, label, shown, speech_seconds, person: None, reason: "shared".into(), checked: false });
            continue;
        }
        let centroid = from_blob(r.get::<Vec<u8>, _>("centroid").as_slice());
        let (person, reason, checked) = offer(&meet_name, &centroid, &people);
        out.push(LinkCandidate { meet_name, label, shown, speech_seconds, person, reason: reason.into(), checked });
    }
    Ok(out)
}

/// Remembers one more meeting of a person's voice (as naming a voice does)
async fn remember_voice(pool: &SqlitePool, model: &str, meeting_id: &str, label: &str, person: &str) -> Result<()> {
    let centroid: Option<Vec<u8>> = sqlx::query_scalar("SELECT centroid FROM meeting_speakers WHERE meeting_id = ? AND label = ?").bind(meeting_id).bind(label).fetch_optional(pool).await?;
    let centroid = centroid.map(|b| from_blob(&b)).unwrap_or_default();
    if centroid.is_empty() || !super::commands::voice_recognition_on(pool).await {
        return Ok(());
    }
    super::commands::save_voice(pool, model, person, &centroid).await
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkRequest {
    pub meet_name: String,
    pub person: String,
}

/// «Прив'язати позначені»: each account's voice in this meeting gets the
/// person's name, the account is linked and the voice remembered
pub async fn link_in_meeting(pool: &SqlitePool, model: &str, meeting_id: &str, links: &[LinkRequest]) -> Result<usize> {
    let mut done = 0;
    for l in links {
        let person = l.person.trim();
        if person.is_empty() {
            continue;
        }
        let rows = sqlx::query("SELECT label, name, suggested FROM meeting_speakers WHERE meeting_id = ? AND meet_name = ?").bind(meeting_id).bind(&l.meet_name).fetch_all(pool).await?;
        for r in rows {
            let label: String = r.get("label");
            let name: Option<String> = r.get("name");
            let suggested: Option<String> = r.get("suggested");
            if name.as_deref() == Some(person) {
                // Already shown so (found by voice): only the voice is remembered
                remember_voice(pool, model, meeting_id, &label, person).await?;
            } else {
                let shown = name.or_else(|| suggested.as_deref().map(super::names::unconfirmed)).unwrap_or_else(|| label.clone());
                rename_with(pool, model, meeting_id, &shown, person).await?;
            }
        }
        link(pool, &l.meet_name, Some(person)).await?;
        let _ = sqlx::query("UPDATE meeting_speakers SET name_source = ? WHERE meeting_id = ? AND meet_name = ?").bind(LINK).bind(meeting_id).bind(&l.meet_name).execute(pool).await;
        done += 1;
    }
    Ok(done)
}

/// A voice of a meeting next to the saved people, for its card in «Спікери»
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceMatch {
    pub name: String,
    pub similarity: f32,
    /// high, medium, low
    pub level: String,
}

pub fn level_of(similarity: f32) -> &'static str {
    if similarity >= NAME_SIMILARITY {
        "high"
    } else if similarity >= UNSURE_SIMILARITY {
        "medium"
    } else {
        "low"
    }
}

pub async fn voice_matches(pool: &SqlitePool, model: &str, meeting_id: &str, label: &str) -> Result<Vec<VoiceMatch>> {
    let centroid: Option<Vec<u8>> = sqlx::query_scalar("SELECT centroid FROM meeting_speakers WHERE meeting_id = ? AND (label = ? OR name = ?)")
        .bind(meeting_id)
        .bind(label)
        .bind(label)
        .fetch_optional(pool)
        .await?;
    let centroid = centroid.map(|b| from_blob(&b)).unwrap_or_default();
    let people = profiles(pool, model).await;
    Ok(closest(&centroid, &people).into_iter().map(|(name, similarity)| VoiceMatch { level: level_of(similarity).into(), name, similarity }).collect())
}

fn model_of<R: Runtime>(app: &AppHandle<R>) -> String {
    models_dir(app).map(|d| model_id(&d)).unwrap_or_else(|| "emb".into())
}

#[tauri::command]
pub async fn get_meet_accounts<R: Runtime>(app: AppHandle<R>) -> Result<Vec<MeetAccount>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    accounts_with_hints(&pool, &model_of(&app)).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_meet_account<R: Runtime>(app: AppHandle<R>, meet_name: String, person: Option<String>) -> Result<(), String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    let person = person.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    link(&pool, &meet_name, person.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_meet_link_candidates<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<Vec<LinkCandidate>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    link_candidates(&pool, &model_of(&app), &meeting_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn link_meet_accounts<R: Runtime>(app: AppHandle<R>, meeting_id: String, links: Vec<LinkRequest>) -> Result<usize, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    link_in_meeting(&pool, &model_of(&app), &meeting_id, &links).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_speaker_voice_matches<R: Runtime>(app: AppHandle<R>, meeting_id: String, label: String) -> Result<Vec<VoiceMatch>, String> {
    let pool = pool(&app).map_err(|e| e.to_string())?;
    voice_matches(&pool, &model_of(&app), &meeting_id, &label).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, v: Vec<f32>) -> VoiceProfile {
        VoiceProfile { name: name.into(), embedding: v, meetings: 3, role: None }
    }

    #[test]
    fn offers_follow_voice_then_name() {
        let people = vec![person("Перший", vec![1.0, 0.0, 0.0]), person("Другий", vec![0.0, 1.0, 0.0]), person("Третій", vec![0.0, 0.0, 1.0])];
        // A sure voice
        assert_eq!(offer("Петро Тестовий", &[0.95, 0.31, 0.0], &people), (Some("Перший".into()), "voice", true));
        // The same name and the voice agrees
        assert_eq!(offer("Другий", &[0.1, 0.99, 0.0], &people), (Some("Другий".into()), "name", true));
        // The same name, the voice says nothing either way
        assert_eq!(offer("другий", &[], &people), (Some("Другий".into()), "name", true));
        // A similar voice only
        assert_eq!(offer("Олена Тестова", &[0.55, 0.5, 0.6], &people), (Some("Третій".into()), "maybe", false));
        // No one
        assert_eq!(offer("Інша Особа", &[-0.5, -0.5, 0.3], &people), (None, "none", false));
        assert_eq!(level_of(0.7), "high");
        assert_eq!(level_of(0.55), "medium");
    }
}
