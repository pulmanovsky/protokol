//! The macOS menu bar, in Ukrainian and in the order Mac apps have: the
//! app's own menu (About, Check for Updates, Settings ⌘,, Hide, Quit), File,
//! Edit (the system's own Undo/Redo/Cut/Copy/Paste, so they work in every text
//! field and ⌘Z still reaches the transcript's history), Recording (as Music's
//! Controls), View, Window and Help. The app's name is declined as Apple does
//! («Вийти з Нотаток»): «Сховати Протокол», «Вийти з Протоколу».

use tauri::menu::{Menu, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Manager, Runtime};

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let item = |id: &str, text: &str, key: Option<&str>| {
        let b = MenuItemBuilder::with_id(id, text);
        match key {
            Some(k) => b.accelerator(k).build(app),
            None => b.build(app),
        }
    };
    let app_menu = SubmenuBuilder::new(app, "Протокол")
        .item(&item("menu_about", "Про Протокол", None)?)
        .item(&item("menu_check_updates", "Перевірити оновлення…", None)?)
        .separator()
        .item(&item("menu_settings", "Налаштування…", Some("CmdOrCtrl+,"))?)
        .separator()
        .item(&PredefinedMenuItem::services(app, Some("Служби"))?)
        .separator()
        .item(&PredefinedMenuItem::hide(app, Some("Сховати Протокол"))?)
        .item(&PredefinedMenuItem::hide_others(app, Some("Сховати інші"))?)
        .item(&PredefinedMenuItem::show_all(app, Some("Показати всі"))?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some("Вийти з Протоколу"))?)
        .build()?;
    let file = SubmenuBuilder::new(app, "Файл")
        .item(&item("menu_import", "Імпортувати аудіо…", Some("CmdOrCtrl+O"))?)
        .separator()
        .item(&item("menu_export", "Експортувати документ…", Some("CmdOrCtrl+Shift+E"))?)
        .separator()
        .item(&PredefinedMenuItem::close_window(app, Some("Закрити вікно"))?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Правка")
        .item(&PredefinedMenuItem::undo(app, Some("Скасувати"))?)
        .item(&PredefinedMenuItem::redo(app, Some("Повторити"))?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, Some("Вирізати"))?)
        .item(&PredefinedMenuItem::copy(app, Some("Копіювати"))?)
        .item(&PredefinedMenuItem::paste(app, Some("Вставити"))?)
        .item(&PredefinedMenuItem::select_all(app, Some("Вибрати все"))?)
        .separator()
        .item(&item("menu_find", "Знайти…", Some("CmdOrCtrl+F"))?)
        .build()?;
    let recording = SubmenuBuilder::new(app, "Запис")
        .item(&item("menu_record_new", "Новий запис", Some("CmdOrCtrl+N"))?)
        .item(&item("menu_record_pause", "Пауза або продовження", None)?)
        .item(&item("menu_record_stop", "Завершити запис", None)?)
        .separator()
        .item(&item("menu_mic", "Вимкнути або увімкнути мікрофон", Some("CmdOrCtrl+Shift+M"))?)
        .build()?;
    let view = SubmenuBuilder::new(app, "Вигляд")
        .item(&item("menu_sidebar", "Показати або сховати бічну панель", Some("Cmd+Ctrl+S"))?)
        .separator()
        .item(&item("menu_tab_transcript", "Стенограма", Some("CmdOrCtrl+1"))?)
        .item(&item("menu_tab_document", "Документ", Some("CmdOrCtrl+2"))?)
        .separator()
        .item(&item("menu_text_bigger", "Більший текст", Some("CmdOrCtrl+Equal"))?)
        .item(&item("menu_text_smaller", "Менший текст", Some("CmdOrCtrl+Minus"))?)
        .item(&item("menu_text_normal", "Звичайний розмір тексту", Some("CmdOrCtrl+0"))?)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, Some("На весь екран"))?)
        .build()?;
    let window = SubmenuBuilder::new(app, "Вікно")
        .item(&PredefinedMenuItem::minimize(app, Some("Згорнути"))?)
        .item(&PredefinedMenuItem::maximize(app, Some("Масштабувати"))?)
        .build()?;
    let help = SubmenuBuilder::new(app, "Довідка")
        .item(&item("menu_privacy", "Приватність і дані", None)?)
        .separator()
        .item(&item("menu_report", "Повідомити про проблему…", None)?)
        .item(&item("menu_suggest", "Запропонувати покращення…", None)?)
        .separator()
        .item(&item("menu_github", "Сторінка проєкту на GitHub", None)?)
        .build()?;
    Menu::with_items(app, &[&app_menu, &file, &edit, &recording, &view, &window, &help])
}

