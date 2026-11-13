//! Google Meet in Protokol's own window.
//!
//! Opens a meeting in a separate window (WebKit, like Safari) and runs
//! `capture.js` inside the Meet page. The window holds three webviews:
//! Google's page (`meet-page`), Protokol's own toolbar on top of it
//! (`meet-bar`, an app page: recording capsule, the call, «Протокол»; while no
//! meeting is chosen it covers the whole window as the start screen) and a
//! small popover under the toolbar for the start/stop offers (`meet-pop`).
//! The toolbar and the popover are the app's pages: the Meet page cannot see
//! or press them. The script only reads what the page
//! plays and shows: the participants' audio streams and their loudness every
//! 100 ms, which tiles are lit as speaking, the tile names. Every 2 s it hands
//! a report to the `meetprobe` plugin, which appends it to
//! `<app data>/meet/<window opened>.jsonl` on this Mac. Nothing is sent
//! anywhere else and the recording itself is not touched: after a recording,
//! speaker recognition reads these reports to tell the call's voices apart by
//! the Meet participant who was speaking (see `speakers::meet`).
//!
//! The script also reads whether Meet's own microphone button is on, and the
//! recording then takes the secretary's microphone only while it is
//! (Settings → Google Meet, see `live`).
//!
//! The Meet page is Google's code, so it gets nothing but these two commands:
//! the `meet-probe` capability allows only `plugin:meetprobe|push` and
//! `plugin:meetprobe|mic`, only from https://meet.google.com, only in the
//! `meet` window. The reply says only whether a recording runs and whether the
//! microphone is left out of it, for the pill the script shows in the page.
//! The window shows only Google's own sites; links Meet opens in a new window
//! go to the default browser.

pub mod frames;
pub mod live;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use log::{info, warn};
use once_cell::sync::Lazy;
use serde::Serialize;
use sqlx::SqlitePool;
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, Url, Webview, WebviewBuilder, WebviewUrl, Window};

/// The window
pub const MEET_LABEL: &str = "meet";
/// Google's page in it
pub const PAGE_LABEL: &str = "meet-page";
/// Protokol's toolbar (and the start screen)
pub const BAR_LABEL: &str = "meet-bar";
/// The offer under the toolbar's capsule
pub const POP_LABEL: &str = "meet-pop";
/// Toolbar height; the window buttons are centred in it
pub const BAR_HEIGHT: f64 = 52.0;
const POP_WIDTH: f64 = 340.0;
const POP_HEIGHT: f64 = 150.0;
/// The popover's right edge, from the window's right edge (under the capsule)
const POP_RIGHT: f64 = 96.0;
/// The start screen covers the window until a meeting is chosen
static START_SCREEN: AtomicBool = AtomicBool::new(false);
static POP_SHOWN: AtomicBool = AtomicBool::new(false);
/// The card steps aside after POP_FOR so Meet's own controls under it (who
/// waits to be admitted, participants) can be reached; the capsule's
/// «Почати запис» stays. Set until the next offer.
static POP_DONE: AtomicBool = AtomicBool::new(false);
static POP_GENERATION: AtomicU64 = AtomicU64::new(0);
const POP_FOR: Duration = Duration::from_secs(12);
/// When a meeting was last chosen (ms): Meet's start page in reports right
/// after that is the old page, not a return to it
static CHOSEN_AT: AtomicU64 = AtomicU64::new(0);
const CAPTURE_JS: &str = include_str!("capture.js");
/// Safari's own user agent: Meet supports Safari and may refuse an unknown WebKit browser
const SAFARI_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";
/// The Meet window's own cookies and sign-in, apart from the app's pages (macOS 14+;
/// older systems use the default store)
const STORE_ID: [u8; 16] = [0x6d, 0x65, 0x65, 0x74, 0x2d, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x6b, 0x6f, 0x6c, 0x2d, 0x30, 0x31];
/// Largest report taken (a 2 s report is a few kilobytes)
const MAX_REPORT_BYTES: usize = 512 * 1024;

/// The app's theme (Settings → Вигляд) for the window's frame: None follows macOS
static THEME: Lazy<Mutex<Option<tauri::Theme>>> = Lazy::new(|| Mutex::new(None));

