use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use anyhow::Result;
#[cfg(target_os = "macos")]
use log::error;

#[cfg(target_os = "macos")]
use crate::audio::capture::AudioCaptureBackend;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecordingPreferences {
    pub save_folder: PathBuf,
    pub auto_save: bool,
    pub file_format: String,
    #[serde(default)]
    pub preferred_mic_device: Option<String>,
    #[serde(default)]
    pub preferred_system_device: Option<String>,
    #[cfg(target_os = "macos")]
    #[serde(default)]
    pub system_audio_backend: Option<String>,
}

impl Default for RecordingPreferences {
    fn default() -> Self {
        Self {
            save_folder: get_default_recordings_folder(),
            auto_save: true,
            file_format: "mp4".to_string(),
            preferred_mic_device: None,
            preferred_system_device: None,
            #[cfg(target_os = "macos")]
            system_audio_backend: Some("coreaudio".to_string()),
        }
    }
}

/// The recordings folder: ~/Movies/Protokol
const RECORDINGS_DIR_NAME: &str = "Protokol";
/// Meetily's folder name: only read, to move recordings out of it once
const LEGACY_RECORDINGS_DIR_NAME: &str = "meetily-recordings";

/// The default recordings folder (~/Movies/Protokol on a Mac). Never the
/// legacy Meetily folder: that one is only moved from (see [`migrate_legacy_folder`]).
pub fn get_default_recordings_folder() -> PathBuf {
    default_recordings_parent().join(RECORDINGS_DIR_NAME)
}

/// The folder earlier versions recorded into
pub fn legacy_recordings_folder() -> PathBuf {
    default_recordings_parent().join(LEGACY_RECORDINGS_DIR_NAME)
}

fn is_legacy(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some(LEGACY_RECORDINGS_DIR_NAME)
}

/// Moves what is in `legacy` into `target`: the folder is renamed when
/// `target` does not exist, otherwise item by item without overwriting.
/// `legacy` is removed when it ends up empty. Returns how many items moved.
pub(crate) fn move_legacy_recordings(legacy: &Path, target: &Path) -> std::io::Result<usize> {
    if !legacy.is_dir() {
        return Ok(0);
    }
    if !target.exists() {
        let n = std::fs::read_dir(legacy)?.count();
        std::fs::rename(legacy, target)?;
        info!("Recordings folder moved: {:?} → {:?} ({} item(s))", legacy, target, n);
        return Ok(n);
    }
    let mut moved = 0;
    for entry in std::fs::read_dir(legacy)?.flatten() {
        let to = target.join(entry.file_name());
        if to.exists() {
            warn!("Not moved, already in {:?}: {:?}", target, entry.file_name());
            continue;
        }
        std::fs::rename(entry.path(), &to)?;
        moved += 1;
    }
    info!("Recordings moved from {:?} into {:?}: {}", legacy, target, moved);
    let left = std::fs::read_dir(legacy).map(|d| d.flatten().filter(|e| e.file_name() != ".DS_Store").count()).unwrap_or(1);
    if left == 0 {
        let _ = std::fs::remove_file(legacy.join(".DS_Store"));
        std::fs::remove_dir(legacy)?;
        info!("Empty legacy folder removed: {:?}", legacy);
    }
    Ok(moved)
}

/// Recordings saved in the database under `legacy/…` now point to `target/…`
/// (also those whose files are missing, so a later restore still matches).
/// The database is copied first with `VACUUM INTO <db>.before-recordings-folder-<time>`.
pub(crate) async fn rewrite_recording_paths(pool: &sqlx::SqlitePool, legacy: &Path, target: &Path, db_path: Option<&Path>) -> anyhow::Result<u64> {
    let from = format!("{}/", legacy.to_string_lossy());
    let to = format!("{}/", target.to_string_lossy());
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE substr(folder_path, 1, length(?)) = ?").bind(&from).bind(&from).fetch_one(pool).await?;
    if n == 0 {
        return Ok(0);
    }
    if let Some(db) = db_path {
        let backup = format!("{}.before-recordings-folder-{}", db.to_string_lossy(), chrono::Utc::now().format("%Y%m%d%H%M%S"));
        sqlx::query("VACUUM INTO ?").bind(&backup).execute(pool).await?;
        info!("Database copied before changing recording paths: {backup}");
    }
    let done = sqlx::query("UPDATE meetings SET folder_path = ? || substr(folder_path, length(?) + 1) WHERE substr(folder_path, 1, length(?)) = ?")
        .bind(&to)
        .bind(&from)
        .bind(&from)
        .bind(&from)
        .execute(pool)
        .await?;
    info!("Recording paths rewritten {} → {}: {}", from, to, done.rows_affected());
    Ok(done.rows_affected())
}

