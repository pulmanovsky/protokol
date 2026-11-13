//! The call in the Meet window as the app sees it while it goes on.
//!
//! Each 2 s report from the page says whether a call is on, how many
//! participant tiles there are and who is lit as speaking. From that:
//! - the recording view shows «Google Meet · учасників: N · говорить: …»
//!   (`meet-live` event to the main window);
//! - a call that starts while nothing is recorded offers «Почати запис», and a
//!   call that ends while recording offers «Завершити запис» (Settings →
//!   Google Meet). Nothing starts or stops by itself. The offer shows where
//!   the person is: under the Meet window's toolbar while that window is in
//!   front, as a dialog in the main window, otherwise in the floating island;
//! - the secretary's microphone follows Meet's own microphone button: off in
//!   Meet, the recording leaves the microphone out (silence, as with
//!   «Вимкнути мікрофон» in Protokol); on in Meet, it is recorded again from
//!   that moment. A recording started while the button is off starts with the
//!   microphone left out. It works the other way too: Protokol's microphone
//!   button (toolbar, island, main window, ⇧⌘M) presses Meet's. Settings →
//!   Google Meet can turn this off.
//!
//! The page itself cannot start, stop or read anything: it only sends reports
//! and the state of its microphone button.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager, Runtime};

use super::{MEET_LABEL, PAGE_LABEL};
/// Reports in a row (2 s each) before a call counts as started or ended
const CALL_REPORTS: u8 = 2;

/// An offer to start or end the recording
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    /// "start" or "stop"
    pub kind: String,
    /// The meeting's title: the calendar event the window was opened for, or
    /// the recording's name
    pub title: Option<String>,
}

/// What the recording view shows about the Meet window
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetLive {
    pub open: bool,
    /// The calendar event the window was opened for
    pub title: Option<String>,
    /// The page hears the call (Meet plays sound only after a click in it)
    pub audio_ok: bool,
    pub in_call: bool,
    pub participants: u32,
    pub speaking: Vec<String>,
    /// Meet's microphone button: true off, false on, None not on the page
    pub meet_mic: Option<bool>,
    /// Meet's camera button: true off, false on, None not on the page
    pub meet_cam: Option<bool>,
}

/// The reply to each report: what the pill in the page shows
#[derive(Clone, Debug, Serialize)]
pub struct PageState {
    /// "off", "on" or "paused"
    pub rec: &'static str,
    /// Seconds recorded, pauses left out
    pub elapsed: f64,
    /// The recording leaves the microphone out
    #[serde(rename = "micOff")]
    pub mic_off: bool,
}

#[derive(Default)]
struct Live {
    shown: MeetLive,
    /// Reports in a row that disagree with `shown.in_call`
    flips: u8,
    /// The calendar event the window was opened for
    title: Option<String>,
    /// «Почати запис» already offered for this call
    start_offered: bool,
    prompt: Option<Prompt>,
}

static LIVE: Lazy<Mutex<Live>> = Lazy::new(|| Mutex::new(Live::default()));
/// «Записувати мікрофон, лише коли він увімкнений у Google Meet», as last read
static MIC_SYNC: AtomicBool = AtomicBool::new(true);

enum Change {
    Started,
    Ended,
}

/// Follows the recording, to take back an offer that no longer fits. Call once from `setup`.
pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    app.listen("recording-started", move |_| {
        // Started while Meet's microphone is off: the window shows it muted
        if crate::audio::live_levels::is_mic_muted() {
            let _ = handle.emit("mic-mute-changed", serde_json::json!({ "muted": true, "at": 0.0 }));
        }
        dismiss(&handle, Some("start"));
    });
    let handle = app.clone();
    app.listen("recording-stopped", move |_| dismiss(&handle, Some("stop")));
}

/// The window opened (or was asked for again with a calendar event)
pub fn on_opened(title: Option<String>, fresh: bool) {
    {
        let mut l = LIVE.lock().unwrap();
        if fresh {
            let prompt = l.prompt.take();
            *l = Live { prompt, ..Default::default() };
            l.shown.open = true;
        }
        if title.is_some() {
            l.title = title;
        }
    }
}

