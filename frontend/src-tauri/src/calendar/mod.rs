//! Meeting reminders from the user's calendar.
//!
//! The app reads the calendar (a Google Calendar "secret address in iCal
//! format", an Outlook / Exchange published calendar link, or an imported
//! .ics file), and a few minutes before each meeting shows a system
//! notification and an in-app prompt to start recording.
//!
//! Nothing is sent to the calendar provider: the feed is only downloaded.
//! The feed link is stored locally in `<app data>/calendar.json`; an imported
//! file in `<app data>/calendar_import.ics`.

pub mod commands;
pub mod ics;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_notification::NotificationExt;
use tracing::{info, warn};

pub use ics::CalendarEvent;

const SETTINGS_FILE: &str = "calendar.json";
const IMPORT_FILE: &str = "calendar_import.ics";

/// Event sent to the UI when a meeting is about to start.
pub const REMINDER_EVENT: &str = "calendar-meeting-reminder";

/// How often the feed link is downloaded again.
const REFRESH_EVERY: StdDuration = StdDuration::from_secs(5 * 60);
/// How often the reminder loop wakes up.
const TICK_EVERY: StdDuration = StdDuration::from_secs(20);
/// A meeting that started less than this long ago still gets a reminder
/// (for example when the app was opened a little late).
const LATE_GRACE_MINUTES: i64 = 10;
/// Largest feed accepted, to protect against a wrong link.
const MAX_FEED_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CalendarSettings {
    pub enabled: bool,
    /// https:// or webcal:// link to an iCal feed.
    pub ics_url: Option<String>,
    /// Remind this many minutes before the start.
    pub remind_minutes_before: u32,
    /// Bring the app window to the front with the "start recording" prompt.
    pub bring_window_to_front: bool,
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ics_url: None,
            remind_minutes_before: 2,
            bring_window_to_front: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReminderPayload {
    pub event: CalendarEvent,
    /// Whole minutes until the start; 0 or negative when it has started.
    pub minutes_until: i64,
}

#[derive(Default)]
struct CalendarState {
    dir: Option<PathBuf>,
    settings: CalendarSettings,
    events: Vec<CalendarEvent>,
    last_refresh: Option<Instant>,
    last_refresh_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    reminded: HashSet<String>,
}

static STATE: Lazy<Mutex<CalendarState>> = Lazy::new(|| Mutex::new(CalendarState::default()));

fn with_state<T>(f: impl FnOnce(&mut CalendarState) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

/// Loads settings from disk. Called once at startup.
pub fn init(app_data_dir: &Path) {
    let settings = std::fs::read_to_string(app_data_dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<CalendarSettings>(&text).ok())
        .unwrap_or_default();
    info!("Calendar reminders enabled: {}", settings.enabled);
    with_state(|s| {
        s.dir = Some(app_data_dir.to_path_buf());
        s.settings = settings;
    });
}

pub fn settings() -> CalendarSettings {
    with_state(|s| s.settings.clone())
}

/// Validates, stores and applies new settings. The feed is reloaded on the
/// next refresh.
pub fn save_settings(mut new: CalendarSettings) -> Result<CalendarSettings, String> {
    new.ics_url = match new.ics_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => Some(normalise_feed_url(url)?),
        None => None,
    };
    new.remind_minutes_before = new.remind_minutes_before.min(60);
    let dir = with_state(|s| s.dir.clone()).ok_or("Календар ще не ініціалізовано.")?;
    let json = serde_json::to_string_pretty(&new).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(SETTINGS_FILE), json).map_err(|e| format!("Не вдалося зберегти налаштування: {e}"))?;
    with_state(|s| {
        if s.settings.ics_url != new.ics_url {
            s.events.clear();
            s.last_error = None;
        }
        s.settings = new.clone();
        s.last_refresh = None;
    });
    Ok(new)
}

