//! Floating recording island: a small always-on-top capsule shown while a
//! recording runs and the main window is not in front.
//!
//! - Visible on every Space and over full-screen apps, not in the Dock.
//! - Excluded from screen sharing and screenshots (content protection), so it
//!   never shows up in a Zoom / Meet / Teams screen share.
//! - Opens on `recording-started`. After `recording-stopped` it stays for the
//!   processing («Обробка запису», then «Стенограма готова») and the page
//!   closes it with `island_action("close")`. Hides while the main window is
//!   focused.
//! - The page measures its capsule and sets the window width with
//!   `island_set_width`, keeping the edge the island is snapped to.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Listener, LogicalPosition, LogicalSize, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

pub const ISLAND_LABEL: &str = "island";
pub const ISLAND_WIDTH: f64 = 372.0;
pub const ISLAND_HEIGHT: f64 = 44.0;
const MIN_WIDTH: f64 = 44.0;
const MAX_WIDTH: f64 = 560.0;
const EDGE_MARGIN: f64 = 16.0;

static RECORDING: AtomicBool = AtomicBool::new(false);
/// Between «Завершити» and «Стенограма готова»
static AFTER_RECORDING: AtomicBool = AtomicBool::new(false);
static MAIN_FOCUSED: AtomicBool = AtomicBool::new(true);

/// Wires the island to the recording lifecycle. Call once from `setup`.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    app.listen("recording-started", move |_| {
        RECORDING.store(true, Ordering::SeqCst);
        AFTER_RECORDING.store(false, Ordering::SeqCst);
        sync(&handle);
    });
    let handle = app.clone();
    app.listen("recording-stopped", move |_| {
        RECORDING.store(false, Ordering::SeqCst);
        AFTER_RECORDING.store(true, Ordering::SeqCst);
        sync(&handle);
    });
}

/// Main window focus changed (from `on_window_event`)
pub fn on_main_focus<R: Runtime>(app: &AppHandle<R>, focused: bool) {
    MAIN_FOCUSED.store(focused, Ordering::SeqCst);
    sync(app);
}

/// Show the island while recording (or processing it) and the main window is
/// not in front
fn sync<R: Runtime>(app: &AppHandle<R>) {
    let active = RECORDING.load(Ordering::SeqCst) || AFTER_RECORDING.load(Ordering::SeqCst);
    let should_show = active && !main_in_front(app);
    match (app.get_webview_window(ISLAND_LABEL), should_show) {
        (Some(window), true) => {
            let _ = window.show();
        }
        (Some(window), false) => {
            let _ = window.hide();
        }
        (None, true) if RECORDING.load(Ordering::SeqCst) => {
            if let Err(e) = create(app) {
                log::error!("Failed to create the recording island: {e}");
            }
        }
        (None, _) => {}
    }
}

fn main_in_front<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(main) = app.get_webview_window("main") else { return false };
    let visible = main.is_visible().unwrap_or(false);
    let minimized = main.is_minimized().unwrap_or(false);
    visible && !minimized && MAIN_FOCUSED.load(Ordering::SeqCst)
}

fn close<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(ISLAND_LABEL) {
        let _ = window.close();
    }
}

fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let builder = WebviewWindowBuilder::new(app, ISLAND_LABEL, WebviewUrl::App("island".into()));
    // macOS: the system material behind the transparent page, rounded to the
    // capsule. It follows the window appearance, which the page keeps in step
    // with the app theme. Set before the page loads and asks for it.
    #[cfg(target_os = "macos")]
    let builder = {
        use tauri::window::{Effect, EffectState, EffectsBuilder};
        NATIVE_GLASS.store(true, Ordering::SeqCst);
        builder.effects(
            EffectsBuilder::new()
                .effect(Effect::Popover)
                .state(EffectState::Active)
                .radius(ISLAND_HEIGHT / 2.0)
                .build(),
        )
    };
    let window = builder
        .title("Протокол — запис")
        .inner_size(ISLAND_WIDTH, ISLAND_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .accept_first_mouse(true)
        .content_protected(!dev_capture_allowed())
        .visible(false)
        .build()?;

    // Default spot: right edge, vertical centre (the page restores a saved spot)
    if let Ok(Some(monitor)) = window.current_monitor().or_else(|_| window.primary_monitor()) {
        let scale = monitor.scale_factor();
        let size = monitor.size().to_logical::<f64>(scale);
        let origin = monitor.position().to_logical::<f64>(scale);
        let x = origin.x + size.width - ISLAND_WIDTH - EDGE_MARGIN;
        let y = origin.y + (size.height - ISLAND_HEIGHT) / 2.0;
        let _ = window.set_position(LogicalPosition::new(x, y));
    }

    #[cfg(target_os = "macos")]
    configure_macos(&window);

    window.show()?;
    Ok(())
}

