//! Keeping the secretary's approved protocols and what was learned from
//! them (see `learned`): the draft the app wrote, the approved version,
//! and per-turn examples by meeting type.

use super::learned::{self, Example, Turn};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

/// The document the app has just written (an earlier approval of the same
/// meeting stays until it is approved again)
pub async fn save_draft(pool: &SqlitePool, meeting_id: &str, meeting_type: &str, draft_md: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO protocol_revisions (meeting_id, meeting_type, draft_md, generated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(meeting_id) DO UPDATE SET meeting_type = excluded.meeting_type, draft_md = excluded.draft_md, generated_at = excluded.generated_at",
    )
    .bind(meeting_id)
    .bind(meeting_type)
    .bind(draft_md)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .map(|_| ())
}

/// Turns of a meeting's transcript: consecutive lines of one speaker
pub async fn turns_of(pool: &SqlitePool, meeting_id: &str) -> sqlx::Result<Vec<Turn>> {
    let rows = sqlx::query("SELECT transcript, speaker, audio_start_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time").bind(meeting_id).fetch_all(pool).await?;
    let mut turns: Vec<Turn> = Vec::new();
    for r in rows {
        let text: String = r.get("transcript");
        let speaker: String = r.get::<Option<String>, _>("speaker").unwrap_or_default();
        let start = r.get::<Option<f64>, _>("audio_start_time").unwrap_or(0.0).max(0.0) as u32;
        match turns.last_mut() {
            Some(t) if t.speaker == speaker => {
                t.text.push(' ');
                t.text.push_str(&text);
            }
            _ => turns.push(Turn { speaker, start, text }),
        }
    }
    Ok(turns)
}

async fn roles_of(pool: &SqlitePool, meeting_id: &str) -> HashMap<String, String> {
    let rows: Vec<(Option<String>, String, Option<String>)> =
        sqlx::query_as("SELECT name, label, role FROM meeting_speakers WHERE meeting_id = ?").bind(meeting_id).fetch_all(pool).await.unwrap_or_default();
    rows.into_iter().filter_map(|(name, label, role)| role.map(|r| (name.unwrap_or(label), r))).collect()
}

/// What an approval taught: edits found and examples kept
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Learned {
    pub edits: usize,
    pub examples: usize,
}

/// On «Затвердити»: compares the app's draft with the approved document and
/// keeps the examples (replacing earlier ones from this meeting)
pub async fn learn_from_approval(pool: &SqlitePool, meeting_id: &str, approved_md: &str) -> sqlx::Result<Learned> {
    let Some(row) = sqlx::query("SELECT meeting_type, draft_md FROM protocol_revisions WHERE meeting_id = ?").bind(meeting_id).fetch_optional(pool).await? else {
        return Ok(Learned::default());
    };
    let (meeting_type, draft_md): (String, String) = (row.get("meeting_type"), row.get("draft_md"));
    let (draft, approved) = (learned::parse_doc(&draft_md), learned::parse_doc(approved_md));
    let edits = learned::diff(&draft, &approved).len();
    let examples = learned::examples_from(meeting_id, &meeting_type, &turns_of(pool, meeting_id).await?, &roles_of(pool, meeting_id).await, &draft, &approved);
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM learned_examples WHERE meeting_id = ?").bind(meeting_id).execute(&mut *tx).await?;
    let now = chrono::Utc::now().to_rfc3339();
    for e in &examples {
        sqlx::query("INSERT INTO learned_examples (meeting_id, meeting_type, speaker, role, turn_text, approved, edits, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&e.meeting_id)
            .bind(&e.meeting_type)
            .bind(&e.speaker)
            .bind(&e.role)
            .bind(&e.turn_text)
            .bind(&e.approved)
            .bind(serde_json::to_string(&e.edits).unwrap_or_else(|_| "[]".into()))
            .bind(&now)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE protocol_revisions SET final_md = ?, approved_at = ? WHERE meeting_id = ?").bind(approved_md).bind(&now).bind(meeting_id).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::learned_events::note("document", "", &meeting_type, examples.len(), Some(meeting_id));
    Ok(Learned { edits, examples: examples.len() })
}

/// The examples kept for a meeting type
pub async fn examples_for(pool: &SqlitePool, meeting_type: &str) -> sqlx::Result<Vec<Example>> {
    let rows = sqlx::query("SELECT meeting_id, meeting_type, speaker, role, turn_text, approved, edits FROM learned_examples WHERE meeting_type = ? ORDER BY id")
        .bind(meeting_type)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Example {
            meeting_id: r.get("meeting_id"),
            meeting_type: r.get("meeting_type"),
            speaker: r.get("speaker"),
            role: r.get("role"),
            turn_text: r.get("turn_text"),
            approved: r.get("approved"),
            edits: serde_json::from_str(&r.get::<String, _>("edits")).unwrap_or_default(),
        })
        .collect())
}

/// For «Словник»: approved meetings learned from, by meeting type
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedMeeting {
    pub meeting_id: String,
    pub title: String,
    pub meeting_type: String,
    pub examples: i64,
    pub approved_at: Option<String>,
}

