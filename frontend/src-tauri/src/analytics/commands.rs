//! Analytics is permanently disabled in Протокол: the app handles sensitive government
//! data and must not send anything out. These commands keep the frontend API intact but
//! do nothing; the PostHog client and dependency were removed.
#![allow(unused_variables)]

use std::collections::HashMap;
use tauri::command;

#[command]
pub async fn init_analytics() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn disable_analytics() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_event(event_name: String, properties: Option<HashMap<String, String>>) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn identify_user(user_id: String, properties: Option<HashMap<String, String>>) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_meeting_started(meeting_id: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_recording_started(meeting_id: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_recording_stopped(meeting_id: String, duration_seconds: Option<u64>) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_meeting_deleted(meeting_id: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_settings_changed(setting_type: String, new_value: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_feature_used(feature_name: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn is_analytics_enabled() -> bool {
    false
}

#[command]
pub async fn start_analytics_session(user_id: String) -> Result<String, String> {
    Err("Аналітику вимкнено".to_string())
}

#[command]
pub async fn end_analytics_session() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_daily_active_user() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_user_first_launch() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_summary_generation_started(model_provider: String, model_name: String, transcript_length: usize) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_summary_generation_completed(model_provider: String, model_name: String, success: bool, duration_seconds: Option<u64>, error_message: Option<String>) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_summary_regenerated(model_provider: String, model_name: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_model_changed(old_provider: String, old_model: String, new_provider: String, new_model: String) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_custom_prompt_used(prompt_length: usize) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_meeting_ended(
    transcription_provider: String,
    transcription_model: String,
    summary_provider: String,
    summary_model: String,
    total_duration_seconds: Option<f64>,
    active_duration_seconds: f64,
    pause_duration_seconds: f64,
    microphone_device_type: String,
    system_audio_device_type: String,
    chunks_processed: u64,
    transcript_segments_count: u64,
    had_fatal_error: bool,
) -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_analytics_enabled() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_analytics_disabled() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn track_analytics_transparency_viewed() -> Result<(), String> {
    Ok(())
}

#[command]
pub async fn is_analytics_session_active() -> bool {
    false
}
