use sqlx::{migrate::MigrateDatabase, Result, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::Path;
use tauri::Manager;

#[derive(Clone)]
pub struct DatabaseManager {
    pool: SqlitePool,
}

/// A migration whose comment was edited after it ran has a new checksum;
/// the record of an already applied one is brought up to date (nothing re-runs)
const EDITED_MIGRATIONS: &[(i64, &str)] = &[
    (20261106100000, "c0cd65ef78400d6f8147df3978436627c7dbcf9ebf93907bc749a9e2214feb96206b35ee8deb04b8f1b02ab36b6e4503"),
    (20261106200000, "70d2a8a0421d62cf646f035636035f61931e9b0fe7d8ab52e2e18ae1cd7b1bcb693b70d914814db5ef9c60bdf81a95cb"),
];

/// Recordings made before the Ukrainian default name are titled «New Recording»:
/// they get «Запис DD.MM.YYYY HH:MM» from the time they were made (local time).
/// The database is copied first (`VACUUM INTO`), next to it.
async fn rename_english_default_titles(pool: &SqlitePool, db_path: &str) {
    let rows: Vec<(String, String)> = match sqlx::query_as("SELECT id, created_at FROM meetings WHERE title = 'New Recording'").fetch_all(pool).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("Default titles not checked: {e}");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    let backup = format!("{db_path}.before-titles-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
    if let Err(e) = sqlx::query("VACUUM INTO ?").bind(&backup).execute(pool).await {
        log::warn!("Default titles left as they are: no backup ({e})");
        return;
    }
    for (id, created) in rows {
        let title = chrono::DateTime::parse_from_rfc3339(&created)
            .map(|t| format!("Запис {}", t.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M")))
            .unwrap_or_else(|_| "Новий запис".to_string());
        if let Err(e) = sqlx::query("UPDATE meetings SET title = ? WHERE id = ? AND title = 'New Recording'").bind(&title).bind(&id).execute(pool).await {
            log::warn!("Title of {id} not changed: {e}");
        }
    }
    log::info!("English default titles renamed (backup: {backup})");
}

async fn repair_edited_migrations(pool: &SqlitePool, migrator: &sqlx::migrate::Migrator) {
    for (version, old) in EDITED_MIGRATIONS {
        let Some(migration) = migrator.iter().find(|m| m.version == *version) else { continue };
        let old: Vec<u8> = (0..old.len() / 2).filter_map(|i| u8::from_str_radix(&old[2 * i..2 * i + 2], 16).ok()).collect();
        // No table yet on a new database: nothing to repair
        let _ = sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ? AND checksum = ?").bind(migration.checksum.as_ref()).bind(version).bind(old).execute(pool).await;
    }
}

impl DatabaseManager {
    pub async fn new(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        if let Some(parent_dir) = Path::new(tauri_db_path).parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir).map_err(|e| sqlx::Error::Io(e))?;
            }
        }

        if !Path::new(tauri_db_path).exists() {
            if Path::new(backend_db_path).exists() {
                log::info!(
                    "Copying database from {} to {}",
                    backend_db_path,
                    tauri_db_path
                );
                fs::copy(backend_db_path, tauri_db_path).map_err(|e| sqlx::Error::Io(e))?;
            } else {
                log::info!("Creating database at {}", tauri_db_path);
                Sqlite::create_database(tauri_db_path).await?;
            }
        }

        let pool = SqlitePool::connect(tauri_db_path).await?;

        {
            // An older app version must still start after a newer one added tables
            let mut migrator = sqlx::migrate!("./migrations");
            migrator.set_ignore_missing(true);
            repair_edited_migrations(&pool, &migrator).await;
            migrator.run(&pool).await?;
        }
        rename_english_default_titles(&pool, tauri_db_path).await;

        Ok(DatabaseManager { pool })
    }

    // NOTE: So for the first time users they needs to start the application
    // after they can just delete the existing .sqlite file and then copy the existing .db file to
    // the current app dir, So the system detects legacy db and copy it and starts with that data
    // (Newly created .sqlite with the copied content from .db)
    pub async fn new_from_app_handle(app_handle: &tauri::AppHandle) -> Result<Self> {
        // Resolve the app's data directory
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");
        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Define database paths
        let tauri_db_path = app_data_dir
            .join("meeting_minutes.sqlite")
            .to_string_lossy()
            .to_string();
        // Legacy backend DB path (for auto-migration if exists)
        let backend_db_path = app_data_dir
            .join("meeting_minutes.db")
            .to_string_lossy()
            .to_string();

        // WAL file paths for defensive cleanup
        let wal_path = app_data_dir.join("meeting_minutes.sqlite-wal");
        let shm_path = app_data_dir.join("meeting_minutes.sqlite-shm");

        log::info!("Tauri DB path: {}", tauri_db_path);
        log::info!("Legacy backend DB path: {}", backend_db_path);

        // Try to open database with defensive WAL handling
        match Self::new(&tauri_db_path, &backend_db_path).await {
            Ok(db_manager) => {
                log::info!("Database opened successfully");
                Ok(db_manager)
            }
            Err(e) => {
                // Check if error is due to corrupted WAL file
                let error_msg = e.to_string();
                if error_msg.contains("malformed") || error_msg.contains("corrupt") {
                    log::warn!("Database appears corrupted, likely due to orphaned WAL file. Attempting recovery...");
                    log::warn!("Error details: {}", error_msg);

                    // Delete potentially corrupted WAL/SHM files
                    if wal_path.exists() {
                        match fs::remove_file(&wal_path) {
                            Ok(_) => log::info!("Removed orphaned WAL file: {:?}", wal_path),
                            Err(e) => log::warn!("Failed to remove WAL file: {}", e),
                        }
                    }
                    if shm_path.exists() {
                        match fs::remove_file(&shm_path) {
                            Ok(_) => log::info!("Removed orphaned SHM file: {:?}", shm_path),
                            Err(e) => log::warn!("Failed to remove SHM file: {}", e),
                        }
                    }

                    // Retry connection without WAL files
                    log::info!("Retrying database connection after WAL cleanup...");
                    match Self::new(&tauri_db_path, &backend_db_path).await {
                        Ok(db_manager) => {
                            log::info!("Database opened successfully after WAL recovery");
                            Ok(db_manager)
                        }
                        Err(retry_err) => {
                            log::error!("Database connection failed even after WAL cleanup: {}", retry_err);
                            Err(retry_err)
                        }
                    }
                } else {
                    // Not a WAL-related error, propagate original error
                    log::error!("Database connection failed: {}", error_msg);
                    Err(e)
                }
            }
        }
    }

    /// Check if this is the first launch (sqlite database doesn't exist yet)
    pub async fn is_first_launch(app_handle: &tauri::AppHandle) -> Result<bool> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        let tauri_db_path = app_data_dir.join("meeting_minutes.sqlite");

        Ok(!tauri_db_path.exists())
    }

    /// Import a legacy database from the specified path and initialize
    pub async fn import_legacy_database(
        app_handle: &tauri::AppHandle,
        legacy_db_path: &str,
    ) -> Result<Self> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Copy legacy database to app data directory as meeting_minutes.db
        let target_legacy_path = app_data_dir.join("meeting_minutes.db");
        log::info!(
            "Copying legacy database from {} to {}",
            legacy_db_path,
            target_legacy_path.display()
        );

        fs::copy(legacy_db_path, &target_legacy_path).map_err(|e| sqlx::Error::Io(e))?;

        // Now use the standard initialization which will detect and migrate the legacy db
        Self::new_from_app_handle(app_handle).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn with_transaction<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Transaction<'_, Sqlite>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut tx = self.pool.begin().await?;
        let result = f(&mut tx).await;

        match result {
            Ok(val) => {
                tx.commit().await?;
                Ok(val)
            }
            Err(err) => {
                tx.rollback().await?;
                Err(err)
            }
        }
    }

    /// Cleanup database connection and checkpoint WAL
    /// This should be called on application shutdown to ensure:
    /// - All WAL changes are written to the main database file
    /// - The .wal and .shm files are deleted
    /// - Connection pool is gracefully closed
    pub async fn cleanup(&self) -> Result<()> {
        log::info!("Starting database cleanup...");

        // Force checkpoint of WAL to main database file and remove WAL file
        // TRUNCATE mode: checkpoints all pages AND deletes the WAL file
        match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
        {
            Ok(_) => log::info!("WAL checkpoint completed successfully"),
            Err(e) => log::warn!("WAL checkpoint failed (non-fatal): {}", e),
        }

        // Close the connection pool gracefully
        self.pool.close().await;
        log::info!("Database connection pool closed");

        Ok(())
    }
}

