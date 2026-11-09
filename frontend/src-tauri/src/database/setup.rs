use log::info;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use crate::state::AppState;

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch {
        info!("First launch detected - will notify window when ready");

        // Delay event emission to ensure window is ready and React listeners are registered
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            app_handle
                .emit("first-launch-detected", ())
                .expect("Failed to emit first-launch-detected event");
            info!("Emitted first-launch-detected after delay");
        });
    } else {
        // Normal flow - initialize database immediately
        let db_manager = DatabaseManager::new_from_app_handle(app)
            .await
            .map_err(|e| format!("Failed to initialize database manager: {}", e))?;

        // A document left half-made by the last quit or crash: shown as
        // interrupted, the previous version back (it would wait for ever)
        match crate::database::repositories::summary::SummaryProcessesRepository::reset_interrupted(db_manager.pool()).await {
            Ok(n) if n > 0 => log::info!("Documents interrupted by the last quit: {n}"),
            Ok(_) => {}
            Err(e) => log::warn!("Could not reset interrupted documents: {e}"),
        }

        // Older installs kept cloud API keys in SQLite: move them to the Keychain
        if let Err(e) = crate::database::repositories::setting::SettingsRepository::migrate_cloud_keys_to_keychain(db_manager.pool()).await {
            log::warn!("Failed to migrate API keys to the Keychain: {}", e);
        }

        app.manage(AppState { db_manager });
        info!("Database initialized successfully");
    }

    Ok(())
}