/// Once at startup: recordings leave Meetily's folder for ~/Movies/Protokol.
/// Preferences that point at the legacy folder (or nowhere) now point at the
/// default; a folder the person chose stays as it is. The legacy folder is
/// never created.
pub async fn migrate_legacy_folder<R: Runtime>(app: &AppHandle<R>, pool: &sqlx::SqlitePool, db_path: Option<&Path>) {
    let legacy = legacy_recordings_folder();
    let target = get_default_recordings_folder();
    if let Ok(store) = app.store("recording_preferences.json") {
        if let Some(mut value) = store.get("preferences") {
            let saved = value.get("save_folder").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if saved.is_empty() || is_legacy(Path::new(&saved)) {
                value["save_folder"] = serde_json::Value::String(target.to_string_lossy().to_string());
                store.set("preferences", value);
                match store.save() {
                    Ok(()) => info!("Recordings folder setting: {:?} → {:?}", saved, target),
                    Err(e) => warn!("Recordings folder setting not saved: {e}"),
                }
            }
        }
    }
    if legacy.is_dir() {
        if let Err(e) = move_legacy_recordings(&legacy, &target) {
            warn!("Recordings not moved out of {:?}: {e}", legacy);
        }
    }
    match rewrite_recording_paths(pool, &legacy, &target, db_path).await {
        Ok(n) if n > 0 => {}
        Ok(_) => {}
        Err(e) => warn!("Recording paths not rewritten: {e}"),
    }
}

fn default_recordings_parent() -> PathBuf {
    let documents = || dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "windows")]
    {
        // Windows: %USERPROFILE%\Music
        dirs::audio_dir().unwrap_or_else(documents)
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Movies
        dirs::video_dir().unwrap_or_else(documents)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux/Others: ~/Documents
        documents()
    }
}

/// The folder chosen in Settings → Загальні → Сховище (None: the default).
/// Kept in memory so the recorder and the import, which have no app handle,
/// save new recordings there; set whenever the preferences are read or saved.
static CHOSEN_FOLDER: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

fn remember_folder(folder: &PathBuf) {
    let chosen = (folder.as_os_str().len() > 0 && *folder != get_default_recordings_folder()).then(|| folder.clone());
    if let Ok(mut f) = CHOSEN_FOLDER.write() {
        *f = chosen;
    }
}

/// Where new recordings and imports go: the chosen folder when it can be used
/// (an unplugged disk falls back to the default, so a recording is never lost)
pub fn recordings_folder() -> PathBuf {
    let chosen = CHOSEN_FOLDER.read().ok().and_then(|f| f.clone());
    if let Some(folder) = chosen {
        match ensure_recordings_directory(&folder) {
            Ok(()) => return folder,
            Err(e) => warn!("Chosen recordings folder {:?} can't be used ({}); saving to the default folder", folder, e),
        }
    }
    get_default_recordings_folder()
}

/// Ensure the recordings directory exists
pub fn ensure_recordings_directory(path: &PathBuf) -> Result<()> {
    if is_legacy(path) && !path.exists() {
        return Err(anyhow::anyhow!("Meetily's old recordings folder is not created again"));
    }
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        info!("Created recordings directory: {:?}", path);
    }
    Ok(())
}

/// Generate a unique filename for a recording
pub fn generate_recording_filename(format: &str) -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    format!("recording_{}.{}", timestamp, format)
}

/// Load recording preferences from store
pub async fn load_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<RecordingPreferences> {
    // Try to load from Tauri store
    let store = match app.store("recording_preferences.json") {
        Ok(store) => store,
        Err(e) => {
            warn!("Failed to access store: {}, using defaults", e);
            return Ok(RecordingPreferences::default());
        }
    };

    // Try to get the preferences from store
    let prefs = if let Some(value) = store.get("preferences") {
        match serde_json::from_value::<RecordingPreferences>(value.clone()) {
            Ok(mut p) => {
                info!("Loaded recording preferences from store");
                // Update macOS backend to current value if needed
                #[cfg(target_os = "macos")]
                {
                    let backend = crate::audio::capture::get_current_backend();
                    p.system_audio_backend = Some(backend.to_string());
                }
                p
            }
            Err(e) => {
                warn!("Failed to deserialize preferences: {}, using defaults", e);
                RecordingPreferences::default()
            }
        }
    } else {
        info!("No stored preferences found, using defaults");
        RecordingPreferences::default()
    };

    info!("Loaded recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          prefs.save_folder, prefs.auto_save, prefs.file_format,
          prefs.preferred_mic_device, prefs.preferred_system_device);
    remember_folder(&prefs.save_folder);
    Ok(prefs)
}