/// The file the current window's reports go to
static REPORT_FILE: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));
static REPORTS: Lazy<Mutex<u64>> = Lazy::new(|| Mutex::new(0));
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Google's own sites, apart from google.<country>: the sign-in hops through
/// accounts.youtube.com, Meet loads parts and frames from these
const GOOGLE_DOMAINS: &[&str] = &[
    "youtube.com",
    "googleusercontent.com",
    "googleapis.com",
    "gstatic.com",
    "ggpht.com",
    "googlevideo.com",
    "doubleclick.net",
    "googlesyndication.com",
];

/// google.com, google.com.ua, google.co.uk, google.de … and their subdomains
/// (the sign-in sets its cookie on accounts.google.<country> too)
fn is_google_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    let Some(i) = labels.iter().rposition(|l| *l == "google") else { return false };
    let tld = &labels[i + 1..];
    let cc = |l: &str| l.len() == 2 && l.bytes().all(|b| b.is_ascii_lowercase());
    match tld {
        ["com"] => true,
        [c] => cc(c),
        ["com" | "co", c] => cc(c),
        _ => false,
    }
}

/// Addresses the Meet window may show itself: Meet, the Google sign-in and
/// Google's own sites. On macOS this is asked for every frame as well, not only the page
fn allowed_in_window(url: &Url) -> bool {
    match url.scheme() {
        "about" | "blob" | "data" => return true,
        "https" => {}
        _ => return false,
    }
    let Some(host) = url.host_str() else { return false };
    is_google_host(host)
        || GOOGLE_DOMAINS
            .iter()
            .any(|d| host == *d || host.strip_suffix(d).is_some_and(|rest| rest.ends_with('.')))
}

/// When each host was last handed to the default browser
static OPENED_OUTSIDE: Lazy<Mutex<HashMap<String, Instant>>> = Lazy::new(|| Mutex::new(HashMap::new()));
const OPEN_OUTSIDE_EVERY: Duration = Duration::from_secs(30);

/// True at most once per host per 30 s
fn first_in_a_while(host: &str) -> bool {
    let now = Instant::now();
    let mut last = OPENED_OUTSIDE.lock().unwrap();
    match last.get(host) {
        Some(t) if now.duration_since(*t) < OPEN_OUTSIDE_EVERY => false,
        _ => {
            last.insert(host.to_string(), now);
            true
        }
    }
}

/// A meeting address typed or pasted by the user: only https://meet.google.com/…
fn meeting_url(input: Option<&str>) -> Result<Url, String> {
    let text = input.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("https://meet.google.com/");
    let text = if text.starts_with("http") { text.to_string() } else { format!("https://{text}") };
    let url = Url::parse(&text).map_err(|_| "Це не посилання на Google Meet".to_string())?;
    if url.scheme() != "https" || url.host_str() != Some("meet.google.com") {
        return Err("Потрібне посилання на Google Meet (https://meet.google.com/…)".to_string());
    }
    Ok(url)
}

/// Where the reports go: one file per opened window
fn sessions_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("meet"))
}

fn new_report_file<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let dir = sessions_dir(app)?;
    if let Err(e) = fs::create_dir_all(&dir) {
        warn!("Meet: cannot create {}: {e}", dir.display());
        return None;
    }
    Some(dir.join(format!("{}.jsonl", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"))))
}

/// Reports older than this are deleted when the window opens (a recording
/// keeps its own copy of what it used, `meet.json` in its folder)
const KEEP_DAYS: u64 = 60;

fn prune(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let limit = Duration::from_secs(KEEP_DAYS * 24 * 3600);
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > limit);
        if old && fs::remove_file(&path).is_ok() {
            info!("Meet: old report removed");
        }
    }
}