const PROJECT: &str = "https://github.com/pulmanovsky/protokol";

/// What the menu's own items do (the system's items work by themselves)
pub fn on_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    // Recording: the same paths as the menu bar icon
    match id {
        "menu_record_new" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if !crate::is_recording().await {
                    crate::tray::handle_menu_event(&app, "toggle_recording");
                } else if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            });
            return;
        }
        "menu_record_pause" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if !crate::is_recording().await {
                    return;
                }
                let paused = crate::audio::recording_commands::is_recording_paused().await;
                crate::tray::handle_menu_event(&app, if paused { "resume_recording" } else { "pause_recording" });
            });
            return;
        }
        "menu_record_stop" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if crate::is_recording().await {
                    crate::tray::handle_menu_event(&app, "stop_recording");
                }
            });
            return;
        }
        "menu_mic" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if !crate::is_recording().await {
                    return;
                }
                let muted = crate::audio::recording_commands::get_mic_muted().await;
                let _ = crate::audio::recording_commands::set_mic_muted(app, !muted).await;
            });
            return;
        }
        "menu_privacy" | "menu_github" => {
            let url = if id == "menu_github" { PROJECT.to_string() } else { format!("{PROJECT}/blob/main/PRIVACY_POLICY.md") };
            let _ = std::process::Command::new("open").arg(url).spawn();
            return;
        }
        _ => {}
    }
    let Some(window) = app.get_webview_window("main") else { return };
    let _ = window.show();
    let _ = window.set_focus();
    let js = match id {
        "menu_settings" => "window.location.assign('/settings')",
        "menu_about" => "window.location.assign('/settings?section=about')",
        "menu_check_updates" => "window.location.assign('/settings?section=about&check=1')",
        "menu_import" => "window.dispatchEvent(new CustomEvent('protokol:menu-import'))",
        "menu_report" => "window.dispatchEvent(new CustomEvent('protokol:feedback', { detail: 'problem' }))",
        "menu_suggest" => "window.dispatchEvent(new CustomEvent('protokol:feedback', { detail: 'idea' }))",
        "menu_export" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'export' }))",
        "menu_find" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'find' }))",
        "menu_sidebar" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'sidebar' }))",
        "menu_tab_transcript" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'tab-transcript' }))",
        "menu_tab_document" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'tab-document' }))",
        "menu_text_bigger" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'text-bigger' }))",
        "menu_text_smaller" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'text-smaller' }))",
        "menu_text_normal" => "window.dispatchEvent(new CustomEvent('protokol:menu', { detail: 'text-normal' }))",
        _ => return,
    };
    let _ = window.eval(js);
}

/// A native «are you sure?» (the web view's own `confirm()` shows nothing and says yes)
#[tauri::command]
pub async fn api_confirm<R: Runtime>(app: AppHandle<R>, title: String, message: String, ok: String, cancel: String) -> bool {
    // Test runs only: the answer without a system dialog
    #[cfg(feature = "test-hooks")]
    if let Ok(a) = std::env::var("PROTOKOL_TEST_CONFIRM") {
        log::info!("api_confirm «{}»: {} (test answer)", title, a);
        return a == "yes";
    }
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let mut builder = app.dialog().message(message).title(title.clone()).kind(MessageDialogKind::Warning).buttons(MessageDialogButtons::OkCancelCustom(ok, cancel));
    if let Some(window) = app.get_webview_window("main") {
        builder = builder.parent(&window);
    }
    let answer = tauri::async_runtime::spawn_blocking(move || builder.blocking_show()).await.unwrap_or(false);
    log::info!("api_confirm «{}»: {}", title, if answer { "yes" } else { "no" });
    answer
}