/// Save recording preferences to store
pub async fn save_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
    preferences: &RecordingPreferences,
) -> Result<()> {
    info!("Saving recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          preferences.save_folder, preferences.auto_save, preferences.file_format,
          preferences.preferred_mic_device, preferences.preferred_system_device);

    // Get or create store
    let store = app
        .store("recording_preferences.json")
        .map_err(|e| anyhow::anyhow!("Failed to access store: {}", e))?;

    // Serialize preferences to JSON value
    let prefs_value = serde_json::to_value(preferences)
        .map_err(|e| anyhow::anyhow!("Failed to serialize preferences: {}", e))?;

    // Save to store
    store.set("preferences", prefs_value);

    // Persist to disk
    store
        .save()
        .map_err(|e| anyhow::anyhow!("Failed to save store to disk: {}", e))?;

    info!("Successfully persisted recording preferences to disk");

    // Save backend preference to global config
    #[cfg(target_os = "macos")]
    if let Some(backend_str) = &preferences.system_audio_backend {
        if let Some(backend) = AudioCaptureBackend::from_string(backend_str) {
            info!("Setting audio capture backend to: {:?}", backend);
            crate::audio::capture::set_current_backend(backend);
        }
    }

    // Ensure the directory exists
    ensure_recordings_directory(&preferences.save_folder)?;
    remember_folder(&preferences.save_folder);

    Ok(())
}

/// Tauri commands for recording preferences
#[tauri::command]
pub async fn get_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RecordingPreferences, String> {
    load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))
}

#[tauri::command]
pub async fn set_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
    preferences: RecordingPreferences,
) -> Result<(), String> {
    save_recording_preferences(&app, &preferences)
        .await
        .map_err(|e| format!("Failed to save recording preferences: {}", e))
}

#[tauri::command]
pub async fn get_default_recordings_folder_path() -> Result<String, String> {
    let path = get_default_recordings_folder();
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_recordings_folder<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let preferences = load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load preferences: {}", e))?;

    // Ensure directory exists before trying to open it
    ensure_recordings_directory(&preferences.save_folder)
        .map_err(|e| format!("Failed to create directory: {}", e))?;

    let folder_path = preferences.save_folder.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    info!("Opened recordings folder: {}", folder_path);
    Ok(())
}

#[tauri::command]
pub async fn select_recording_folder<R: Runtime>(
    _app: AppHandle<R>,
) -> Result<Option<String>, String> {
    // Use Tauri's dialog to select folder
    // For now, return None - this would need to be implemented with tauri-plugin-dialog
    // when it's available in the Cargo.toml
    warn!("Folder selection not yet implemented - using dialog plugin");
    Ok(None)
}

// Backend selection commands

/// Get available audio capture backends for the current platform
#[tauri::command]
pub async fn get_available_audio_backends() -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let backends = crate::audio::capture::get_available_backends();
        Ok(backends.iter().map(|b| b.to_string()).collect())
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Only ScreenCaptureKit available on non-macOS
        Ok(vec!["screencapturekit".to_string()])
    }
}

/// Get current audio capture backend
#[tauri::command]
pub async fn get_current_audio_backend() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let backend = crate::audio::capture::get_current_backend();
        Ok(backend.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok("screencapturekit".to_string())
    }
}