/// Opens (or brings forward) the Meet window. `title`: the calendar event it
/// is opened for, the name a recording started from the window gets. Without
/// a meeting's link the window opens on Protokol's start screen. Builds
/// webviews, so call it off the main thread.
pub fn open<R: Runtime>(app: &AppHandle<R>, url: Option<&str>, title: Option<String>) -> Result<(), String> {
    let asked = url.map(str::trim).filter(|u| !u.is_empty());
    if let Some(window) = app.get_window(MEET_LABEL) {
        // Another meeting's link: the window goes to it
        if let Some(asked) = asked {
            let target = meeting_url(Some(asked))?;
            if let Some(page) = app.get_webview(PAGE_LABEL) {
                let here = page.url().ok();
                if here.as_ref().map(|u| u.path()) != Some(target.path()) && target.path() != "/" {
                    let _ = page.navigate(target);
                }
            }
            chosen();
            set_start_screen(app, false);
            live::set_title(app, title);
        } else {
            live::on_opened(title, false);
        }
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    let start = asked.is_none();
    let url = meeting_url(asked)?;
    if let Some(dir) = sessions_dir(app) {
        prune(&dir);
    }
    let file = new_report_file(app);
    if let Some(f) = &file {
        info!("Meet: reports go to {}", f.display());
    }
    *REPORT_FILE.lock().unwrap() = file;
    *REPORTS.lock().unwrap() = 0;
    START_SCREEN.store(start, Ordering::SeqCst);
    POP_SHOWN.store(false, Ordering::SeqCst);
    if !start {
        chosen();
    }
    build_window(app, url).map_err(|e| format!("Не вдалося відкрити вікно Meet: {e}"))?;
    live::on_opened(title, true);
    crate::app_menu::show_meet_window_item(app, true);
    Ok(())
}

fn chosen() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    CHOSEN_AT.store(now, Ordering::SeqCst);
}

fn since_chosen_ms() -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    now.saturating_sub(CHOSEN_AT.load(Ordering::SeqCst))
}

/// The window: no title bar of its own (the window buttons sit in Protokol's
/// toolbar), Google's page under the toolbar, the popover above both
fn build_window<R: Runtime>(app: &AppHandle<R>, url: Url) -> tauri::Result<()> {
    let mut config = tauri::utils::config::WindowConfig::default();
    config.label = MEET_LABEL.into();
    config.title = "Google Meet — Протокол".into();
    config.width = 1180.0;
    config.height = 800.0;
    config.min_width = Some(760.0);
    config.min_height = Some(520.0);
    config.theme = *THEME.lock().unwrap();
    #[cfg(target_os = "macos")]
    {
        config.title_bar_style = tauri::utils::TitleBarStyle::Overlay;
        config.hidden_title = true;
        config.traffic_light_position = Some(tauri::utils::config::LogicalPosition { x: 15.0, y: 28.0 });
    }
    let window = tauri::window::WindowBuilder::from_config(app, &config)?.build()?;
    let (width, height) = logical_size(&window);

    let page = WebviewBuilder::new(PAGE_LABEL, WebviewUrl::External(url))
        .user_agent(SAFARI_UA)
        .initialization_script(CAPTURE_JS)
        .data_store_identifier(STORE_ID)
        // Asked for every frame too, not only the page: anything else is cancelled
        // and opened in the default browser at most once per host per 30 s, so a
        // hidden frame of the page cannot keep opening Chrome
        .on_navigation(|url| {
            if allowed_in_window(url) {
                return true;
            }
            let host = url.host_str().unwrap_or("?").to_string();
            if matches!(url.scheme(), "https" | "http") && first_in_a_while(&host) {
                info!("Meet: {host} opens in the default browser");
                let _ = std::process::Command::new("open").arg(url.as_str()).spawn();
            } else {
                info!("Meet: {host} not shown in the Meet window");
            }
            false
        })
        // Links Meet opens in a new window (chat links, help): the default browser
        .on_new_window(|url, _features| {
            if matches!(url.scheme(), "https" | "http") {
                info!("Meet: new window {} opens in the default browser", url.host_str().unwrap_or("?"));
                let _ = std::process::Command::new("open").arg(url.as_str()).spawn();
            }
            tauri::webview::NewWindowResponse::Deny
        });
    window.add_child(page, LogicalPosition::new(0.0, BAR_HEIGHT), LogicalSize::new(width, (height - BAR_HEIGHT).max(1.0)))?;

    let bar_height = if START_SCREEN.load(Ordering::SeqCst) { height } else { BAR_HEIGHT };
    let bar = window.add_child(
        WebviewBuilder::new(BAR_LABEL, WebviewUrl::App("meet-bar".into())),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(width, bar_height),
    )?;
    if START_SCREEN.load(Ordering::SeqCst) {
        let _ = bar.set_focus();
    }

    let pop = window.add_child(
        WebviewBuilder::new(POP_LABEL, WebviewUrl::App("meet-pop".into())).transparent(true),
        LogicalPosition::new((width - POP_RIGHT - POP_WIDTH).max(0.0), BAR_HEIGHT - 6.0),
        LogicalSize::new(POP_WIDTH, POP_HEIGHT),
    )?;
    let _ = pop.hide();
    Ok(())
}