/// Accepts https:// and webcal:// links (webcal is https in practice).
pub fn normalise_feed_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    let url = if let Some(rest) = url.strip_prefix("webcal://") {
        format!("https://{rest}")
    } else {
        url.to_string()
    };
    let parsed = url::Url::parse(&url).map_err(|_| "Це не схоже на посилання.".to_string())?;
    if parsed.scheme() != "https" {
        return Err("Потрібне посилання https:// або webcal://.".to_string());
    }
    Ok(url)
}

/// Saves an exported .ics file as the calendar source.
pub fn import_file(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() > MAX_FEED_BYTES {
        return Err("Файл завеликий (понад 20 МБ).".to_string());
    }
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| "Файл не в кодуванні UTF-8.".to_string())?;
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("Це не файл календаря (.ics).".to_string());
    }
    let dir = with_state(|s| s.dir.clone()).ok_or("Календар ще не ініціалізовано.")?;
    std::fs::write(dir.join(IMPORT_FILE), &text).map_err(|e| format!("Не вдалося зберегти файл: {e}"))?;
    let now = Utc::now();
    let count = ics::events_between(&text, now, now + Duration::days(30)).len();
    with_state(|s| s.last_refresh = None);
    Ok(count)
}

pub fn remove_imported_file() -> Result<(), String> {
    let dir = with_state(|s| s.dir.clone()).ok_or("Календар ще не ініціалізовано.")?;
    let path = dir.join(IMPORT_FILE);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Не вдалося видалити файл: {e}"))?;
    }
    with_state(|s| {
        s.last_refresh = None;
        s.events.clear();
    });
    Ok(())
}

pub fn has_imported_file() -> bool {
    with_state(|s| s.dir.as_ref().map(|d| d.join(IMPORT_FILE).exists()).unwrap_or(false))
}

/// Downloads or reads the calendar and keeps the next two days of meetings.
pub async fn refresh() -> Result<usize, String> {
    let (settings, dir) = with_state(|s| (s.settings.clone(), s.dir.clone()));
    let result = load_calendar_text(&settings, dir.as_deref()).await.map(|text| {
        let now = Utc::now();
        ics::events_between(&text, now - Duration::hours(12), now + Duration::hours(48))
    });
    with_state(|s| {
        s.last_refresh = Some(Instant::now());
        match &result {
            Ok(events) => {
                s.events = events.clone();
                s.last_refresh_at = Some(Utc::now());
                s.last_error = None;
            }
            Err(e) => s.last_error = Some(e.clone()),
        }
    });
    result.map(|events| events.len())
}

async fn load_calendar_text(settings: &CalendarSettings, dir: Option<&Path>) -> Result<String, String> {
    if let Some(url) = &settings.ics_url {
        return fetch_feed(url).await;
    }
    let path = dir.map(|d| d.join(IMPORT_FILE)).ok_or("Календар ще не ініціалізовано.")?;
    std::fs::read_to_string(&path).map_err(|_| "Додайте посилання на календар або файл .ics.".to_string())
}

async fn fetch_feed(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Не вдалося завантажити календар: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Календар повернув помилку {}. Перевірте, що посилання повне і не застаріло.",
            response.status().as_u16()
        ));
    }
    let bytes = response.bytes().await.map_err(|e| format!("Не вдалося прочитати календар: {e}"))?;
    if bytes.len() > MAX_FEED_BYTES {
        return Err("Календар завеликий (понад 20 МБ).".to_string());
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("За посиланням не календар. Потрібна «секретна адреса у форматі iCal».".to_string());
    }
    Ok(text)
}

#[derive(Debug, Clone, Serialize)]
pub struct CalendarStatus {
    pub settings: CalendarSettings,
    pub has_imported_file: bool,
    pub last_refresh_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// Meetings from now (including ones in progress) over the next 24 hours.
    pub upcoming: Vec<CalendarEvent>,
}

pub fn status() -> CalendarStatus {
    let now = Utc::now();
    with_state(|s| CalendarStatus {
        settings: s.settings.clone(),
        has_imported_file: s.dir.as_ref().map(|d| d.join(IMPORT_FILE).exists()).unwrap_or(false),
        last_refresh_at: s.last_refresh_at,
        last_error: s.last_error.clone(),
        upcoming: s
            .events
            .iter()
            .filter(|e| e.end.unwrap_or(e.start + Duration::minutes(30)) > now && e.start < now + Duration::hours(24))
            .cloned()
            .collect(),
    })
}

