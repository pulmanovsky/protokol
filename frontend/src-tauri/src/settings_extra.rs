//! Settings 0.3.16: Загальні (login item, menu bar icon), Сховище (what takes
//! space) and Конфіденційність (the log, deleting voices and rules).

use std::path::Path;

use log::{info, warn};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_store::StoreExt;

const PREFS: &str = "preferences.json";
const TRAY_KEY: &str = "show_tray_icon";

// ===== Загальні → «Відкривати Протокол під час входу в систему» =====

#[cfg(target_os = "macos")]
mod login_item {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    fn service() -> Option<*mut AnyObject> {
        let class = AnyClass::get(c"SMAppService")?;
        let s: *mut AnyObject = unsafe { msg_send![class, mainAppService] };
        (!s.is_null()).then_some(s)
    }

    /// SMAppServiceStatus: 0 not registered, 1 enabled, 2 requires approval, 3 not found
    pub fn status() -> isize {
        match service() {
            Some(s) => unsafe { msg_send![s, status] },
            None => 3,
        }
    }

    pub fn set(on: bool) -> Result<(), String> {
        let s = service().ok_or("Ця версія macOS не підтримує автозапуск програм.")?;
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let ok: bool = unsafe {
            if on {
                msg_send![s, registerAndReturnError: &mut err]
            } else {
                msg_send![s, unregisterAndReturnError: &mut err]
            }
        };
        if ok {
            Ok(())
        } else {
            let text = if err.is_null() {
                String::new()
            } else {
                unsafe {
                    let d: *mut AnyObject = msg_send![err, localizedDescription];
                    let u: *const std::os::raw::c_char = msg_send![d, UTF8String];
                    std::ffi::CStr::from_ptr(u).to_string_lossy().into_owned()
                }
            };
            Err(format!("macOS не змінив автозапуск: {text}"))
        }
    }
}

#[derive(Serialize)]
pub struct LoginItem {
    pub on: bool,
    /// macOS waits for the person to allow it in System Settings → Login Items
    pub needs_approval: bool,
}

#[tauri::command]
pub async fn api_login_item_get() -> LoginItem {
    #[cfg(target_os = "macos")]
    {
        let s = login_item::status();
        LoginItem { on: s == 1 || s == 2, needs_approval: s == 2 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        LoginItem { on: false, needs_approval: false }
    }
}

#[tauri::command]
pub async fn api_login_item_set(on: bool) -> Result<LoginItem, String> {
    #[cfg(target_os = "macos")]
    {
        login_item::set(on)?;
        info!("Login item {}", if on { "on" } else { "off" });
    }
    let _ = on;
    Ok(api_login_item_get().await)
}

// ===== Загальні → «Показувати значок у рядку меню» =====

pub fn tray_visible<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.store(PREFS).ok().and_then(|s| s.get(TRAY_KEY)).and_then(|v| v.as_bool()).unwrap_or(true)
}

/// Shows or hides the menu bar icon as chosen (at start and on change)
pub fn apply_tray<R: Runtime>(app: &AppHandle<R>) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        if let Err(e) = tray.set_visible(tray_visible(app)) {
            warn!("Menu bar icon not changed: {e}");
        }
    }
}

#[tauri::command]
pub async fn api_tray_visible_get<R: Runtime>(app: AppHandle<R>) -> bool {
    tray_visible(&app)
}

#[tauri::command]
pub async fn api_tray_visible_set<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    let store = app.store(PREFS).map_err(|e| e.to_string())?;
    store.set(TRAY_KEY, on);
    store.save().map_err(|e| e.to_string())?;
    apply_tray(&app);
    Ok(())
}

// ===== Сховище =====

#[derive(Serialize, Default)]
pub struct StorageUsage {
    /// The recordings folder
    pub audio: u64,
    /// Models (speech, document, speakers)
    pub models: u64,
    /// Database, transcripts, dictionary and the rest of the app's data
    pub data: u64,
    /// Free space on the disk with the app's data
    pub free: Option<u64>,
}

/// Bytes in a folder; symbolic links are not followed
fn folder_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|e| folder_size(&e.path())).sum()
}

