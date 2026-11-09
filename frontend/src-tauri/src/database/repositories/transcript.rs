use crate::api::{TranscriptSearchResult, TranscriptSegment};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

pub struct TranscriptsRepository;

impl TranscriptsRepository {
    /// A meeting already saved for this recording: the same recording folder,
    /// or (a save that lost its folder) the same title within two minutes.
    /// A row saved without its folder gets the folder when it comes.
    async fn same_recording(pool: &SqlitePool, title: &str, folder: Option<&str>) -> Result<Option<String>, SqlxError> {
        const SAME_SAVE_S: i64 = 120;
        let folder = folder.map(str::trim).filter(|f| !f.is_empty());
        if let Some(f) = folder {
            let by_folder: Option<String> = sqlx::query_scalar("SELECT id FROM meetings WHERE folder_path = ? ORDER BY created_at LIMIT 1")
                .bind(f)
                .fetch_optional(pool)
                .await?;
            if by_folder.is_some() {
                return Ok(by_folder);
            }
        }
        let recent: Vec<(String, Option<String>, String)> =
            sqlx::query_as("SELECT id, folder_path, CAST(created_at AS TEXT) FROM meetings WHERE title = ? ORDER BY created_at DESC LIMIT 5")
                .bind(title)
                .fetch_all(pool)
                .await?;
        let now = Utc::now();
        for (id, old_folder, created) in recent {
            let Ok(at) = chrono::DateTime::parse_from_rfc3339(&created) else { continue };
            if (now - at.with_timezone(&Utc)).num_seconds().abs() > SAME_SAVE_S {
                continue;
            }
            let old_empty = old_folder.as_deref().map(str::trim).unwrap_or("").is_empty();
            match folder {
                // A different recording that happens to have the same title
                Some(_) if !old_empty => continue,
                Some(f) => {
                    sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = ?").bind(f).bind(&id).execute(pool).await?;
                    return Ok(Some(id));
                }
                None => return Ok(Some(id)),
            }
        }
        Ok(None)
    }

    /// Saves a new meeting and its associated transcript segments.
    /// This function uses a transaction to ensure that either both the meeting
    /// and all its transcripts are saved, or none of them are.
    pub async fn save_transcript(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
    ) -> Result<String, SqlxError> {
        // The same recording saved twice (two stop paths fired for one stop)
        // gets one meeting: the second save returns the first one's id
        if let Some(existing) = Self::same_recording(pool, meeting_title, folder_path.as_deref()).await? {
            info!("Meeting already saved for this recording: {}", existing);
            crate::learned_events::record_auto_fixes(&existing, &crate::ukrainian::corrections::take_pending());
            return Ok(existing);
        }
        let meeting_id = format!("meeting-{}", Uuid::new_v4());

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now();

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(now)
        .bind(now)
        .bind(&folder_path)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting '{}': {}", meeting_title, e);
            transaction.rollback().await?;
            return Err(e);
        }

        info!("Successfully created meeting with id: {}", meeting_id);

        // 2. Save each transcript segment with audio timing fields
        for segment in transcripts {
            let transcript_id = format!("transcript-{}", Uuid::new_v4());
            let result = sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
                 VALUES (?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&transcript_id)
            .bind(&meeting_id)
            .bind(&segment.text)
            .bind(&segment.timestamp)
            .bind(segment.audio_start_time)
            .bind(segment.audio_end_time)
            .bind(segment.duration)
            .execute(&mut *transaction)
            .await;

            if let Err(e) = result {
                error!(
                    "Failed to save transcript segment for meeting {}: {}",
                    meeting_id, e
                );
                transaction.rollback().await?;
                return Err(e);
            }
        }

        info!(
            "Successfully saved {} transcript segments for meeting {}",
            transcripts.len(),
            meeting_id
        );

        // Commit the transaction
        transaction.commit().await?;
        // Words the learned rules fixed while this recording was recognised
        crate::learned_events::record_auto_fixes(&meeting_id, &crate::ukrainian::corrections::take_pending());