/// Meetings that need a reminder now and have not had one yet.
fn due_reminders(
    events: &[CalendarEvent],
    reminded: &HashSet<String>,
    now: DateTime<Utc>,
    minutes_before: u32,
) -> Vec<CalendarEvent> {
    events
        .iter()
        .filter(|e| {
            let remind_at = e.start - Duration::minutes(minutes_before as i64);
            let too_late = e.start + Duration::minutes(LATE_GRACE_MINUTES);
            now >= remind_at && now < too_late && !reminded.contains(&e.occurrence_key())
        })
        .cloned()
        .collect()
}

/// Starts the background reminder loop.
pub fn start_reminder_loop<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tick(&app).await;
            tokio::time::sleep(TICK_EVERY).await;
        }
    });
}

async fn tick<R: Runtime>(app: &AppHandle<R>) {
    let (enabled, needs_refresh) = with_state(|s| {
        (
            s.settings.enabled,
            s.last_refresh.map(|t| t.elapsed() >= REFRESH_EVERY).unwrap_or(true),
        )
    });
    if !enabled {
        return;
    }
    if needs_refresh {
        if let Err(e) = refresh().await {
            warn!("Calendar refresh failed: {e}");
        }
    }

    let now = Utc::now();
    let (due, bring_to_front) = with_state(|s| {
        let due = due_reminders(&s.events, &s.reminded, now, s.settings.remind_minutes_before);
        for event in &due {
            s.reminded.insert(event.occurrence_key());
        }
        (due, s.settings.bring_window_to_front)
    });
    if due.is_empty() || crate::audio::recording_commands::is_recording().await {
        return;
    }

    for event in due {
        let seconds_until = (event.start - now).num_seconds();
        let minutes_until = if seconds_until <= 0 { 0 } else { (seconds_until + 59) / 60 };
        info!("Calendar reminder for a meeting in {minutes_until} min");
        let body = if minutes_until > 0 {
            format!("Починається через {minutes_until} хв. Увімкніть запис для протоколу.")
        } else {
            "Нарада вже почалася. Увімкніть запис для протоколу.".to_string()
        };
        if let Err(e) = app.notification().builder().title(&event.title).body(body).show() {
            warn!("Failed to show calendar notification: {e}");
        }
        if bring_to_front {
            crate::tray::focus_main_window(app);
        }
        if let Err(e) = app.emit(REMINDER_EVENT, ReminderPayload { event, minutes_until }) {
            warn!("Failed to emit calendar reminder: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(uid: &str, start: DateTime<Utc>) -> CalendarEvent {
        CalendarEvent {
            uid: uid.into(),
            title: "Нарада".into(),
            start,
            end: None,
            meeting_url: None,
            location: None,
        }
    }

    #[test]
    fn reminds_in_the_window_and_only_once() {
        let now = Utc::now();
        let soon = event("a", now + Duration::minutes(1));
        let later = event("b", now + Duration::minutes(30));
        let started = event("c", now - Duration::minutes(5));
        let long_ago = event("d", now - Duration::minutes(30));
        let events = vec![soon.clone(), later, started.clone(), long_ago];

        let due = due_reminders(&events, &HashSet::new(), now, 2);
        assert_eq!(due, vec![soon.clone(), started]);

        let reminded: HashSet<String> = due.iter().map(|e| e.occurrence_key()).collect();
        assert!(due_reminders(&events, &reminded, now, 2).is_empty());
    }

    #[test]
    fn feed_links_are_checked() {
        assert_eq!(
            normalise_feed_url(" webcal://calendar.google.com/calendar/ical/x/private-abc/basic.ics ").unwrap(),
            "https://calendar.google.com/calendar/ical/x/private-abc/basic.ics"
        );
        assert!(normalise_feed_url("http://example.com/a.ics").is_err());
        assert!(normalise_feed_url("not a link").is_err());
    }
}