fn free_space(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df").args(["-k"]).arg(path).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let kb: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb * 1024)
}

#[tauri::command]
pub async fn api_storage_usage<R: Runtime>(app: AppHandle<R>) -> Result<StorageUsage, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let recordings = crate::audio::recording_preferences::recordings_folder();
    tokio::task::spawn_blocking(move || {
        let models = folder_size(&data_dir.join("models"));
        let all = folder_size(&data_dir);
        StorageUsage {
            audio: folder_size(&recordings),
            models,
            data: all.saturating_sub(models),
            free: free_space(&data_dir),
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// Сховище → «Папка моделей»: where the models are
#[tauri::command]
pub async fn api_models_folder<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn api_models_reveal<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("open").arg(&dir).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ===== Конфіденційність =====

fn log_dir() -> Option<std::path::PathBuf> {
    Some(dirs::home_dir()?.join("Library/Logs").join(crate::identity::IDENTIFIER))
}

/// Shows a folder in Finder. The app's own folders are named after its
/// identifier («ua.protokol.app»), which macOS takes for an application:
/// `open <folder>` silently tries to launch it. Revealing a file inside
/// (`open -R`) shows the folder in Finder instead.
pub fn reveal_in_finder(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let inside = std::fs::read_dir(dir).ok().and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path())));
    let mut cmd = std::process::Command::new("open");
    match inside {
        Some(item) => cmd.arg("-R").arg(item),
        None => cmd.arg(dir),
    };
    let status = cmd.status().map_err(|e| e.to_string())?;
    if status.success() {
        info!("Shown in Finder: {:?}", dir);
        Ok(())
    } else {
        Err("Не вдалося відкрити папку у Finder".to_string())
    }
}

/// «Показати у Finder» for the log folder (the log file is selected)
#[tauri::command]
pub async fn api_log_reveal() -> Result<(), String> {
    let dir = log_dir().ok_or("Немає домашньої папки")?;
    let file = dir.join("protokol.log");
    if file.exists() {
        let status = std::process::Command::new("open").arg("-R").arg(&file).status().map_err(|e| e.to_string())?;
        if status.success() {
            info!("Log shown in Finder: {:?}", file);
            return Ok(());
        }
    }
    reveal_in_finder(&dir)
}

/// The log's size in bytes
#[tauri::command]
pub async fn api_log_size() -> u64 {
    log_dir().map(|d| folder_size(&d)).unwrap_or(0)
}

/// «Очистити…»: the log files are emptied (the app keeps writing to them)
#[tauri::command]
pub async fn api_log_clear() -> Result<(), String> {
    let dir = log_dir().ok_or("Немає домашньої папки")?;
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(()) };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("log") {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
                f.set_len(0).map_err(|e| e.to_string())?;
            }
        }
    }
    info!("Log cleared");
    Ok(())
}

/// «Голосові відбитки — Видалити всі…»: names stay, voices go; returns how many
#[tauri::command]
pub async fn api_voice_profiles_delete_all<R: Runtime>(app: AppHandle<R>) -> Result<u64, String> {
    let pool = crate::speakers::commands::pool(&app).map_err(|e| e.to_string())?;
    let done = sqlx::query("DELETE FROM voice_profiles").execute(&pool).await.map_err(|e| e.to_string())?;
    info!("Voice profiles deleted: {}", done.rows_affected());
    Ok(done.rows_affected())
}

/// «Пам'ять виправлень — Очистити…»: the learned rules go, the dictionary stays
#[tauri::command]
pub async fn api_corrections_clear() -> Result<usize, String> {
    let all = crate::ukrainian::corrections::all_corrections();
    for c in &all {
        crate::ukrainian::corrections::remove_correction(&c.wrong)?;
    }
    info!("Correction rules cleared: {}", all.len());
    Ok(all.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_size_counts_files_and_skips_links() {
        let dir = std::env::temp_dir().join(format!("protokol-size-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("sub/b.bin"), vec![0u8; 500]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("a.bin"), dir.join("link.bin")).unwrap();
        assert_eq!(folder_size(&dir), 1500);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
