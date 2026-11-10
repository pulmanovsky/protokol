//! One document per template for a meeting. The document shown is the
//! `summary_processes` row; each finished document is also kept here under
//! its template, so going back to a template shows its document again
//! without making it anew. The secretary's edits go to both.

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentVariant {
    pub template_id: String,
    pub created_at: String,
    pub updated_at: String,
    /// The document shown now
    pub current: bool,
}

/// A stable fingerprint of the transcript a document was made from (FNV-1a)
pub fn source_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.trim().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub struct DocumentVariantsRepository;

impl DocumentVariantsRepository {
    /// A document just made with `template_id`: it is the one shown, and kept
    /// under its template (a document made anew replaces the earlier one)
    pub async fn record(pool: &SqlitePool, meeting_id: &str, template_id: &str, source_hash: Option<&str>) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE summary_processes SET template_id = ? WHERE meeting_id = ?")
            .bind(template_id)
            .bind(meeting_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO document_variants (meeting_id, template_id, result, source_hash, created_at, updated_at)
             SELECT meeting_id, ?, result, ?, ?, ? FROM summary_processes WHERE meeting_id = ? AND result IS NOT NULL
             ON CONFLICT(meeting_id, template_id) DO UPDATE SET
                result = excluded.result, source_hash = excluded.source_hash,
                created_at = excluded.created_at, updated_at = excluded.updated_at",
        )
        .bind(template_id)
        .bind(source_hash)
        .bind(&now)
        .bind(&now)
        .bind(meeting_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    /// A document made before 0.3.8 has no template on record: kept under
    /// `template_id` (the one it was most likely made with) before another
    /// template replaces it. Nothing changes when it already has one.
    pub async fn keep_shown(pool: &SqlitePool, meeting_id: &str, template_id: &str) -> Result<bool, sqlx::Error> {
        let untracked: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM summary_processes WHERE meeting_id = ? AND template_id IS NULL AND result IS NOT NULL AND LOWER(status) = 'completed'",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?;
        if untracked.is_none() {
            return Ok(false);
        }
        Self::record(pool, meeting_id, template_id, None).await?;
        Ok(true)
    }

    /// The documents kept for a meeting, oldest first
    pub async fn list(pool: &SqlitePool, meeting_id: &str) -> Result<Vec<DocumentVariant>, sqlx::Error> {
        let rows: Vec<(String, String, String, bool)> = sqlx::query_as(
            "SELECT v.template_id, v.created_at, v.updated_at,
                    COALESCE(v.template_id = (SELECT template_id FROM summary_processes WHERE meeting_id = v.meeting_id), 0)
             FROM document_variants v WHERE v.meeting_id = ? ORDER BY v.created_at",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(template_id, created_at, updated_at, current)| DocumentVariant { template_id, created_at, updated_at, current })
            .collect())
    }

    /// Shows the document kept under `template_id`; false when there is none
    pub async fn show(pool: &SqlitePool, meeting_id: &str, template_id: &str) -> Result<bool, sqlx::Error> {
        let now = Utc::now();
        let r = sqlx::query(
            "UPDATE summary_processes SET
                result = (SELECT result FROM document_variants WHERE meeting_id = ? AND template_id = ?),
                template_id = ?, status = 'completed', error = NULL, updated_at = ?
             WHERE meeting_id = ? AND LOWER(status) != 'pending'
               AND EXISTS (SELECT 1 FROM document_variants WHERE meeting_id = ? AND template_id = ?)",
        )
        .bind(meeting_id)
        .bind(template_id)
        .bind(template_id)
        .bind(now)
        .bind(meeting_id)
        .bind(meeting_id)
        .bind(template_id)
        .execute(pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// The template of the document shown now, and whether the transcript
    /// changed since it was made (`text` as it would be sent now)
    pub async fn shown(pool: &SqlitePool, meeting_id: &str, text: Option<&str>) -> Result<(Option<String>, bool), sqlx::Error> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT p.template_id, v.source_hash FROM summary_processes p
             LEFT JOIN document_variants v ON v.meeting_id = p.meeting_id AND v.template_id = p.template_id
             WHERE p.meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?;
        let Some((template_id, hash)) = row else { return Ok((None, false)) };
        let changed = match (text, hash) {
            (Some(text), Some(hash)) => source_hash(text) != hash,
            _ => false,
        };
        Ok((template_id, changed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Нарада', '2026-11-10T08:00:00Z', '2026-11-10T08:00:00Z')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn finish(pool: &SqlitePool, result: &str) {
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES ('m1', 'completed', '2026-11-10T08:00:00Z', '2026-11-10T08:00:00Z', ?)
             ON CONFLICT(meeting_id) DO UPDATE SET result = excluded.result, status = 'completed'",
        )
        .bind(result)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn result(pool: &SqlitePool) -> String {
        sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = 'm1'").fetch_one(pool).await.unwrap()
    }

    #[test]
    fn the_fingerprint_is_stable() {
        assert_eq!(source_hash("Спікер 1: добрий день\n"), source_hash("Спікер 1: добрий день"));
        assert_ne!(source_hash("Спікер 1: добрий день"), source_hash("Спікер 1: добрий вечір"));
        assert_eq!(source_hash(""), "cbf29ce484222325");
    }

    #[tokio::test]
    async fn each_template_keeps_its_document_and_edits() {
        let pool = pool().await;
        // A document made before: kept under its likely template, once
        finish(&pool, r#"{"markdown":"старий"}"#).await;
        assert!(DocumentVariantsRepository::keep_shown(&pool, "m1", "meeting_materials_uk").await.unwrap());
        assert!(!DocumentVariantsRepository::keep_shown(&pool, "m1", "decisions_tasks_uk").await.unwrap());
        // No fingerprint: never «changed»
        assert_eq!(DocumentVariantsRepository::shown(&pool, "m1", Some("будь-що")).await.unwrap(), (Some("meeting_materials_uk".into()), false));
        finish(&pool, r#"{"markdown":"підсумки"}"#).await;
        DocumentVariantsRepository::record(&pool, "m1", "meeting_materials_uk", Some(&source_hash("текст"))).await.unwrap();
        finish(&pool, r#"{"markdown":"рішення"}"#).await;
        DocumentVariantsRepository::record(&pool, "m1", "decisions_tasks_uk", Some(&source_hash("текст"))).await.unwrap();

        let list = DocumentVariantsRepository::list(&pool, "m1").await.unwrap();
        assert_eq!(list.iter().map(|v| (v.template_id.as_str(), v.current)).collect::<Vec<_>>(), [("meeting_materials_uk", false), ("decisions_tasks_uk", true)]);

        // The secretary edits the document shown: its template keeps the edit
        let edited = serde_json::json!({"markdown": "рішення, виправлено"});
        assert!(crate::database::repositories::summary::SummaryProcessesRepository::update_meeting_summary(&pool, "m1", &edited).await.unwrap());

        // Back to the first template, then again to the second
        assert!(DocumentVariantsRepository::show(&pool, "m1", "meeting_materials_uk").await.unwrap());
        assert_eq!(result(&pool).await, r#"{"markdown":"підсумки"}"#);
        assert!(DocumentVariantsRepository::show(&pool, "m1", "decisions_tasks_uk").await.unwrap());
        assert!(result(&pool).await.contains("рішення, виправлено"));
        // A template never used has nothing to show
        assert!(!DocumentVariantsRepository::show(&pool, "m1", "meeting_summary_uk").await.unwrap());

        // The transcript changed since the document was made
        assert_eq!(DocumentVariantsRepository::shown(&pool, "m1", Some("текст")).await.unwrap(), (Some("decisions_tasks_uk".into()), false));
        assert_eq!(DocumentVariantsRepository::shown(&pool, "m1", Some("текст змінено")).await.unwrap(), (Some("decisions_tasks_uk".into()), true));

        // Deleting the meeting deletes its documents
        crate::database::repositories::meeting::MeetingsRepository::delete_meeting(&pool, "m1").await.unwrap();
        assert!(DocumentVariantsRepository::list(&pool, "m1").await.unwrap().is_empty());
    }
}
