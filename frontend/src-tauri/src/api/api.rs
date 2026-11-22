use log::{debug as log_debug, error as log_error, info as log_info, warn as log_warn};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    database::{
        models::MeetingModel,
        repositories::{
            meeting::MeetingsRepository, setting::SettingsRepository,
            transcript::TranscriptsRepository,
        },
    },
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub title: String,
    /// RFC 3339 creation time, used by the home screen's recent list
    pub created_at: String,
    /// Protocol review status: "draft" or "approved"
    pub status: String,
    /// Length of the recording in seconds (end of the last transcript segment)
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptSearchResult {
    pub id: String,
    pub title: String,
    #[serde(rename = "matchContext")]
    pub match_context: String,
    pub timestamp: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveModelConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptConfig {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveTranscriptConfigRequest {
    pub provider: String,
    pub model: String,
    #[serde(rename = "apiKey")]
    pub api_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingDetails {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    /// Protocol review status: "draft" or "approved"
    pub status: String,
    pub transcripts: Vec<MeetingTranscript>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingTranscript {
    pub id: String,
    pub text: String,
    pub timestamp: String,
    // Recording-relative timestamps for audio-transcript synchronization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Who spoke, when speakers were recognised
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub speaker: Option<String>,
    /// The user corrected this line by hand
    #[serde(default)]
    pub edited: bool,
    /// The speaker of this line was set by hand
    #[serde(default)]
    pub speaker_manual: bool,
    /// Marked «перевірено»
    #[serde(default)]
    pub reviewed: bool,
    /// Left out of the document («Не включати в документ»)
    #[serde(default)]
    pub excluded: bool,
}

/// Meeting metadata without transcripts (for pagination)
#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_path: Option<String>,
    /// Protocol review status: "draft" or "approved"
    pub status: String,
}

/// Paginated transcripts response with total count
#[derive(Debug, Serialize, Deserialize)]
pub struct PaginatedTranscriptsResponse {
    pub transcripts: Vec<MeetingTranscript>,
    pub total_count: i64,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub timestamp: String,
    // NEW: Recording-relative timestamps for playback synchronization
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: Option<String>,
    pub email: String,
    pub license_key: String,
    pub company: Option<String>,
    pub position: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub is_licensed: bool,
}

// Helper function to get auth token from store (optional)
#[allow(dead_code)]
// Helper function to get server address - now hardcoded
// Generic API call function with optional authentication
// API Commands for Tauri

#[tauri::command]
pub async fn api_get_meetings<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    auth_token: Option<String>,
) -> Result<Vec<Meeting>, String> {
    log_info!(
        "api_get_meetings called with auth_token(native) : {}",
        auth_token.is_some()
    );
    let pool = state.db_manager.pool();
    let meetings: Result<Vec<MeetingModel>, sqlx::Error> =
        MeetingsRepository::get_meetings(pool).await;

    // Durations for the Протоколи list; a failure here only hides the column
    let durations: std::collections::HashMap<String, f64> = sqlx::query_as::<_, (String, Option<f64>)>(
        "SELECT meeting_id, MAX(audio_end_time) FROM transcripts GROUP BY meeting_id",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .filter_map(|(id, end)| end.map(|e| (id, e)))
    .collect();

    match meetings {
        Ok(meeting_models) => {
            log_info!("Successfully got {} meetings", meeting_models.len());

            let result: Vec<Meeting> = meeting_models
                .into_iter()
                .map(|m| Meeting {
                    duration_seconds: durations.get(&m.id).copied(),
                    id: m.id,
                    title: m.title,
                    created_at: m.created_at.0.to_rfc3339(),
                    status: m.status,
                })
                .collect();
            Ok(result)
        }
        Err(e) => {
            log_error!("Error getting meetings: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_search_transcripts<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    query: String,
    auth_token: Option<String>,
) -> Result<Vec<TranscriptSearchResult>, String> {
    log_info!(
        "api_search_transcripts called with query: '{}', auth_token: {}",
        query,
        auth_token.is_some()
    );

    let pool = state.db_manager.pool();

    match TranscriptsRepository::search_transcripts(pool, &query).await {
        Ok(results) => {
            log_info!(
                "Search completed successfully with {} results.",
                results.len()
            );
            Ok(results)
        }
        Err(e) => {
            log_error!("Error searching transcripts for query '{}': {}", query, e);
            Err(format!("Failed to search transcripts: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_get_model_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    _auth_token: Option<String>,
) -> Result<Option<ModelConfig>, String> {
    log_info!("api_get_model_config called (native)");
    let pool = state.db_manager.pool();
    // Gemma 4 for the document on Macs with 24 GB or more: a saved MamayLM switches once
    if let Ok(app_data_dir) = _app.path().app_data_dir() {
        crate::summary::summary_engine::preferred::apply_once(pool, &app_data_dir).await;
    }

    match SettingsRepository::get_model_config(pool).await {
        Ok(Some(mut config)) => {
            // Providers and built-in models removed in Протокол fall back to the local default model
            let removed_provider = matches!(config.provider.as_str(), "openai" | "claude" | "groq" | "openrouter" | "custom-openai");
            let unknown_builtin = config.provider == "builtin-ai"
                && crate::summary::summary_engine::get_model_by_name(&config.model).is_none();
            if removed_provider || unknown_builtin {
                let fallback = crate::summary::summary_engine::get_default_model().name;
                log_warn!(
                    "Model {}/{} is no longer offered; switching to builtin-ai/{}",
                    config.provider, config.model, fallback
                );
                config.provider = "builtin-ai".to_string();
                config.model = fallback;
                if let Err(e) = SettingsRepository::save_model_config(
                    pool,
                    &config.provider,
                    &config.model,
                    &config.whisper_model,
                    config.ollama_endpoint.as_deref(),
                )
                .await
                {
                    log_error!("Failed to save migrated model config: {}", e);
                }
            }
            log_info!(
                "✅ Found model config in database: provider={}, model={}, whisperModel={}, ollamaEndpoint={:?}",
                &config.provider,
                &config.model,
                &config.whisper_model,
                &config.ollama_endpoint
            );
            match SettingsRepository::get_api_key(pool, &config.provider).await {
                Ok(api_key) => {
                    log_info!("Successfully retrieved model config and API key.");
                    Ok(Some(ModelConfig {
                        provider: config.provider,
                        model: config.model,
                        whisper_model: config.whisper_model,
                        api_key,
                        ollama_endpoint: config.ollama_endpoint,
                    }))
                }
                Err(e) => {
                    log_error!(
                        "Failed to get API key for provider {}: {}",
                        &config.provider,
                        e
                    );
                    Err(e.to_string())
                }
            }
        }
        Ok(None) => {
            log_warn!("⚠️ No model config found in database - database may be empty or settings table not initialized");
            Ok(None)
        }
        Err(e) => {
            log_error!("❌ Failed to get model config from database: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_save_model_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    whisper_model: String,
    api_key: Option<String>,
    ollama_endpoint: Option<String>,
    _auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "💾 api_save_model_config called (native): provider='{}', model='{}', whisperModel='{}', ollamaEndpoint={:?}",
        &provider,
        &model,
        &whisper_model,
        &ollama_endpoint
    );
    let pool = state.db_manager.pool();

    if let Err(e) = SettingsRepository::save_model_config(
        pool,
        &provider,
        &model,
        &whisper_model,
        ollama_endpoint.as_deref(),
    )
    .await
    {
        log_error!("❌ Failed to save model config to database: {}", e);
        return Err(e.to_string());
    }

    if let Some(key) = api_key {
        if !key.is_empty() {
            log_info!("🔑 API key provided, saving...");
            if let Err(e) = SettingsRepository::save_api_key(pool, &provider, &key).await {
                log_error!("❌ Failed to save API key: {}", e);
                return Err(e.to_string());
            }
        }
    }

    // Trigger graceful shutdown of built-in AI sidecar if it's running
    // This ensures that if the user switched models/providers, the old one is cleaned up
    // The shutdown happens in the background, so it won't block the UI
    if let Err(e) = crate::summary::summary_engine::client::shutdown_sidecar_gracefully().await {
        log_warn!("Failed to initiate graceful sidecar shutdown: {}", e);
    }

    log_info!("✅ Successfully saved model configuration to database");
    Ok(
        serde_json::json!({ "status": "success", "message": "Model configuration saved successfully" }),
    )
}

#[tauri::command]
pub async fn api_get_transcript_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    _auth_token: Option<String>,
) -> Result<Option<TranscriptConfig>, String> {
    log_info!("api_get_transcript_config called (native)");
    let pool = state.db_manager.pool();

    match SettingsRepository::get_transcript_config(pool).await {
        Ok(Some(mut config)) => {
            // Parakeet and small/English-only Whisper models are no longer offered: move to the default
            let supported = config.provider == "localWhisper" && crate::config::is_supported_whisper_model(&config.model);
            let local_unsupported = config.provider == "parakeet" || config.provider == "localWhisper";
            if local_unsupported && !supported {
                log_warn!(
                    "Transcription model {}/{} is no longer offered; switching to localWhisper/{}",
                    config.provider, config.model, crate::config::DEFAULT_WHISPER_MODEL
                );
                config.provider = "localWhisper".to_string();
                config.model = crate::config::DEFAULT_WHISPER_MODEL.to_string();
                if let Err(e) = SettingsRepository::save_transcript_config(pool, &config.provider, &config.model).await {
                    log_error!("Failed to save migrated transcription config: {}", e);
                }
            }
            log_info!(
                "Found transcript config: provider={}, model={}",
                &config.provider,
                &config.model
            );
            match SettingsRepository::get_transcript_api_key(pool, &config.provider).await {
                Ok(api_key) => {
                    log_info!("Successfully retrieved transcript config and API key.");
                    Ok(Some(TranscriptConfig {
                        provider: config.provider,
                        model: config.model,
                        api_key,
                    }))
                }
                Err(e) => {
                    log_error!(
                        "Failed to get transcript API key for provider {}: {}",
                        &config.provider,
                        e
                    );
                    Err(e.to_string())
                }
            }
        }
        Ok(None) => {
            log_info!("No transcript config found, returning default.");
            Ok(Some(TranscriptConfig {
                provider: "localWhisper".to_string(),
                model: crate::config::DEFAULT_WHISPER_MODEL.to_string(),
                api_key: None,
            }))
        }
        Err(e) => {
            log_error!("Failed to get transcript config: {}", e);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_save_transcript_config<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    model: String,
    api_key: Option<String>,
    _auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_save_transcript_config called (native) for provider '{}'",
        &provider
    );
    let pool = state.db_manager.pool();

    if let Err(e) = SettingsRepository::save_transcript_config(pool, &provider, &model).await {
        log_error!("Failed to save transcript config: {}", e);
        return Err(e.to_string());
    }

    if let Some(key) = api_key {
        if !key.is_empty() {
            log_info!("API key provided, saving for transcript provider...");
            if let Err(e) = SettingsRepository::save_transcript_api_key(pool, &provider, &key).await
            {
                log_error!("Failed to save transcript API key: {}", e);
                return Err(e.to_string());
            }
        }
    }

    log_info!("Successfully saved transcript configuration.");
    Ok(
        serde_json::json!({ "status": "success", "message": "Transcript configuration saved successfully" }),
    )
}

#[tauri::command]
pub async fn api_get_transcript_api_key<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    _auth_token: Option<String>,
) -> Result<String, String> {
    log_info!(
        "api_get_transcript_api_key called (native) for provider '{}'",
        &provider
    );
    match SettingsRepository::get_transcript_api_key(&state.db_manager.pool(), &provider).await {
        Ok(key) => {
            log_info!(
                "Successfully retrieved transcript API key for provider '{}'.",
                &provider
            );
            Ok(key.unwrap_or_default())
        }
        Err(e) => {
            log_error!(
                "Failed to get transcript API key for provider '{}': {}",
                &provider,
                e
            );
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_delete_api_key<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    provider: String,
    _auth_token: Option<String>,
) -> Result<(), String> {
    log_info!(
        "log_api_delete_api_key called (native) for provider '{}'",
        &provider
    );
    match SettingsRepository::delete_api_key(&state.db_manager.pool(), &provider).await {
        Ok(_) => {
            log_info!("Successfully deleted API key for provider '{}'.", &provider);
            Ok(())
        }
        Err(e) => {
            log_error!(
                "Failed to delete API key for provider '{}': {}",
                &provider,
                e
            );
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn api_delete_meeting<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_delete_meeting called for meeting_id(native): {}, auth_token: {}",
        meeting_id,
        auth_token.is_some()
    );

    let pool = state.db_manager.pool();

    match MeetingsRepository::delete_meeting(pool, &meeting_id).await {
        Ok(true) => {
            log_info!("Successfully deleted meeting {}", meeting_id);
            Ok(serde_json::json!({
                "status": "success",
                "message": "Meeting deleted successfully"
            }))
        }
        Ok(false) => {
            log_warn!("Meeting not found or already deleted: {}", meeting_id);
            Err(format!(
                "Meeting not found or could not be deleted: {}",
                meeting_id
            ))
        }
        Err(e) => {
            log_error!("Error deleting meeting {}: {}", meeting_id, e);
            Err(format!("Failed to delete meeting: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_get_meeting<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
    auth_token: Option<String>,
) -> Result<MeetingDetails, String> {
    log_info!(
        "api_get_meeting called(native) for meeting_id: {}, auth_token: {}",
        meeting_id,
        auth_token.is_some()
    );

    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting(pool, &meeting_id).await {
        Ok(Some(meeting)) => {
            log_info!("Successfully retrieved meeting {}", meeting_id);
            Ok(meeting)
        }
        Ok(None) => {
            log_warn!("Meeting not found: {}", meeting_id);
            Err(format!("Meeting not found: {}", meeting_id))
        }
        Err(e) => {
            log_error!("Error retrieving meeting {}: {}", meeting_id, e);
            Err(format!("Failed to retrieve meeting: {}", e))
        }
    }
}

/// Get meeting metadata without transcripts (for pagination)
#[tauri::command]
pub async fn api_get_meeting_metadata<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingMetadata, String> {
    log_info!("api_get_meeting_metadata called for meeting_id: {}", meeting_id);

    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting_metadata(pool, &meeting_id).await {
        Ok(Some(meeting)) => {
            log_info!("Successfully retrieved meeting metadata {}", meeting_id);
            Ok(MeetingMetadata {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at.0.to_rfc3339(),
                updated_at: meeting.updated_at.0.to_rfc3339(),
                folder_path: meeting.folder_path,
                status: meeting.status,
            })
        }
        Ok(None) => {
            log_warn!("Meeting not found: {}", meeting_id);
            Err(format!("Meeting not found: {}", meeting_id))
        }
        Err(e) => {
            log_error!("Error retrieving meeting metadata {}: {}", meeting_id, e);
            Err(format!("Failed to retrieve meeting metadata: {}", e))
        }
    }
}

/// Get paginated transcripts for a meeting
#[tauri::command]
pub async fn api_get_meeting_transcripts<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    limit: i64,
    offset: i64,
    state: tauri::State<'_, AppState>,
) -> Result<PaginatedTranscriptsResponse, String> {
    log_info!(
        "api_get_meeting_transcripts called for meeting_id: {}, limit: {}, offset: {}",
        meeting_id,
        limit,
        offset
    );

    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting_transcripts_paginated(pool, &meeting_id, limit, offset).await {
        Ok((transcripts, total_count)) => {
            log_info!(
                "Successfully retrieved {} transcripts for meeting {} (total: {})",
                transcripts.len(),
                meeting_id,
                total_count
            );

            let excluded = crate::speakers::lines::excluded_ids(pool, &meeting_id).await.unwrap_or_default();
            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    excluded: excluded.contains(&t.id),
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                    speaker: t.speaker,
                    edited: t.original_transcript.is_some(),
                    speaker_manual: t.speaker_manual.unwrap_or(0) != 0,
                    reviewed: t.reviewed_at.is_some(),
                })
                .collect::<Vec<_>>();

            let has_more = (offset + meeting_transcripts.len() as i64) < total_count;

            Ok(PaginatedTranscriptsResponse {
                transcripts: meeting_transcripts,
                total_count,
                has_more,
            })
        }
        Err(e) => {
            log_error!("Error retrieving transcripts for meeting {}: {}", meeting_id, e);
            Err(format!("Failed to retrieve transcripts: {}", e))
        }
    }
}

/// Organisation / unit name for protocol headers ("" when not set)
#[tauri::command]
pub async fn api_get_organization_name(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let pool = state.db_manager.pool();
    SettingsRepository::get_flag(pool, crate::summary::ORGANIZATION_NAME_FLAG)
        .await
        .map(|value| value.unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn api_set_organization_name(state: tauri::State<'_, AppState>, name: String) -> Result<(), String> {
    let pool = state.db_manager.pool();
    SettingsRepository::set_flag(pool, crate::summary::ORGANIZATION_NAME_FLAG, name.trim())
        .await
        .map_err(|e| e.to_string())
}

/// Path of a meeting's audio recording, allowed for playback in the app.
/// Only the meeting's own folder (from the database) is opened to the
/// asset protocol, never an arbitrary path from the page.
#[tauri::command]
pub async fn api_meeting_audio_path<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Option<String>, String> {
    use tauri::Manager;
    let pool = state.db_manager.pool();
    let Some(meeting) = MeetingsRepository::get_meeting_metadata(pool, &meeting_id).await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let Some(folder) = meeting.folder_path.map(std::path::PathBuf::from).filter(|p| p.is_dir()) else {
        return Ok(None);
    };
    let audio = std::fs::read_dir(&folder)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "mp4" | "m4a" | "wav" | "mp3" | "ogg" | "webm"))
        })
        .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    let Some(audio) = audio else { return Ok(None) };
    app.asset_protocol_scope().allow_file(&audio).map_err(|e| e.to_string())?;
    Ok(Some(audio.to_string_lossy().to_string()))
}

/// Text to the system clipboard
#[tauri::command]
pub async fn api_copy_text(text: String) -> Result<(), String> {
    copy_text(&text).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn copy_text(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut pbcopy = Command::new("/usr/bin/pbcopy").env("LANG", "en_US.UTF-8").env("LC_ALL", "en_US.UTF-8").stdin(Stdio::piped()).spawn()?;
    pbcopy.stdin.take().ok_or(std::io::ErrorKind::BrokenPipe)?.write_all(text.as_bytes())?;
    let status = pbcopy.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("pbcopy failed"))
    }
}

#[cfg(not(target_os = "macos"))]
fn copy_text(_text: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("no system clipboard here"))
}

/// The clocks of a meeting's recording: see `audio::player_time`
#[tauri::command]
pub async fn api_player_time_points<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<(f64, f64)>, String> {
    let Some(path) = api_meeting_audio_path(app, state, meeting_id).await? else { return Ok(Vec::new()) };
    Ok(crate::audio::player_time::points_of(std::path::Path::new(&path)))
}

/// «Ховати вікно Протоколу під час показу екрана» (default on)
#[tauri::command]
pub async fn api_get_hide_from_screen_share(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let pool = state.db_manager.pool();
    SettingsRepository::get_flag(pool, crate::island::HIDE_FROM_SCREEN_SHARE_FLAG)
        .await
        .map(|value| value.as_deref() == Some("1"))
        .map_err(|e| e.to_string())
}

/// Stores the setting and applies it to the main window right away
#[tauri::command]
pub async fn api_set_hide_from_screen_share<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    SettingsRepository::set_flag(pool, crate::island::HIDE_FROM_SCREEN_SHARE_FLAG, if enabled { "1" } else { "0" })
        .await
        .map_err(|e| e.to_string())?;
    crate::island::apply_content_protection(&app, enabled);
    Ok(())
}

/// Sets the protocol review status of a meeting ("draft" or "approved").
#[tauri::command]
pub async fn api_set_meeting_status(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    status: String,
) -> Result<serde_json::Value, String> {
    if status != "draft" && status != "approved" {
        return Err(format!("Невідомий статус: {}", status));
    }
    let pool = state.db_manager.pool();
    match MeetingsRepository::update_meeting_status(pool, &meeting_id, &status).await {
        Ok(true) if status == "approved" => {
            // Learn from what the secretary approved (vs the app's draft)
            let approved = crate::database::repositories::summary::SummaryProcessesRepository::get_summary_data(pool, &meeting_id)
                .await
                .ok()
                .flatten()
                .and_then(|p| p.result)
                .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
                .and_then(|v| v.get("markdown").and_then(|m| m.as_str()).map(str::to_string));
            let learned = match approved {
                Some(md) => crate::summary::learning_store::learn_from_approval(pool, &meeting_id, &md).await.unwrap_or_else(|e| {
                    log_error!("Learning from the approval failed: {}", e);
                    Default::default()
                }),
                None => Default::default(),
            };
            Ok(serde_json::json!({ "status": status, "edits": learned.edits, "examples": learned.examples }))
        }
        Ok(true) => Ok(serde_json::json!({ "status": status })),
        Ok(false) => Err(format!("No meeting found with id {}", meeting_id)),
        Err(e) => {
            log_error!("Failed to update meeting status {}", e);
            Err(format!("Failed to update meeting status: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_save_meeting_title<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    title: String,
    auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_save_meeting_title called for meeting_id: {}, auth_token: {}",
        meeting_id,
        auth_token.is_some()
    );
    let pool = state.db_manager.pool();
    match MeetingsRepository::update_meeting_title(pool, &meeting_id, &title).await {
        Ok(true) => {
            log_info!("Successfully saved meeting title");
            Ok(serde_json::json!({"message": "Meeting title saved successfully"}))
        }
        Ok(false) => {
            log_error!("No meeting found with id {}", meeting_id);
            Err(format!("No meeting found with id {}", meeting_id))
        }
        Err(e) => {
            log_error!("Failed to update meeting {}", e);
            Err(format!("Failed to update meeting: {}", e))
        }
    }
}

#[tauri::command]
pub async fn api_save_transcript<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_title: String,
    transcripts: Vec<serde_json::Value>,
    folder_path: Option<String>,
    auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_save_transcript called for meeting: {}, transcripts: {}, folder_path: {:?}, auth_token: {}",
        meeting_title,
        transcripts.len(),
        folder_path,
        auth_token.is_some()
    );

    // Log first transcript for debugging
    if let Some(first) = transcripts.first() {
        log_debug!(
            "First transcript data: {}",
            serde_json::to_string_pretty(first).unwrap_or_default()
        );
    }

    // Convert serde_json::Value to TranscriptSegment
    let transcripts_to_save: Vec<TranscriptSegment> = transcripts
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            log_error!("Failed to parse transcript segments: {}", e);
            format!("Invalid transcript data format: {}. Please check the data structure.", e)
        })?;

    // Log parsed segments count and first segment details
    if let Some(first_seg) = transcripts_to_save.first() {
        log_debug!("First parsed segment: text='{}', audio_start_time={:?}, audio_end_time={:?}, duration={:?}",
                   first_seg.text.chars().take(50).collect::<String>(),
                   first_seg.audio_start_time,
                   first_seg.audio_end_time,
                   first_seg.duration);
    }

    let pool = state.db_manager.pool();

    // Now, call the repository with the correctly typed data.
    match TranscriptsRepository::save_transcript(
        pool,
        &meeting_title,
        &transcripts_to_save,
        folder_path,
    )
    .await
    {
        Ok(meeting_id) => {
            log_info!(
                "Successfully saved transcript and created meeting with id: {}",
                meeting_id
            );
            // Every window's lists show it, whichever stop saved it
            let _ = app.emit("meetings-changed", &meeting_id);
            Ok(serde_json::json!({
                "status": "success",
                "message": "Transcript saved successfully",
                "meeting_id": meeting_id
            }))
        }
        Err(e) => {
            log_error!(
                "Error saving transcript for meeting '{}': {}",
                meeting_title,
                e
            );
            Err(format!("Failed to save transcript: {}", e))
        }
    }
}

/// Opens the meeting's recording folder in the system file explorer
#[tauri::command]
pub async fn open_meeting_folder<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    log_info!("open_meeting_folder called for meeting_id: {}", meeting_id);

    let pool = state.db_manager.pool();

    // Get meeting with folder_path
    let meeting: Option<MeetingModel> = sqlx::query_as(
        "SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?",
    )
    .bind(&meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Database error: {}", e))?;

    match meeting {
        Some(m) => {
            if let Some(folder_path) = m.folder_path {
                log_info!("Opening meeting folder: {}", folder_path);

                // Verify folder exists
                let path = std::path::Path::new(&folder_path);
                if !path.exists() {
                    log_warn!("Folder path does not exist: {}", folder_path);
                    return Err(format!("Recording folder not found: {}", folder_path));
                }

                // Open folder based on OS
                #[cfg(target_os = "macos")]
                {
                    std::process::Command::new("open")
                        .arg(&folder_path)
                        .spawn()
                        .map_err(|e| format!("Failed to open folder: {}", e))?;
                }

                #[cfg(target_os = "windows")]
                {
                    std::process::Command::new("explorer")
                        .arg(&folder_path)
                        .spawn()
                        .map_err(|e| format!("Failed to open folder: {}", e))?;
                }

                #[cfg(target_os = "linux")]
                {
                    std::process::Command::new("xdg-open")
                        .arg(&folder_path)
                        .spawn()
                        .map_err(|e| format!("Failed to open folder: {}", e))?;
                }

                log_info!("Successfully opened folder: {}", folder_path);
                Ok(())
            } else {
                log_warn!("Meeting {} has no folder_path set", meeting_id);
                Err("Recording folder path not available for this meeting".to_string())
            }
        }
        None => {
            log_warn!("Meeting not found: {}", meeting_id);
            Err("Meeting not found".to_string())
        }
    }
}

// Simple test command to check backend connectivity


#[tauri::command]
pub async fn open_external_url<R: Runtime>(app: AppHandle<R>, url: String) -> Result<(), String> {
    use std::process::Command;

    // A Google Meet link never opens in a browser: Protokol's Meet window
    if crate::meet::is_meet_link(&url) {
        let start_screen = url::Url::parse(url.trim()).is_ok_and(|u| u.path() == "/");
        return crate::meet::open(&app, (!start_screen).then_some(url.as_str()), None);
    }

    let result = if cfg!(target_os = "windows") {
        Command::new("cmd").args(&["/C", "start", &url]).output()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(&url).output()
    } else {
        // Linux and other Unix-like systems
        Command::new("xdg-open").arg(&url).output()
    };

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to open URL: {}", e)),
    }
}