/// A folder chosen in the system's panel (None: cancelled)
#[tauri::command]
pub async fn api_pick_folder<R: Runtime>(app: AppHandle<R>, title: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let builder = app.dialog().file().set_title(title);
    tauri::async_runtime::spawn_blocking(move || builder.blocking_pick_folder()).await.ok().flatten().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().to_string())
}

/// What an offline models folder may hold, and where each goes in the app's data
fn model_target(name: &str) -> Option<&'static str> {
    match name {
        n if n.starts_with("ggml-") && n.ends_with(".bin") => Some(""),
        "segmentation-3.0.onnx" | "voice-embedding.onnx" => Some("speakers"),
        n if n.ends_with(".gguf") => Some("summary"),
        "w2v-uk-int8.onnx" | "vocab.json" => Some("aligner"),
        _ => None,
    }
}

#[derive(Clone, serde::Serialize)]
pub struct ImportedModels {
    pub copied: Vec<String>,
    pub skipped: Vec<String>,
}

/// Models from a folder (a USB stick, a shared drive): for Macs without the
/// internet. Known files are copied into the app's models folder (on the same
/// disk, copied without taking room twice); files already there are skipped.
#[tauri::command]
pub async fn api_import_models<R: Runtime>(app: AppHandle<R>, folder: String) -> Result<ImportedModels, String> {
    let models = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    tauri::async_runtime::spawn_blocking(move || import_models(std::path::Path::new(&folder), &models)).await.map_err(|e| e.to_string())?
}

pub(crate) fn import_models(from: &std::path::Path, models: &std::path::Path) -> Result<ImportedModels, String> {
    let mut found: Vec<std::path::PathBuf> = Vec::new();
    let mut walk = vec![(from.to_path_buf(), 0)];
    while let Some((dir, depth)) = walk.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("Не вдалося відкрити папку: {e}"))?.flatten() {
            let p = entry.path();
            if p.is_dir() && depth < 2 {
                walk.push((p, depth + 1));
            } else if p.is_file() && p.file_name().and_then(|n| n.to_str()).and_then(model_target).is_some() {
                found.push(p);
            }
        }
    }
    if found.is_empty() {
        return Err("У папці немає моделей «Протоколу»".into());
    }
    let (mut copied, mut skipped) = (Vec::new(), Vec::new());
    for p in found {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let dest_dir = models.join(model_target(&name).unwrap_or_default());
        std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
        let dest = dest_dir.join(&name);
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if std::fs::metadata(&dest).map(|m| m.len() == size).unwrap_or(false) {
            skipped.push(name);
            continue;
        }
        // Copied beside, then put in place: a half-copied model is never used
        let part = dest_dir.join(format!(".{name}.part"));
        std::fs::copy(&p, &part).map_err(|e| format!("Не вдалося скопіювати {name}: {e}"))?;
        std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
        copied.push(name);
    }
    log::info!("Models from a folder: {} copied, {} already there", copied.len(), skipped.len());
    Ok(ImportedModels { copied, skipped })
}

#[cfg(test)]
mod model_import_tests {
    #[test]
    fn known_models_go_to_their_places_and_others_are_left() {
        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(from.path().join("pack/speakers")).unwrap();
        std::fs::write(from.path().join("pack/ggml-large-v3.bin"), b"w").unwrap();
        std::fs::write(from.path().join("pack/speakers/voice-embedding.onnx"), b"v").unwrap();
        std::fs::write(from.path().join("pack/Model.Q4_K_M.gguf"), b"g").unwrap();
        std::fs::write(from.path().join("pack/notes.txt"), b"n").unwrap();
        let r = super::import_models(from.path(), to.path()).unwrap();
        assert_eq!(r.copied.len(), 3);
        assert!(to.path().join("ggml-large-v3.bin").is_file());
        assert!(to.path().join("speakers/voice-embedding.onnx").is_file());
        assert!(to.path().join("summary/Model.Q4_K_M.gguf").is_file());
        assert!(!to.path().join("notes.txt").exists());
        // Again: nothing to copy
        let again = super::import_models(from.path(), to.path()).unwrap();
        assert_eq!((again.copied.len(), again.skipped.len()), (0, 3));
        assert!(super::import_models(to.path().join("speakers").as_path(), tempfile::tempdir().unwrap().path()).is_ok());
    }
}
