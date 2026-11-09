//! Tauri commands for calendar reminders.

use super::{CalendarSettings, CalendarStatus};

/// Current settings, the next meetings and the last error, if any.
#[tauri::command]
pub async fn api_calendar_status() -> Result<CalendarStatus, String> {
    Ok(super::status())
}

/// Saves settings and, when reminders are on, reloads the calendar right away
/// so that a wrong link is reported immediately.
#[tauri::command]
pub async fn api_calendar_save_settings(settings: CalendarSettings) -> Result<CalendarStatus, String> {
    let saved = super::save_settings(settings)?;
    if saved.enabled && (saved.ics_url.is_some() || super::has_imported_file()) {
        super::refresh().await?;
    }
    Ok(super::status())
}

/// Uses an exported .ics file as the calendar.
#[tauri::command]
pub async fn api_calendar_import_file(file_bytes: Vec<u8>) -> Result<CalendarStatus, String> {
    super::import_file(&file_bytes)?;
    super::refresh().await?;
    Ok(super::status())
}

#[tauri::command]
pub async fn api_calendar_remove_file() -> Result<CalendarStatus, String> {
    super::remove_imported_file()?;
    Ok(super::status())
}

#[tauri::command]
pub async fn api_calendar_refresh() -> Result<CalendarStatus, String> {
    super::refresh().await?;
    Ok(super::status())
}