        Ok(meeting_id)
    }

    /// Searches for a query string within the transcripts.
    /// It returns a list of matching transcripts with context.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, SqlxError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_query = format!("%{}%", query.to_lowercase());

        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT m.id, m.title, t.transcript, t.timestamp
             FROM meetings m
             JOIN transcripts t ON m.id = t.meeting_id
             WHERE LOWER(t.transcript) LIKE ?",
        )
        .bind(&search_query)
        .fetch_all(pool)
        .await?;

        let results = rows
            .into_iter()
            .map(|(id, title, transcript, timestamp)| {
                let match_context = Self::get_match_context(&transcript, query);
                TranscriptSearchResult {
                    id,
                    title,
                    match_context,
                    timestamp,
                }
            })
            .collect();

        Ok(results)
    }

    /// Helper function to extract a snippet of text around the first match of a query.
    fn get_match_context(transcript: &str, query: &str) -> String {
        let transcript_lower = transcript.to_lowercase();
        let query_lower = query.to_lowercase();

        match transcript_lower.find(&query_lower) {
            Some(match_index) => {
                let start_index = match_index.saturating_sub(100);
                let end_index = (match_index + query.len() + 100).min(transcript.len());

                let mut context = String::new();
                if start_index > 0 {
                    context.push_str("...");
                }
                context.push_str(&transcript[start_index..end_index]);
                if end_index < transcript.len() {
                    context.push_str("...");
                }
                context
            }
            None => transcript.chars().take(200).collect(), // Fallback to the start of the transcript
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, folder_path TEXT, status TEXT NOT NULL DEFAULT 'draft')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE transcripts (id TEXT PRIMARY KEY, meeting_id TEXT, transcript TEXT, timestamp TEXT, audio_start_time REAL, audio_end_time REAL, duration REAL)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn lines() -> Vec<TranscriptSegment> {
        vec![TranscriptSegment { id: "1".into(), text: "Доброго ранку".into(), timestamp: "08:00".into(), audio_start_time: Some(1.0), audio_end_time: Some(2.0), duration: Some(1.0) }]
    }

    async fn rows(pool: &SqlitePool) -> Vec<(String, Option<String>)> {
        sqlx::query_as("SELECT id, folder_path FROM meetings ORDER BY created_at").fetch_all(pool).await.unwrap()
    }

    async fn transcripts(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM transcripts").fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn one_recording_saved_twice_is_one_meeting() {
        // The same folder twice
        let p = pool().await;
        let a = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some("/r/Нарада 1".into())).await.unwrap();
        let b = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some("/r/Нарада 1".into())).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(rows(&p).await.len(), 1);
        assert_eq!(transcripts(&p).await, 1);

        // Folder first, then a save that lost the folder (this morning's case)
        let p = pool().await;
        let a = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some("/r/Нарада 2".into())).await.unwrap();
        let b = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), None).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(rows(&p).await, vec![(a.clone(), Some("/r/Нарада 2".to_string()))]);

        // No folder first, then the folder: the row gets it
        let p = pool().await;
        let a = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some(String::new())).await.unwrap();
        let b = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some("/r/Нарада 3".into())).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(rows(&p).await, vec![(a, Some("/r/Нарада 3".to_string()))]);
        assert_eq!(transcripts(&p).await, 1);
    }

    #[tokio::test]
    async fn different_meetings_with_one_title_are_both_kept() {
        let p = pool().await;
        // A meeting with this title five minutes ago, then a new one
        let earlier = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES ('m-old', 'Нарада', ?, ?, NULL)")
            .bind(&earlier)
            .bind(&earlier)
            .execute(&p)
            .await
            .unwrap();
        let b = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), None).await.unwrap();
        assert_ne!(b, "m-old");
        // Two recordings with the same title a moment apart, each with its own folder
        let c = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some("/r/a".into())).await.unwrap();
        let d = TranscriptsRepository::save_transcript(&p, "Нарада", &lines(), Some("/r/b".into())).await.unwrap();
        assert_ne!(c, d);
        assert_eq!(rows(&p).await.len(), 3, "old one, the folderless one with /r/a filled in, and /r/b");
    }
}