/// A meeting was chosen in the window (a link, a code, a new meeting) or the
/// window went back to its start screen: the name a recording started from
/// the window gets is this meeting's calendar event, or none
pub fn set_title<R: Runtime>(app: &AppHandle<R>, title: Option<String>) {
    let title = title.filter(|t| !t.trim().is_empty());
    let changed = {
        let mut l = LIVE.lock().unwrap();
        let changed = l.title != title;
        l.title = title;
        changed
    };
    if changed {
        emit_live(app);
    }
}

/// The window closed: a call in it has ended
pub async fn on_closed<R: Runtime>(app: &AppHandle<R>) {
    let was_in_call = {
        let mut l = LIVE.lock().unwrap();
        let was = l.shown.in_call;
        l.shown = MeetLive::default();
        l.flips = 0;
        l.start_offered = false;
        was
    };
    emit_live(app);
    if was_in_call {
        call_ended(app).await;
    }
}

/// One report from the page
pub async fn on_report<R: Runtime>(app: &AppHandle<R>, report: &serde_json::Value) -> PageState {
    let in_call = report.get("inCall").and_then(|v| v.as_bool()).unwrap_or(false);
    let participants = report.get("participants").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let audio_ok = report.get("ctx").and_then(|c| c.get("state")).and_then(|s| s.as_str()) == Some("running");
    let names: Vec<&str> = report
        .get("names")
        .and_then(|n| n.as_array())
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("")).collect())
        .unwrap_or_default();
    // Who is lit in the last 100 ms frame of the report
    let speaking: Vec<String> = report
        .get("lit")
        .and_then(|f| f.as_array())
        .and_then(|frames| frames.last())
        .and_then(|f| f.as_array())
        .map(|lit| {
            lit.iter()
                .filter_map(|i| i.as_u64())
                .filter_map(|i| names.get(i as usize).filter(|n| !n.is_empty()).map(|n| n.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let change = {
        let mut l = LIVE.lock().unwrap();
        l.shown.open = true;
        l.shown.audio_ok = audio_ok;
        l.shown.meet_cam = report.get("cam").and_then(|v| v.as_bool());
        l.shown.participants = if in_call { participants } else { 0 };
        l.shown.speaking = if in_call { speaking } else { Vec::new() };
        if in_call == l.shown.in_call {
            l.flips = 0;
            None
        } else {
            l.flips += 1;
            if l.flips < CALL_REPORTS {
                None
            } else {
                l.flips = 0;
                l.shown.in_call = in_call;
                if in_call {
                    Some(Change::Started)
                } else {
                    l.start_offered = false;
                    Some(Change::Ended)
                }
            }
        }
    };
    emit_live(app);
    match change {
        Some(Change::Started) => call_started(app).await,
        Some(Change::Ended) => call_ended(app).await,
        None => {}
    }
    page_state()
}

/// What the pill shows about the recording
pub fn page_state() -> PageState {
    let mic_off = crate::audio::live_levels::is_mic_muted();
    match crate::audio::recording_commands::recording_glance() {
        None => PageState { rec: "off", elapsed: 0.0, mic_off: false },
        Some((true, secs, _)) => PageState { rec: "paused", elapsed: secs, mic_off },
        Some((false, secs, _)) => PageState { rec: "on", elapsed: secs, mic_off },
    }
}

/// Meet's microphone button changed. While recording, the secretary's
/// microphone follows it at once. Only a change acts, so a microphone turned
/// on or off by hand in Protokol stays so until the button changes again.
pub async fn on_meet_mic<R: Runtime>(app: &AppHandle<R>, muted: Option<bool>) {
    let changed = {
        let mut l = LIVE.lock().unwrap();
        let changed = l.shown.meet_mic != muted;
        l.shown.meet_mic = muted;
        changed
    };
    if !changed {
        return;
    }
    emit_live(app);
    let Some(muted) = muted else { return };
    let sync = match super::pool(app) {
        Some(pool) => super::flag_on(&pool, super::MIC_SYNC).await,
        None => MIC_SYNC.load(Ordering::SeqCst),
    };
    MIC_SYNC.store(sync, Ordering::SeqCst);
    if !sync || crate::audio::recording_commands::recording_glance().is_none() {
        return;
    }
    log::info!("Meet: microphone {} in Meet, the recording follows", if muted { "off" } else { "on" });
    crate::audio::recording_commands::apply_mic_muted(app, muted);
}

/// Protokol's own microphone button was pressed: Meet's button is pressed to
/// match, so the call hears the secretary exactly while the recording does.
/// False when Meet's microphone is not followed (switch off, no window, no
/// button on the page).
pub fn press_meet_mic<R: Runtime>(app: &AppHandle<R>, muted: bool) -> bool {
    let known = LIVE.lock().unwrap().shown.meet_mic.is_some();
    if !known || !MIC_SYNC.load(Ordering::SeqCst) {
        return false;
    }
    let Some(page) = app.get_webview(PAGE_LABEL) else { return false };
    page.eval(format!("window.__protokolSetMic && window.__protokolSetMic({muted})")).is_ok()
}

/// The camera button in Protokol's toolbar: presses Meet's own camera button
#[tauri::command]
pub fn meet_set_camera<R: Runtime>(app: AppHandle<R>, off: bool) -> Result<(), String> {
    let page = app.get_webview(PAGE_LABEL).ok_or("Вікно Google Meet закрито")?;
    page.eval(format!("window.__protokolSetCam && window.__protokolSetCam({off})")).map_err(|e| e.to_string())
}

/// Settings → Google Meet changed the switch
pub fn set_mic_sync(on: bool) {
    MIC_SYNC.store(on, Ordering::SeqCst);
}

/// Whether a recording that starts now leaves the microphone out: Meet's
/// microphone is off in the open window
pub fn mic_muted_at_start() -> bool {
    let l = LIVE.lock().unwrap();
    MIC_SYNC.load(Ordering::SeqCst) && l.shown.open && l.shown.meet_mic == Some(true)
}

/// The Meet window as the recording view, the toolbar and the island show it
pub fn current() -> MeetLive {
    let l = LIVE.lock().unwrap();
    MeetLive { title: l.title.clone(), ..l.shown.clone() }
}

fn emit_live<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit("meet-live", current());
}

async fn call_started<R: Runtime>(app: &AppHandle<R>) {
    log::info!("Meet: the call started");
    if crate::audio::recording_commands::recording_glance().is_some() {
        return;
    }
    let Some(pool) = super::pool(app) else { return };
    if !super::flag_on(&pool, super::REMIND_START).await {
        return;
    }
    let title = {
        let mut l = LIVE.lock().unwrap();
        if l.start_offered {
            return;
        }
        l.start_offered = true;
        l.title.clone()
    };
    show(app, Prompt { kind: "start".into(), title });
}

async fn call_ended<R: Runtime>(app: &AppHandle<R>) {
    log::info!("Meet: the call ended");
    dismiss(app, Some("start"));
    let Some((_, _, name)) = crate::audio::recording_commands::recording_glance() else { return };
    let Some(pool) = super::pool(app) else { return };
    if !super::flag_on(&pool, super::OFFER_STOP).await {
        return;
    }
    show(app, Prompt { kind: "stop".into(), title: name.filter(|n| !n.trim().is_empty()) });
}

fn show<R: Runtime>(app: &AppHandle<R>, prompt: Prompt) {
    super::new_pop();
    LIVE.lock().unwrap().prompt = Some(prompt.clone());
    let _ = app.emit("meet-prompt", Some(prompt));
    place(app);
}

/// Takes the offer back (only one of `kind`, when given)
fn dismiss<R: Runtime>(app: &AppHandle<R>, kind: Option<&str>) {
    let taken = {
        let mut l = LIVE.lock().unwrap();
        match &l.prompt {
            Some(p) if kind.map_or(true, |k| p.kind == k) => l.prompt.take().is_some(),
            _ => false,
        }
    };
    if taken {
        let _ = app.emit("meet-prompt", None::<Prompt>);
        place(app);
    }
}

/// The offer currently shown, if any (for the surfaces when they load)
pub fn prompt() -> Option<Prompt> {
    LIVE.lock().unwrap().prompt.clone()
}

/// The main window came forward or went back
pub fn on_main_focus<R: Runtime>(app: &AppHandle<R>) {
    place(app);
}

/// The Meet window came forward or went back
pub fn on_meet_focus<R: Runtime>(app: &AppHandle<R>) {
    place(app);
}

/// Where the offer shows: under the Meet window's toolbar while that window
/// is the last of Protokol's windows in front; otherwise the main window has
/// it as a dialog; the floating island shows it while the person is outside
/// Protokol (island.rs decides)
fn place<R: Runtime>(app: &AppHandle<R>) {
    let has = LIVE.lock().unwrap().prompt.is_some();
    super::show_pop(app, has && crate::island::meet_is_last_in_front(app));
    crate::island::set_prompt(app, has);
}

/// «Почати запис» from the toolbar or an offer: the main window starts it the
/// same way as from the calendar reminder, named after the calendar event.
/// No window is brought forward: the person stays in the call.
fn start_recording<R: Runtime>(app: &AppHandle<R>) {
    if crate::audio::recording_commands::recording_glance().is_some() {
        return;
    }
    let title = LIVE.lock().unwrap().title.clone();
    let _ = app.emit_to("main", "meet-start-recording", serde_json::json!({ "title": title }));
}

/// The toolbar's «Почати запис»
#[tauri::command]
pub fn meet_start_recording<R: Runtime>(app: AppHandle<R>) {
    dismiss(&app, Some("start"));
    start_recording(&app);
}

/// The offer for the surfaces when they load
#[tauri::command]
pub fn meet_prompt_current() -> Option<Prompt> {
    prompt()
}

/// An answer to the offer: "start" or "stop" act, anything else closes it
#[tauri::command]
pub async fn meet_prompt_answer<R: Runtime>(app: AppHandle<R>, answer: String) -> Result<(), String> {
    let Some(prompt) = LIVE.lock().unwrap().prompt.take() else { return Ok(()) };
    let _ = app.emit("meet-prompt", None::<Prompt>);
    place(&app);
    match (prompt.kind.as_str(), answer.as_str()) {
        ("start", "start") => start_recording(&app),
        ("stop", "stop") => {
            if crate::audio::recording_commands::recording_glance().is_some() {
                crate::tray::handle_menu_event(&app, "stop_recording");
            }
        }
        ("stop", "no") => continue_after_call(&app),
        _ => {}
    }
    Ok(())
}

/// «Продовжити запис» after the call ended: the recording goes on in the
/// room, so a microphone that followed Meet's button (off when the call
/// ended) is turned back on. Meet's button is gone, nothing else follows it.
fn continue_after_call<R: Runtime>(app: &AppHandle<R>) {
    if !MIC_SYNC.load(Ordering::SeqCst) || crate::audio::recording_commands::recording_glance().is_none() {
        return;
    }
    let in_call = LIVE.lock().unwrap().shown.in_call;
    if in_call {
        return;
    }
    log::info!("Meet: the call ended and the recording goes on, the microphone is on");
    crate::audio::recording_commands::apply_mic_muted(app, false);
}

/// The Meet window for the recording view (when it loads)
#[tauri::command]
pub fn get_meet_live() -> MeetLive {
    current()
}

/// «Показати нараду»: brings the Meet window forward
#[tauri::command]
pub fn meet_show<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let w = app.get_window(MEET_LABEL).ok_or("Вікно Google Meet закрито")?;
    let _ = w.unminimize();
    w.show().map_err(|e| e.to_string())?;
    w.set_focus().map_err(|e| e.to_string())
}
