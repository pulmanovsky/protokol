//! The document model on Macs with 24 GB or more: Gemma 4 26B.
//!
//! In a blind test on four meetings Gemma 4 summarised speakers as well as
//! MamayLM, wrote the overall summary better (4 of 4) and was 2.4 times
//! faster, at ~14 GB of memory instead of ~7.5 GB. So Macs with 24 GB or more
//! use it; smaller Macs keep MamayLM, which also stays as Gemma 4's fallback
//! (client.rs).
//!
//! On such a Mac, once: a document model still set to MamayLM becomes Gemma 4
//! (a marker file keeps a later choice of MamayLM in Settings from being
//! undone). Then Gemma 4 is downloaded in the background if there is room;
//! until it is in, MamayLM writes the documents.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, Runtime};

use super::models::{self, GEMMA4_MODEL, MAMAYLM_MODEL};
use crate::database::repositories::setting::SettingsRepository;

/// Present once the switch to Gemma 4 was made (or found unnecessary) on this Mac
const MARKER: &str = ".gemma4-preferred";

/// Free disk space to leave beside the downloaded model
const DISK_HEADROOM_MB: u64 = 10 * 1024;

fn marker_path(app_data_dir: &Path) -> PathBuf {
    models::get_models_directory(&app_data_dir.to_path_buf()).join(MARKER)
}

/// Whether this Mac should write documents with Gemma 4
pub fn gemma4_preferred_here() -> bool {
    let ram = super::commands::get_system_ram_gb().unwrap_or(0);
    super::commands::recommend_summary_model(cfg!(target_os = "macos"), ram) == GEMMA4_MODEL
}

/// The model a saved choice becomes: MamayLM turns into Gemma 4 once, on a Mac
/// that has room for it; anything else stays as it is
fn switched_model(provider: &str, model: &str, preferred_here: bool, switched_before: bool) -> Option<&'static str> {
    (preferred_here && !switched_before && provider == "builtin-ai" && model == MAMAYLM_MODEL).then_some(GEMMA4_MODEL)
}

/// Switch a saved MamayLM choice to Gemma 4, once, on a Mac with room for it.
/// Returns the model to use now when it was switched.
pub async fn apply_once(pool: &SqlitePool, app_data_dir: &Path) -> Option<&'static str> {
    if !gemma4_preferred_here() {
        return None;
    }
    let marker = marker_path(app_data_dir);
    if marker.exists() {
        return None;
    }
    let config = match SettingsRepository::get_model_config(pool).await {
        Ok(Some(c)) => c,
        // Nothing saved yet: onboarding picks the recommended model itself
        _ => return None,
    };
    let switched = switched_model(&config.provider, &config.model, true, false);
    if let Some(model) = switched {
        if let Err(e) = SettingsRepository::save_model_config(
            pool,
            &config.provider,
            model,
            &config.whisper_model,
            config.ollama_endpoint.as_deref(),
        )
        .await
        {
            log::error!("Could not switch the document model to {}: {}", model, e);
            return None;
        }
        log::info!("Document model switched from {} to {} (Mac with 24 GB or more)", MAMAYLM_MODEL, model);
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&marker, b"20.11: Gemma 4 for the document on Macs with 24 GB or more\n") {
        log::warn!("Could not write {}: {}", marker.display(), e);
    }
    switched
}

/// Free space on the disk that holds `dir`, in MiB
fn free_disk_mb(dir: &Path) -> Option<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space() / (1024 * 1024))
}

fn room_for(model_mb: u64, free_mb: u64) -> bool {
    free_mb >= model_mb + DISK_HEADROOM_MB
}

/// At startup: the one-time switch, then Gemma 4's download in the background
/// when it is the chosen model and is not in yet
pub async fn at_startup<R: Runtime>(app: AppHandle<R>) {
    if !gemma4_preferred_here() {
        return;
    }
    let Ok(app_data_dir) = app.path().app_data_dir() else { return };
    // The database is opened by the window's first calls; wait for it a while
    let mut pool = None;
    for _ in 0..120 {
        if let Some(state) = app.try_state::<crate::state::AppState>() {
            pool = Some(state.db_manager.pool().clone());
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let Some(pool) = pool else { return };
    apply_once(&pool, &app_data_dir).await;

    let chosen = match SettingsRepository::get_model_config(&pool).await {
        Ok(Some(c)) if c.provider == "builtin-ai" => c.model,
        _ => return,
    };
    if chosen != GEMMA4_MODEL {
        return;
    }
    let Some(def) = models::get_model_by_name(GEMMA4_MODEL) else { return };
    let models_dir = models::get_models_directory(&app_data_dir);
    if models::model_file_complete(&app_data_dir, GEMMA4_MODEL) {
        return;
    }
    let _ = std::fs::create_dir_all(&models_dir);
    match free_disk_mb(&models_dir) {
        Some(free) if room_for(def.size_mb, free) => {}
        Some(free) => {
            log::warn!("DISK-LOW Gemma 4 is not downloaded: {} MB free, {} MB needed; MamayLM keeps writing documents", free, def.size_mb + DISK_HEADROOM_MB);
            return;
        }
        None => {
            log::warn!("Free disk space unknown; Gemma 4 is not downloaded automatically");
            return;
        }
    }
    // Let the app settle (startup work, a recording started right away) first
    tokio::time::sleep(Duration::from_secs(60)).await;
    log::info!("Downloading Gemma 4 for the document in the background");
    let state = app.state::<super::commands::ModelManagerState>();
    if let Err(e) = super::commands::builtin_ai_download_model(app.clone(), state, GEMMA4_MODEL.to_string()).await {
        log::warn!("Gemma 4 download did not finish: {}; MamayLM keeps writing documents", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_saved_mamaylm_switches_and_only_once() {
        assert_eq!(switched_model("builtin-ai", MAMAYLM_MODEL, true, false), Some(GEMMA4_MODEL));
        assert_eq!(switched_model("builtin-ai", MAMAYLM_MODEL, true, true), None);
        assert_eq!(switched_model("builtin-ai", MAMAYLM_MODEL, false, false), None);
        assert_eq!(switched_model("builtin-ai", "gemma3:12b", true, false), None);
        assert_eq!(switched_model("ollama", MAMAYLM_MODEL, true, false), None);
    }

    #[test]
    fn download_needs_room_beside_the_model() {
        assert!(room_for(14_000, 30_000));
        assert!(!room_for(14_000, 20_000));
    }
}