fn logical_size<R: Runtime>(window: &Window<R>) -> (f64, f64) {
    let scale = window.scale_factor().unwrap_or(1.0);
    match window.inner_size() {
        Ok(size) => {
            let size = size.to_logical::<f64>(scale);
            (size.width, size.height)
        }
        Err(_) => (1180.0, 800.0),
    }
}

/// Places the three webviews for the window's size (on every resize)
pub fn layout<R: Runtime>(window: &Window<R>) {
    let app = window.app_handle();
    let (width, height) = logical_size(window);
    let start = START_SCREEN.load(Ordering::SeqCst);
    if let Some(page) = app.get_webview(PAGE_LABEL) {
        let _ = page.set_position(LogicalPosition::new(0.0, BAR_HEIGHT));
        let _ = page.set_size(LogicalSize::new(width, (height - BAR_HEIGHT).max(1.0)));
    }
    if let Some(bar) = app.get_webview(BAR_LABEL) {
        let _ = bar.set_position(LogicalPosition::new(0.0, 0.0));
        let _ = bar.set_size(LogicalSize::new(width, if start { height } else { BAR_HEIGHT }));
    }
    if let Some(pop) = app.get_webview(POP_LABEL) {
        let _ = pop.set_position(LogicalPosition::new((width - POP_RIGHT - POP_WIDTH).max(0.0), BAR_HEIGHT - 6.0));
    }
}

/// The start screen on or off: the toolbar's page covers the whole window
/// while it is on, Google's page is under it
pub fn set_start_screen<R: Runtime>(app: &AppHandle<R>, on: bool) {
    let was = START_SCREEN.swap(on, Ordering::SeqCst);
    if was == on {
        return;
    }
    if let Some(window) = app.get_window(MEET_LABEL) {
        layout(&window);
    }
    let _ = app.emit_to(BAR_LABEL, "meet-start-screen", on);
    if on {
        // Back on the start screen: the last meeting's name no longer applies
        live::set_title(app, None);
    } else {
        if let Some(page) = app.get_webview(PAGE_LABEL) {
            let _ = page.set_focus();
        }
    }
}

/// The page reports Meet's own start page: back to Protokol's start screen
/// (after leaving a call, or once the sign-in is done)
pub(crate) fn on_landing<R: Runtime>(app: &AppHandle<R>) {
    if since_chosen_ms() > 6000 {
        set_start_screen(app, true);
    }
}

/// Shows or hides the offer under the toolbar's capsule
pub(crate) fn show_pop<R: Runtime>(app: &AppHandle<R>, show: bool) {
    let show = show && !POP_DONE.load(Ordering::SeqCst);
    let was = POP_SHOWN.swap(show, Ordering::SeqCst);
    if was == show {
        return;
    }
    if show {
        let generation = POP_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(POP_FOR).await;
            if POP_GENERATION.load(Ordering::SeqCst) == generation && POP_SHOWN.load(Ordering::SeqCst) {
                POP_DONE.store(true, Ordering::SeqCst);
                show_pop(&app, false);
            }
        });
    }
    // The main window's dialog steps aside while the offer is here: one place at a time
    let _ = app.emit("meet-pop-shown", show);
    let Some(pop) = app.get_webview(POP_LABEL) else { return };
    if show {
        if let Some(window) = app.get_window(MEET_LABEL) {
            layout(&window);
        }
        let _ = pop.show();
    } else {
        let _ = pop.hide();
    }
}

/// A new offer: its card may show again
pub(crate) fn new_pop() {
    POP_DONE.store(false, Ordering::SeqCst);
}

/// Whether the offer shows under the Meet window's toolbar (for the main
/// window's dialog when it loads)
#[tauri::command]
pub fn meet_pop_shown() -> bool {
    POP_SHOWN.load(Ordering::SeqCst)
}