/// Set audio capture backend
#[tauri::command]
pub async fn set_audio_backend(backend: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;
        use crate::audio::permissions::{
            check_screen_recording_permission, request_screen_recording_permission,
        };

        let backend_enum = AudioCaptureBackend::from_string(&backend)
            .ok_or_else(|| format!("Invalid backend: {}", backend))?;

        // If switching to Core Audio, log information about Audio Capture permission
        if backend_enum == AudioCaptureBackend::CoreAudio {
            info!("🔐 Core Audio backend requires Audio Capture permission (macOS 14.4+)");
            info!("📍 Permission dialog will appear automatically when recording starts");

            // Check if permission is already granted (this is informational only)
            if !check_screen_recording_permission() {
                warn!("⚠️  Audio Capture permission may not be granted");

                // Attempt to open System Settings (opens System Settings)
                if let Err(e) = request_screen_recording_permission() {
                    error!("Failed to open System Settings: {}", e);
                }

                return Err(
                    "Core Audio requires Audio Capture permission. \
                    The permission dialog will appear when you start recording. \
                    If already denied, enable it in System Settings → Privacy & Security → Audio Capture, \
                    then restart the app.".to_string()
                );
            }

            info!(
                "✅ Core Audio backend selected - permission check will occur at recording start"
            );
        }

        info!("Setting audio backend to: {:?}", backend_enum);
        crate::audio::capture::set_current_backend(backend_enum);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        if backend != "screencapturekit" {
            return Err(format!(
                "Backend {} not available on this platform",
                backend
            ));
        }
        Ok(())
    }
}

/// Get backend information (name and description)
#[derive(Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub async fn get_audio_backend_info() -> Result<Vec<BackendInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;

        let backends = vec![
            BackendInfo {
                id: AudioCaptureBackend::ScreenCaptureKit.to_string(),
                name: AudioCaptureBackend::ScreenCaptureKit.name().to_string(),
                description: AudioCaptureBackend::ScreenCaptureKit
                    .description()
                    .to_string(),
            },
            BackendInfo {
                id: AudioCaptureBackend::CoreAudio.to_string(),
                name: AudioCaptureBackend::CoreAudio.name().to_string(),
                description: AudioCaptureBackend::CoreAudio.description().to_string(),
            },
        ];
        Ok(backends)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(vec![BackendInfo {
            id: "screencapturekit".to_string(),
            name: "ScreenCaptureKit".to_string(),
            description: "Default system audio capture".to_string(),
        }])
    }
}


#[cfg(test)]
mod legacy_folder_tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("protokol-legacy-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn legacy_recordings_move_and_paths_follow() {
        let root = tmp("move");
        let legacy = root.join("meetily-recordings");
        let target = root.join("Protokol");
        std::fs::create_dir_all(legacy.join("Нарада 1")).unwrap();
        std::fs::write(legacy.join("Нарада 1/audio.mp4"), b"a").unwrap();
        // the target exists with one recording of the same name: not overwritten
        std::fs::create_dir_all(target.join("Нарада 2")).unwrap();
        std::fs::create_dir_all(legacy.join("Нарада 2")).unwrap();
        assert_eq!(move_legacy_recordings(&legacy, &target).unwrap(), 1);
        assert!(target.join("Нарада 1/audio.mp4").exists());
        assert!(legacy.join("Нарада 2").exists(), "a clash stays where it was");

        let db = root.join("m.sqlite");
        let url = db.to_string_lossy().to_string();
        use sqlx::migrate::MigrateDatabase;
        sqlx::Sqlite::create_database(&url).await.unwrap();
        let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
        sqlx::query("CREATE TABLE meetings (id TEXT, folder_path TEXT)").execute(&pool).await.unwrap();
        let old = format!("{}/Нарада 1", legacy.to_string_lossy());
        sqlx::query("INSERT INTO meetings VALUES ('a', ?), ('b', '/elsewhere/x')").bind(&old).execute(&pool).await.unwrap();
        assert_eq!(rewrite_recording_paths(&pool, &legacy, &target, Some(&db)).await.unwrap(), 1);
        let a: String = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'a'").fetch_one(&pool).await.unwrap();
        let b: String = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = 'b'").fetch_one(&pool).await.unwrap();
        assert_eq!(a, format!("{}/Нарада 1", target.to_string_lossy()));
        assert_eq!(b, "/elsewhere/x", "a chosen folder stays");
        assert!(std::fs::read_dir(&root).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains("before-recordings-folder")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn whole_legacy_folder_is_renamed_when_protokol_is_new_and_absent_one_is_never_created() {
        let root = tmp("rename");
        let legacy = root.join("meetily-recordings");
        let target = root.join("Protokol");
        std::fs::create_dir_all(legacy.join("x")).unwrap();
        assert_eq!(move_legacy_recordings(&legacy, &target).unwrap(), 1);
        assert!(!legacy.exists() && target.join("x").exists());
        // absent: nothing moves, nothing is created
        assert_eq!(move_legacy_recordings(&legacy, &target).unwrap(), 0);
        assert!(!legacy.exists());
        assert!(ensure_recordings_directory(&legacy).is_err());
        assert!(!legacy.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