#[cfg(test)]
mod repair_tests {
    use super::*;

    const OLD: &str = EDITED_MIGRATIONS[0].1;

    async fn counts(pool: &SqlitePool) -> (i64, i64) {
        (sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations").fetch_one(pool).await.unwrap(), sqlx::query_scalar("SELECT COUNT(*) FROM meetings").fetch_one(pool).await.unwrap())
    }

    #[tokio::test]
    async fn an_old_checksum_is_brought_up_to_date_and_nothing_runs_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.sqlite");
        let db = db.to_str().unwrap();
        let first = DatabaseManager::new(db, "/nonexistent-backend.db").await.unwrap();
        let pool = first.pool().clone();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Тест', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')").execute(&pool).await.unwrap();
        let old: Vec<u8> = (0..OLD.len() / 2).map(|i| u8::from_str_radix(&OLD[2 * i..2 * i + 2], 16).unwrap()).collect();
        sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?").bind(&old).bind(EDITED_MIGRATIONS[0].0).execute(&pool).await.unwrap();
        let before = counts(&pool).await;
        pool.close().await;
        let again = DatabaseManager::new(db, "/nonexistent-backend.db").await.unwrap();
        assert_eq!(counts(again.pool()).await, before);
        let now: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?").bind(EDITED_MIGRATIONS[0].0).fetch_one(again.pool()).await.unwrap();
        assert_ne!(now, old);
    }

    #[tokio::test]
    #[ignore]
    async fn a_copy_of_a_real_database_opens() {
        let db = std::env::var("PROTOKOL_DB_COPY").unwrap();
        assert!(db.contains(&crate::test_copy_root()), "only a copy");
        let raw = SqlitePool::connect(&db).await.unwrap();
        let tables = ["meetings", "transcripts", "meeting_speakers", "voice_profiles", "summary_processes", "_sqlx_migrations"];
        let mut before = Vec::new();
        for t in tables {
            before.push(sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {t}")).fetch_one(&raw).await.unwrap());
        }
        raw.close().await;
        let opened = DatabaseManager::new(&db, "/nonexistent-backend.db").await.unwrap();
        let mut after = Vec::new();
        for t in tables {
            after.push(sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {t}")).fetch_one(opened.pool()).await.unwrap());
        }
        assert_eq!(after, before);
        println!("tables unchanged: {after:?}");
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[tokio::test]
    async fn english_default_titles_get_ukrainian_names_after_a_backup() {
        let dir = std::env::temp_dir().join(format!("protokol-titles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("m.sqlite");
        let url = db.to_string_lossy().to_string();
        Sqlite::create_database(&url).await.unwrap();
        let pool = SqlitePool::connect(&url).await.unwrap();
        sqlx::query("CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT, created_at TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings VALUES ('a', 'New Recording', '2026-11-06T19:13:26.498997+00:00'), ('b', 'Нарада 07.11.2026 08:18', '2026-11-07T06:52:09+00:00')").execute(&pool).await.unwrap();
        rename_english_default_titles(&pool, &url).await;
        let a: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id = 'a'").fetch_one(&pool).await.unwrap();
        let b: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id = 'b'").fetch_one(&pool).await.unwrap();
        assert!(a.starts_with("Запис 06.11.2026 "), "{a}");
        assert_eq!(b, "Нарада 07.11.2026 08:18");
        assert!(std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains("before-titles")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