/// Whether the start screen is on (for the toolbar's page when it loads)
#[tauri::command]
pub fn meet_start_screen() -> bool {
    START_SCREEN.load(Ordering::SeqCst)
}

/// A choice on the start screen: "new" (a new meeting), "signin" (Google's
/// sign-in), or a meeting's link or code. `title`: the calendar event.
#[tauri::command]
pub async fn meet_go<R: Runtime>(app: AppHandle<R>, target: String, title: Option<String>) -> Result<(), String> {
    let target = target.trim();
    let url = match target {
        "new" => Url::parse("https://meet.google.com/new").map_err(|e| e.to_string())?,
        "signin" => Url::parse("https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fmeet.google.com%2F").map_err(|e| e.to_string())?,
        code if is_meeting_code(code) => meeting_url(Some(&format!("meet.google.com/{}", code.to_lowercase())))?,
        link => meeting_url(Some(link))?,
    };
    if url.host_str() == Some("meet.google.com") && url.path() == "/" {
        return Err("Потрібне посилання на нараду або її код".into());
    }
    let Some(page) = app.get_webview(PAGE_LABEL) else {
        return open(&app, Some(url.as_str()), title.filter(|t| !t.trim().is_empty()));
    };
    chosen();
    page.navigate(url).map_err(|e| e.to_string())?;
    live::set_title(&app, title);
    set_start_screen(&app, false);
    Ok(())
}

/// «abc-defg-hij»
fn is_meeting_code(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 3
        && [3, 4, 3].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_alphabetic()))
}

/// The app's theme changed (or a window loaded): the Meet window's frame
/// follows it. `theme`: "light", "dark" or None for the system's.
#[tauri::command]
pub fn meet_set_theme<R: Runtime>(app: AppHandle<R>, theme: Option<String>) {
    let theme = match theme.as_deref() {
        Some("light") => Some(tauri::Theme::Light),
        Some("dark") => Some(tauri::Theme::Dark),
        _ => None,
    };
    *THEME.lock().unwrap() = theme;
    if let Some(w) = app.get_window(MEET_LABEL) {
        let _ = w.set_theme(theme);
    }
}

/// Opens the Meet window. `url` empty = Meet's start page; `title` = the
/// calendar event it is opened for.
#[tauri::command]
pub async fn meet_open<R: Runtime>(app: AppHandle<R>, url: Option<String>, title: Option<String>) -> Result<(), String> {
    open(&app, url.as_deref(), title.filter(|t| !t.trim().is_empty()))
}

/// One 2 s report from the script in the Meet page. The reply is what its
/// pill shows: whether a recording runs and for how long.
#[tauri::command]
async fn push<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, report: serde_json::Value) -> Result<live::PageState, String> {
    if webview.label() != PAGE_LABEL {
        return Err("not the Meet page".into());
    }
    let line = serde_json::to_string(&report).map_err(|e| e.to_string())?;
    if line.len() > MAX_REPORT_BYTES {
        return Err("report too large".into());
    }
    let path = {
        let mut guard = REPORT_FILE.lock().unwrap();
        if guard.is_none() {
            *guard = new_report_file(&app);
        }
        guard.clone()
    };
    let Some(path) = path else { return Err("no report folder".into()) };
    {
        // One line in one write, one report at a time: two reports arriving
        // together must not end up glued on one line
        let _guard = WRITE_LOCK.lock().unwrap();
        let mut f = OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
        f.write_all(format!("{line}\n").as_bytes()).map_err(|e| e.to_string())?;
    }

    // A short line in the app log now and then, to follow a meeting from the log
    let n = {
        let mut c = REPORTS.lock().unwrap();
        *c += 1;
        *c
    };
    if n == 1 || n % 15 == 0 {
        let streams = report.get("streams").and_then(|s| s.as_array()).map(|a| a.len()).unwrap_or(0);
        let tiles = report.get("tiles").and_then(|t| t.as_u64()).unwrap_or(0);
        let hits: u64 = report
            .get("classHits")
            .and_then(|h| h.as_object())
            .map(|o| o.values().filter_map(|v| v.as_u64()).sum())
            .unwrap_or(0);
        let ctx = report.get("ctx").and_then(|c| c.get("state")).and_then(|s| s.as_str()).unwrap_or("none");
        let in_call = report.get("inCall").and_then(|v| v.as_bool()).unwrap_or(false);
        info!("Meet report #{n}: in call={in_call} streams={streams} tiles={tiles} speaking-class hits={hits} audio={ctx}");
        check_sign_in(&webview);
    }
    if report.get("landing").and_then(|v| v.as_bool()) == Some(true) {
        on_landing(&app);
    }
    Ok(live::on_report(&app, &report).await)
}