/// Floating level, every Space, over full-screen apps, ignored by Cmd-Tab / Exposé
#[cfg(target_os = "macos")]
fn configure_macos<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    use objc::{msg_send, sel, sel_impl};
    // NSWindowCollectionBehavior
    const CAN_JOIN_ALL_SPACES: u64 = 1 << 0;
    const STATIONARY: u64 = 1 << 4;
    const IGNORES_CYCLE: u64 = 1 << 6;
    const FULL_SCREEN_AUXILIARY: u64 = 1 << 8;
    // NSStatusWindowLevel keeps it above full-screen apps
    const STATUS_WINDOW_LEVEL: i64 = 25;

    let Ok(ns_window) = window.ns_window() else { return };
    let ns_window = ns_window as *mut objc::runtime::Object;
    let behavior = CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY;
    let _ = window.run_on_main_thread({
        let ptr = ns_window as usize;
        move || unsafe {
            let ns_window = ptr as *mut objc::runtime::Object;
            let _: () = msg_send![ns_window, setCollectionBehavior: behavior];
            let _: () = msg_send![ns_window, setLevel: STATUS_WINDOW_LEVEL];
            let _: () = msg_send![ns_window, setHidesOnDeactivate: false];
        }
    });
}

// ---------------------------------------------------------------------------
// Commands used by the island page
// ---------------------------------------------------------------------------

/// Brings the main window to the front
#[tauri::command]
pub fn island_focus_main<R: Runtime>(app: AppHandle<R>) {
    crate::tray::focus_main_window(&app);
}

/// Pause, resume or stop, through the same handlers as the menu bar icon;
/// `close` ends the «Стенограма готова» state
#[tauri::command]
pub fn island_action<R: Runtime>(app: AppHandle<R>, action: String) -> Result<(), String> {
    if action == "close" {
        AFTER_RECORDING.store(false, Ordering::SeqCst);
        if !RECORDING.load(Ordering::SeqCst) {
            close(&app);
        }
        return Ok(());
    }
    let id = match action.as_str() {
        "pause" => "pause_recording",
        "resume" => "resume_recording",
        "stop" => "stop_recording",
        other => return Err(format!("Unknown island action: {other}")),
    };
    crate::tray::handle_menu_event(&app, id);
    Ok(())
}

/// Fits the window to the capsule. With `anchor: "right"` the right edge
/// stays where it is, so an island snapped to the right edge grows leftwards.
#[tauri::command]
pub fn island_set_width<R: Runtime>(app: AppHandle<R>, width: f64, anchor: Option<String>) -> Result<(), String> {
    let window = app.get_webview_window(ISLAND_LABEL).ok_or("No island")?;
    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
    if anchor.as_deref() == Some("right") {
        let scale = window.scale_factor().map_err(|e| e.to_string())?;
        let pos = window.outer_position().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
        let size = window.outer_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
        let right = pos.x + size.width;
        if (size.width - width).abs() >= 0.5 {
            window.set_position(LogicalPosition::new(right - width, pos.y)).map_err(|e| e.to_string())?;
        }
    }
    window.set_size(LogicalSize::new(width, ISLAND_HEIGHT)).map_err(|e| e.to_string())
}

/// Whether the window has the native macOS glass behind the page. The page
/// draws its own frosted fill when it does not.
#[tauri::command]
pub fn island_has_native_glass() -> bool {
    NATIVE_GLASS.load(Ordering::SeqCst)
}

/// Set on macOS, where the island window is built with the system material
pub static NATIVE_GLASS: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Hide the main window from screen sharing (setting, default on)
// ---------------------------------------------------------------------------

/// The main window is hidden from screen capture only when the user turns
/// this on (the recording island always is). A new key: the old one was
/// written on every start, so it could not tell a choice from the default.
pub const HIDE_FROM_SCREEN_SHARE_FLAG: &str = "hide_main_window_from_capture";

/// Applies the setting to the main window. `PROTOKOL_ALLOW_SCREEN_CAPTURE=1`
/// turns it off for development screenshots.
fn dev_capture_allowed() -> bool {
    std::env::var("PROTOKOL_ALLOW_SCREEN_CAPTURE").as_deref() == Ok("1")
}

pub fn apply_content_protection<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let enabled = enabled && !dev_capture_allowed();
    if let Some(main) = app.get_webview_window("main") {
        if let Err(e) = main.set_content_protected(enabled) {
            log::warn!("Failed to set content protection: {e}");
        }
    }
}