pub async fn learned_meetings(pool: &SqlitePool) -> sqlx::Result<Vec<LearnedMeeting>> {
    let rows = sqlx::query(
        "SELECT r.meeting_id, COALESCE(m.title, '') AS title, r.meeting_type, r.approved_at,
                (SELECT COUNT(*) FROM learned_examples e WHERE e.meeting_id = r.meeting_id) AS examples
         FROM protocol_revisions r LEFT JOIN meetings m ON m.id = r.meeting_id
         WHERE r.approved_at IS NOT NULL ORDER BY r.approved_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| LearnedMeeting { meeting_id: r.get("meeting_id"), title: r.get("title"), meeting_type: r.get("meeting_type"), examples: r.get("examples"), approved_at: r.get("approved_at") })
        .collect())
}

/// Forgets what was learned from one approved meeting
pub async fn forget_meeting(pool: &SqlitePool, meeting_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM learned_examples WHERE meeting_id = ?").bind(meeting_id).execute(pool).await?;
    sqlx::query("UPDATE protocol_revisions SET final_md = NULL, approved_at = NULL WHERE meeting_id = ?").bind(meeting_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn an_approval_keeps_examples_for_the_type() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let manager = crate::database::manager::DatabaseManager::new(db.to_str().unwrap(), "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        for (id, text, speaker, t) in [("a", "Почнемо.", "Я", 0.0), ("b", "Склад готовий. Подамо заявку на пальне до вечора.", "Спікер 2", 60.0)] {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, speaker) VALUES (?, 'm1', ?, 'x', ?, ?)")
                .bind(id)
                .bind(text)
                .bind(t)
                .bind(speaker)
                .execute(&pool)
                .await
                .unwrap();
        }
        let draft = "## Хід наради\n\n**ВИСТУПИЛИ:** Спікер 2 [01:00] — Склад готовий.\n\n## Завдання\n\n| Хто | Що | До коли | Час у записі |\n| --- | --- | --- | --- |\n| Спікер 2 | Перевірити склад | — | [01:00] |\n";
        let approved = "## Хід наради\n\n**ВИСТУПИЛИ:** Спікер 2 [01:00] — Склад готовий.\n\n## Завдання\n\n| Хто | Що | До коли | Час у записі |\n| --- | --- | --- | --- |\n| Спікер 2 | Подати заявку на пальне | до вечора | [01:00] |\n";
        super::save_draft(&pool, "m1", "Щоденна нарада", draft).await.unwrap();
        let learned = super::learn_from_approval(&pool, "m1", approved).await.unwrap();
        assert_eq!(learned.edits, 2); // one task added, one removed
        assert_eq!(learned.examples, 1);
        let ex = super::examples_for(&pool, "Щоденна нарада").await.unwrap();
        assert!(ex[0].approved.contains("Подати заявку на пальне"));
        assert!(ex[0].edits.iter().any(|e| e.contains("прибрав")));
        let list = super::learned_meetings(&pool).await.unwrap();
        assert_eq!((list.len(), list[0].examples), (1, 1));
        super::forget_meeting(&pool, "m1").await.unwrap();
        assert!(super::examples_for(&pool, "Щоденна нарада").await.unwrap().is_empty());
        // No draft kept: nothing to learn
        assert_eq!(super::learn_from_approval(&pool, "nope", approved).await.unwrap().examples, 0);
    }
}

#[cfg(test)]
mod on_a_copy {
    /// PROTOKOL_DB = a COPY of the app database: the learning migration runs
    /// on real data and nothing else changes
    #[tokio::test]
    #[ignore = "needs a database copy"]
    async fn migrations_run_on_a_database_copy() {
        let db = std::env::var("PROTOKOL_DB").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let meetings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings").fetch_one(&pool).await.unwrap();
        let learned = super::learned_meetings(&pool).await.unwrap();
        println!("COPY OK: {meetings} meetings, {} approved learned", learned.len());
        assert!(meetings > 0);
    }

    /// On a COPY: the stored draft «approved» with one changed task; the
    /// examples are kept and come back for the next document of that type,
    /// as a prompt block. PROTOKOL_DB, PROTOKOL_MEETING. Counts only.
    #[tokio::test]
    #[ignore = "needs a database copy"]
    async fn a_synthetic_approval_on_a_copy_is_learned_and_used() {
        let db = std::env::var("PROTOKOL_DB").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let meeting = std::env::var("PROTOKOL_MEETING").unwrap();
        let manager = crate::database::manager::DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let pool = manager.pool().clone();
        let (draft, kind): (String, String) = sqlx::query_as("SELECT draft_md, meeting_type FROM protocol_revisions WHERE meeting_id = ?").bind(&meeting).fetch_one(&pool).await.unwrap();
        // The secretary rewrites the first point of the first «ВИСТУПИЛИ» block
        let mut approved = String::new();
        let mut done = false;
        for line in draft.lines() {
            if !done && approved.contains("ВИСТУПИЛИ") && line.trim_start().starts_with("- ") {
                approved.push_str("- Доповів про стан комплектації відділу.\n");
                done = true;
            } else {
                approved.push_str(line);
                approved.push('\n');
            }
        }
        let learned = super::learn_from_approval(&pool, &meeting, &approved).await.unwrap();
        let examples = super::examples_for(&pool, &kind).await.unwrap();
        let block = super::learned::prompt_block(&examples.iter().take(3).collect::<Vec<_>>(), 4000);
        println!("SYNTHETIC edits {} examples kept {} retrieved for the type {} prompt block {} chars, has the edit: {}", learned.edits, learned.examples, examples.len(), block.chars().count(), block.contains("комплектації"));
    }
}