/// Meet's microphone button changed (true: off, false: on, null: no button
/// on the page). Sent at once, apart from the 2 s report.
#[tauri::command]
async fn mic<R: Runtime>(app: AppHandle<R>, webview: Webview<R>, muted: Option<bool>) -> Result<live::PageState, String> {
    if webview.label() != PAGE_LABEL {
        return Err("not the Meet page".into());
    }
    live::on_meet_mic(&app, muted).await;
    Ok(live::page_state())
}

/// Remembers whether the window is signed in to Google, for Settings → Google
/// Meet (the cookies are asked for, never read or kept)
fn check_sign_in<R: Runtime>(webview: &Webview<R>) {
    // Google's sign-in cookies are set for .google.com, which a lookup for
    // one address (accounts.google.com) does not return: all of the window's
    // cookies are asked for and only the names of Google's are looked at
    let Ok(cookies) = webview.cookies() else { return };
    let signed_in = cookies.iter().any(|c| {
        matches!(c.name(), "SID" | "__Secure-1PSID" | "__Secure-3PSID")
            && c.domain().is_some_and(|d| d.trim_start_matches('.') == "google.com" || d.ends_with(".google.com"))
    });
    let app = webview.app_handle().clone();
    let _ = app.emit_to(BAR_LABEL, "meet-signed-in", signed_in);
    tauri::async_runtime::spawn(async move {
        if let Some(pool) = pool(&app) {
            let _ = crate::database::repositories::setting::SettingsRepository::set_flag(&pool, SIGNED_IN, if signed_in { "yes" } else { "no" }).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Settings → Google Meet
// ---------------------------------------------------------------------------

/// «Відкривати наради Google Meet у вікні Протоколу»
pub const OPEN_IN_APP: &str = "meet_open_in_app";
/// «Нагадувати почати запис, коли нарада почалась»
pub const REMIND_START: &str = "meet_remind_start";
/// «Пропонувати завершити запис, коли нарада завершилась»
pub const OFFER_STOP: &str = "meet_offer_stop";
/// «Ділити репліки за учасниками Google Meet»
pub const SPLIT: &str = "meet_split";
/// «Записувати мікрофон, лише коли він увімкнений у Google Meet»
pub const MIC_SYNC: &str = "meet_mic_sync";
/// Whether the window was signed in to Google when last seen
const SIGNED_IN: &str = "meet_signed_in";

pub(crate) fn pool<R: Runtime>(app: &AppHandle<R>) -> Option<SqlitePool> {
    let state = app.try_state::<crate::state::AppState>()?;
    Some(state.db_manager.pool().clone())
}

/// The switches are on unless turned off
pub(crate) async fn flag_on(pool: &SqlitePool, key: &str) -> bool {
    crate::database::repositories::setting::SettingsRepository::get_flag(pool, key).await.ok().flatten().as_deref() != Some("off")
}

/// After a recording, voices are told apart by the Meet participant speaking
pub async fn split_on(pool: &SqlitePool) -> bool {
    flag_on(pool, SPLIT).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetSettings {
    open_in_app: bool,
    remind_start: bool,
    offer_stop: bool,
    split: bool,
    mic_sync: bool,
}

#[tauri::command]
pub async fn get_meet_settings<R: Runtime>(app: AppHandle<R>) -> Result<MeetSettings, String> {
    let pool = pool(&app).ok_or("Немає бази даних")?;
    Ok(MeetSettings {
        open_in_app: flag_on(&pool, OPEN_IN_APP).await,
        remind_start: flag_on(&pool, REMIND_START).await,
        offer_stop: flag_on(&pool, OFFER_STOP).await,
        split: flag_on(&pool, SPLIT).await,
        mic_sync: flag_on(&pool, MIC_SYNC).await,
    })
}

/// `key`: openInApp, remindStart, offerStop, split or micSync
#[tauri::command]
pub async fn set_meet_setting<R: Runtime>(app: AppHandle<R>, key: String, on: bool) -> Result<(), String> {
    let flag = match key.as_str() {
        "openInApp" => OPEN_IN_APP,
        "remindStart" => REMIND_START,
        "offerStop" => OFFER_STOP,
        "split" => SPLIT,
        "micSync" => MIC_SYNC,
        other => return Err(format!("Невідоме налаштування: {other}")),
    };
    let pool = pool(&app).ok_or("Немає бази даних")?;
    crate::database::repositories::setting::SettingsRepository::set_flag(&pool, flag, if on { "on" } else { "off" })
        .await
        .map_err(|e| e.to_string())?;
    if flag == MIC_SYNC {
        live::set_mic_sync(on);
    }
    Ok(())
}

/// The window's own reports kept apart from recordings, and the sign-in
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetData {
    /// Report files with a call in them (a window opened without joining a
    /// call leaves a file too, which is not a meeting)
    meetings: usize,
    bytes: u64,
    /// None: the window has not been opened since this was added
    signed_in: Option<bool>,
    window_open: bool,
}

fn report_files(dir: &std::path::Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .map(|p| {
            let len = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            (p, len)
        })
        .collect()
}

async fn meet_data<R: Runtime>(app: &AppHandle<R>) -> MeetData {
    let files = sessions_dir(app).map(|d| report_files(&d)).unwrap_or_default();
    let signed_in = match pool(app) {
        Some(pool) => crate::database::repositories::setting::SettingsRepository::get_flag(&pool, SIGNED_IN)
            .await
            .ok()
            .flatten()
            .map(|v| v == "yes"),
        None => None,
    };
    let meetings = files
        .iter()
        .filter(|(path, _)| fs::read_to_string(path).is_ok_and(|text| text.contains("\"inCall\":true")))
        .count();
    MeetData {
        meetings,
        bytes: files.iter().map(|(_, n)| n).sum(),
        signed_in,
        window_open: app.get_window(MEET_LABEL).is_some(),
    }
}

#[tauri::command]
pub async fn get_meet_data<R: Runtime>(app: AppHandle<R>) -> MeetData {
    meet_data(&app).await
}

/// «Показати у Finder»
#[tauri::command]
pub async fn reveal_meet_data<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let dir = sessions_dir(&app).ok_or("Немає папки даних")?;
    // A file inside is selected: the data folder sits in «ua.protokol.app»,
    // which Finder takes for an app and does not open by itself
    crate::settings_extra::reveal_in_finder(&dir)
}

/// «Видалити…»: the reports kept apart from recordings. A recording keeps its
/// own copy, deleted with the recording. The open window's current file stays.
#[tauri::command]
pub async fn delete_meet_data<R: Runtime>(app: AppHandle<R>) -> Result<MeetData, String> {
    let dir = sessions_dir(&app).ok_or("Немає папки даних")?;
    let current = if app.get_window(MEET_LABEL).is_some() { REPORT_FILE.lock().unwrap().clone() } else { None };
    let mut failed = 0;
    for (path, _) in report_files(&dir) {
        if Some(&path) == current.as_ref() {
            continue;
        }
        if fs::remove_file(&path).is_err() {
            failed += 1;
        }
    }
    info!("Meet: reports deleted from Settings ({failed} failed)");
    if failed > 0 {
        return Err(format!("Не вдалося видалити файлів: {failed}"));
    }
    Ok(meet_data(&app).await)
}

/// «Вийти з Google…»: closes the window and deletes its own cookies and site
/// data (the sign-in). Chrome, Safari and the app's pages are not touched.
#[tauri::command]
pub async fn meet_sign_out<R: Runtime>(app: AppHandle<R>) -> Result<MeetData, String> {
    if let Some(w) = app.get_window(MEET_LABEL) {
        w.destroy().map_err(|e| e.to_string())?;
        // WebKit lets a store go only once no page uses it
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    #[cfg(target_vendor = "apple")]
    {
        let known = app.fetch_data_store_identifiers().await.unwrap_or_default();
        if known.contains(&STORE_ID) {
            app.remove_data_store(STORE_ID).await.map_err(|e| format!("Не вдалося вийти з Google: {e}"))?;
        }
    }
    if let Some(pool) = pool(&app) {
        let _ = crate::database::repositories::setting::SettingsRepository::set_flag(&pool, SIGNED_IN, "no").await;
    }
    info!("Meet: signed out, the window's data removed");
    Ok(meet_data(&app).await)
}

/// How the last speaker recognition of a meeting used the Meet window (for
/// the banners in «Спікери»). None: no Meet window during that recording.
#[tauri::command]
pub async fn get_meet_status<R: Runtime>(app: AppHandle<R>, meeting_id: String) -> Result<Option<crate::speakers::meet::MeetStatus>, String> {
    let pool = pool(&app).ok_or("Немає бази даних")?;
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
        .bind(&meeting_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten();
    Ok(folder.and_then(|f| crate::speakers::meet::load_status(std::path::Path::new(&f))))
}

/// The `meetprobe` plugin: the only command the Meet page can call. Its
/// setup tells speaker recognition where the reports are.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    PluginBuilder::new("meetprobe")
        .invoke_handler(tauri::generate_handler![push, mic])
        .setup(|app, _| {
            let mut dirs = Vec::new();
            if let Some(d) = sessions_dir(app) {
                dirs.push(d);
            }
            // The stage-0 test copy wrote its reports to the log folder
            if let Ok(d) = app.path().app_log_dir() {
                dirs.push(d.join("meet-probe"));
            }
            frames::set_sessions_dirs(dirs);
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_addresses() {
        assert_eq!(meeting_url(Some("https://meet.google.com/abc-defg-hij")).unwrap().as_str(), "https://meet.google.com/abc-defg-hij");
        assert_eq!(meeting_url(Some("meet.google.com/abc-defg-hij")).unwrap().as_str(), "https://meet.google.com/abc-defg-hij");
        assert_eq!(meeting_url(None).unwrap().as_str(), "https://meet.google.com/");
        assert!(meeting_url(Some("http://meet.google.com/abc")).is_err());
        assert!(meeting_url(Some("https://meet.google.com.evil.example/abc")).is_err());
        assert!(meeting_url(Some("https://example.com/")).is_err());
    }

    #[test]
    fn window_stays_on_meet_and_sign_in() {
        let ok = |s: &str| allowed_in_window(&Url::parse(s).unwrap());
        assert!(ok("https://meet.google.com/abc-defg-hij"));
        assert!(ok("https://accounts.google.com/signin"));
        assert!(ok("https://accounts.youtube.com/accounts/SetSID"));
        assert!(!ok("https://example.com/"));
        assert!(!ok("https://google.com.evil.example/"));
        assert!(!ok("http://meet.google.com/"));
        // The sign-in's country domain and Google's own frames
        assert!(ok("https://accounts.google.com.ua/accounts/SetSID"));
        assert!(ok("https://accounts.google.co.uk/accounts/SetSID"));
        assert!(ok("https://www.google.de/"));
        assert!(ok("https://2507573.fls.doubleclick.net/activityi"));
        assert!(ok("https://lh3.googleusercontent.com/a/x"));
        assert!(ok("https://google.co.uk/"));
        assert!(ok("https://google.com/"));
        assert!(ok("https://accounts.youtube.com/accounts/SetSID"));
        assert!(ok("https://yt3.ggpht.com/x"));
        assert!(ok("blob:https://meet.google.com/1234"));
        assert!(ok("data:text/plain,x"));
        assert!(!ok("https://google.evil/"));
        assert!(!ok("https://evil-google.com/"));
        assert!(!ok("https://google.com.evil.example/"));
        assert!(!ok("http://accounts.google.com/"));
        assert!(!ok("https://google.evil.example/"));
        assert!(!ok("https://accounts.google.com.evil.example/"));
        assert!(!ok("https://notgoogle.com/"));
        assert!(!ok("https://evilyoutube.com/"));
        assert!(!ok("https://google.example.com/"));
    }
}

#[cfg(test)]
mod throttle_tests {
    use super::first_in_a_while;

    #[test]
    fn outside_browser_once_per_host() {
        assert!(first_in_a_while("throttle-test-a.example"));
        assert!(!first_in_a_while("throttle-test-a.example"));
        assert!(first_in_a_while("throttle-test-b.example"));
    }
}
