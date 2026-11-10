use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
// Removed unused import

// Performance optimization: Conditional logging macros for hot paths
#[cfg(debug_assertions)]
macro_rules! perf_debug {
    ($($arg:tt)*) => {
        log::debug!($($arg)*)
    };
}

#[cfg(not(debug_assertions))]
macro_rules! perf_debug {
    ($($arg:tt)*) => {};
}

#[cfg(debug_assertions)]
macro_rules! perf_trace {
    ($($arg:tt)*) => {
        log::trace!($($arg)*)
    };
}

#[cfg(not(debug_assertions))]
macro_rules! perf_trace {
    ($($arg:tt)*) => {};
}

// Make these macros available to other modules
pub(crate) use perf_debug;
pub(crate) use perf_trace;

// Re-export async logging macros for external use (removed due to macro conflicts)

// Declare audio module
pub mod analytics;
pub mod api;
pub mod audio;
pub mod config;
pub mod console_utils;
pub mod database;
pub mod notifications;
pub mod ollama;
pub mod onboarding;
pub mod parakeet_engine;
pub mod state;
pub mod summary;
pub mod tray;
pub mod app_menu;
pub mod updates;
pub mod logfile;
pub mod identity;
pub mod learned_events;
pub mod waveform;
pub mod utils;
pub mod ukrainian;
pub mod speakers;
pub mod calendar;
pub mod whisper_engine;
pub mod power;
pub mod island;

#[cfg(test)]
mod eval_pipeline;

use audio::{list_audio_devices, AudioDevice, trigger_audio_permission};
use log::{error as log_error, info as log_info};
use notifications::commands::NotificationManagerState;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::RwLock;

static RECORDING_FLAG: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
static ONNX_RUNTIME_INIT_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub(crate) fn ensure_onnx_runtime_available() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    if let Some(error) = ONNX_RUNTIME_INIT_ERROR.get() {
        anyhow::bail!("{error}");
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn catch_onnx_runtime_init<F, E>(init: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), E>,
    E: std::fmt::Display,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(init)) {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            Err(format!("ONNX Runtime initialization panicked: {message}"))
        }
    }
}

#[cfg(target_os = "windows")]
fn record_onnx_runtime_failure(error: String) {
    log::error!("{error}");
    let _ = ONNX_RUNTIME_INIT_ERROR.set(error);
}

#[cfg(all(test, target_os = "windows"))]
mod onnx_runtime_tests {
    use super::catch_onnx_runtime_init;

    #[test]
    fn catch_onnx_runtime_init_converts_errors_and_panics() {
        assert!(catch_onnx_runtime_init(|| Ok::<(), &str>(())).is_ok());
        assert_eq!(
            catch_onnx_runtime_init(|| Err::<(), _>("initializer error")),
            Err("initializer error".to_string())
        );

        let error = catch_onnx_runtime_init(|| -> Result<(), &str> {
            panic!("synthetic loader panic");
        })
        .unwrap_err();
        assert!(error.contains("synthetic loader panic"));
    }
}

// Global language preference storage. This build transcribes Ukrainian by
// default: automatic detection often mislabels surzhyk as Russian.
// "auto" stays available in Settings and then really auto-detects.
static LANGUAGE_PREFERENCE: std::sync::LazyLock<StdMutex<String>> =
    std::sync::LazyLock::new(|| StdMutex::new(crate::ukrainian::DEFAULT_LANGUAGE.to_string()));

#[derive(Debug, Deserialize)]
struct RecordingArgs {
    save_path: String,
}

#[derive(Debug, Serialize, Clone)]
struct TranscriptionStatus {
    chunks_in_queue: usize,
    is_processing: bool,
    last_activity_ms: u64,
}

#[tauri::command]
async fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    log_info!("🔥 CALLED start_recording with meeting: {:?}", meeting_name);
    log_info!(
        "📋 Backend received parameters - mic: {:?}, system: {:?}, meeting: {:?}",
        mic_device_name,
        system_device_name,
        meeting_name
    );

    if is_recording().await {
        return Err("Recording already in progress".to_string());
    }

    // Call the actual audio recording system with meeting name
    match audio::recording_commands::start_recording_with_devices_and_meeting(
        app.clone(),
        mic_device_name,
        system_device_name,
        meeting_name.clone(),
    )
    .await
    {
        Ok(_) => {
            RECORDING_FLAG.store(true, Ordering::SeqCst);
            tray::update_tray_menu(&app);

            log_info!("Recording started successfully");

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to start audio recording: {}", e);
            Err(format!("Failed to start recording: {}", e))
        }
    }
}

#[tauri::command]
async fn stop_recording<R: Runtime>(app: AppHandle<R>, args: RecordingArgs) -> Result<(), String> {
    log_info!("Attempting to stop recording...");

    // Check the actual audio recording system state instead of the flag
    if !audio::recording_commands::is_recording().await {
        log_info!("Recording is already stopped");
        return Ok(());
    }

    // Call the actual audio recording system to stop
    match audio::recording_commands::stop_recording(
        app.clone(),
        audio::recording_commands::RecordingArgs {
            save_path: args.save_path.clone(),
        },
    )
    .await
    {
        Ok(_) => {
            RECORDING_FLAG.store(false, Ordering::SeqCst);
            tray::update_tray_menu(&app);

            // Create the save directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(&args.save_path).parent() {
                if !parent.exists() {
                    log_info!("Creating directory: {:?}", parent);
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        let err_msg = format!("Failed to create save directory: {}", e);
                        log_error!("{}", err_msg);
                        return Err(err_msg);
                    }
                }
            }

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to stop audio recording: {}", e);
            // Still update the flag even if stopping failed
            RECORDING_FLAG.store(false, Ordering::SeqCst);
            tray::update_tray_menu(&app);
            Err(format!("Failed to stop recording: {}", e))
        }
    }
}

#[tauri::command]
async fn is_recording() -> bool {
    audio::recording_commands::is_recording().await
}

#[tauri::command]
fn get_transcription_status() -> TranscriptionStatus {
    TranscriptionStatus {
        chunks_in_queue: 0,
        is_processing: false,
        last_activity_ms: 0,
    }
}

#[tauri::command]
fn read_audio_file(file_path: String) -> Result<Vec<u8>, String> {
    match std::fs::read(&file_path) {
        Ok(data) => Ok(data),
        Err(e) => Err(format!("Failed to read audio file: {}", e)),
    }
}

#[tauri::command]
async fn save_transcript(file_path: String, content: String) -> Result<(), String> {
    log_info!("Saving transcript to: {}", file_path);

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(&file_path).parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
    }

    // Write content to file
    std::fs::write(&file_path, content)
        .map_err(|e| format!("Failed to write transcript: {}", e))?;

    log_info!("Transcript saved successfully");
    Ok(())
}

// Audio level monitoring commands
#[tauri::command]
async fn start_audio_level_monitoring<R: Runtime>(
    app: AppHandle<R>,
    device_names: Vec<String>,
) -> Result<(), String> {
    log_info!(
        "Starting audio level monitoring for devices: {:?}",
        device_names
    );

    audio::simple_level_monitor::start_monitoring(app, device_names)
        .await
        .map_err(|e| format!("Failed to start audio level monitoring: {}", e))
}

#[tauri::command]
async fn stop_audio_level_monitoring() -> Result<(), String> {
    log_info!("Stopping audio level monitoring");

    audio::simple_level_monitor::stop_monitoring()
        .await
        .map_err(|e| format!("Failed to stop audio level monitoring: {}", e))
}

#[tauri::command]
async fn is_audio_level_monitoring() -> bool {
    audio::simple_level_monitor::is_monitoring()
}

// Analytics commands are now handled by analytics::commands module

// Whisper commands are now handled by whisper_engine::commands module

#[tauri::command]
async fn get_audio_devices() -> Result<Vec<AudioDevice>, String> {
    list_audio_devices()
        .await
        .map_err(|e| format!("Failed to list audio devices: {}", e))
}

#[tauri::command]
async fn trigger_microphone_permission() -> Result<bool, String> {
    trigger_audio_permission()
        .map_err(|e| format!("Failed to trigger microphone permission: {}", e))
}

#[tauri::command]
async fn start_recording_with_devices<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_and_meeting(app, mic_device_name, system_device_name, None).await
}

#[tauri::command]
async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    log_info!("🚀 CALLED start_recording_with_devices_and_meeting - Mic: {:?}, System: {:?}, Meeting: {:?}",
             mic_device_name, system_device_name, meeting_name);

    // Call the recording module functions that support meeting names
    let recording_result = match (mic_device_name.clone(), system_device_name.clone()) {
        (None, None) => {
            log_info!(
                "No devices specified, starting with defaults and meeting: {:?}",
                meeting_name
            );
            audio::recording_commands::start_recording_with_meeting_name(app.clone(), meeting_name)
                .await
        }
        _ => {
            log_info!(
                "Starting with specified devices: mic={:?}, system={:?}, meeting={:?}",
                mic_device_name,
                system_device_name,
                meeting_name
            );
            audio::recording_commands::start_recording_with_devices_and_meeting(
                app.clone(),
                mic_device_name,
                system_device_name,
                meeting_name,
            )
            .await
        }
    };

    match recording_result {
        Ok(_) => {
            log_info!("Recording started successfully via tauri command");

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to start recording via tauri command: {}", e);
            Err(e)
        }
    }
}

#[tauri::command]
async fn set_language_preference(language: String) -> Result<(), String> {
    let mut lang_pref = LANGUAGE_PREFERENCE
        .lock()
        .map_err(|e| format!("Failed to set language preference: {}", e))?;
    log_info!("Setting language preference to: {}", language);
    *lang_pref = language;
    Ok(())
}

// Internal helper function to get language preference (for use within Rust code)
pub fn get_language_preference_internal() -> Option<String> {
    LANGUAGE_PREFERENCE.lock().ok().map(|lang| lang.clone())
}

pub fn run() {
    log::set_max_level(log::LevelFilter::Info);

    let mut builder = tauri::Builder::default();

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            log_info!(
                "Second app instance requested with args: {:?}, cwd: {:?}",
                args,
                cwd
            );

            tray::focus_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(whisper_engine::parallel_commands::ParallelProcessorState::new())
        .manage(Arc::new(RwLock::new(
            None::<notifications::manager::NotificationManager<tauri::Wry>>,
        )) as NotificationManagerState<tauri::Wry>)
        .manage(audio::init_system_audio_state())
        .manage(summary::summary_engine::ModelManagerState(Arc::new(tokio::sync::Mutex::new(None))))
        .setup(|_app| {
            learned_events::init(_app.handle());
            // The menu bar, as Mac apps have it
            #[cfg(target_os = "macos")]
            match app_menu::build(_app.handle()) {
                Ok(menu) => {
                    if let Err(e) = _app.set_menu(menu) {
                        log::warn!("Menu bar not set: {e}");
                    }
                    _app.on_menu_event(|app, event| app_menu::on_event(app, event.id().as_ref()));
                }
                Err(e) => log::warn!("Menu bar not built: {e}"),
            }
            // Test runs only, in builds with the `test-hooks` feature (never the release dmg)
            #[cfg(feature = "test-hooks")]
            test_hooks(_app.handle());
            // A newer version, looked for once a day when allowed in Settings
            updates::check_daily(_app.handle().clone());
            // Where the word aligner's model is, for work that has no app handle
            let _ = speakers::aligner::model_dir(_app.handle());
            #[cfg(target_os = "windows")]
            match _app.path().resolve(
                "onnxruntime.dll",
                tauri::path::BaseDirectory::Resource,
            ) {
                Ok(runtime_path) => {
                    match catch_onnx_runtime_init(|| {
                        ort::init_from(runtime_path.to_string_lossy().into_owned())
                            .with_telemetry(false)
                            .commit()
                            .map(|_| ())
                    }) {
                        Ok(()) => log::info!(
                            "Initialized bundled ONNX Runtime from {}",
                            runtime_path.display()
                        ),
                        Err(error) => record_onnx_runtime_failure(format!(
                            "Failed to initialize bundled ONNX Runtime from {}: {}",
                            runtime_path.display(),
                            error
                        )),
                    }
                }
                Err(error) => record_onnx_runtime_failure(format!(
                    "Failed to resolve bundled ONNX Runtime resource: {}",
                    error
                )),
            };

            log::info!("Application setup complete");

            // Initialize system tray
            if let Err(e) = tray::create_tray(_app.handle()) {
                log::error!("Failed to create system tray: {}", e);
            }

            // Initialize notification system with proper defaults
            log::info!("Initializing notification system...");
            let app_for_notif = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let notif_state = app_for_notif.state::<NotificationManagerState<tauri::Wry>>();
                match notifications::commands::initialize_notification_manager(app_for_notif.clone()).await {
                    Ok(manager) => {
                        // Set default consent and permissions on first launch
                        if let Err(e) = manager.set_consent(true).await {
                            log::error!("Failed to set initial consent: {}", e);
                        }
                        if let Err(e) = manager.request_permission().await {
                            log::error!("Failed to request initial permission: {}", e);
                        }

                        // Store the initialized manager
                        let mut state_lock = notif_state.write().await;
                        *state_lock = Some(manager);
                        log::info!("Notification system initialized with default permissions");
                    }
                    Err(e) => {
                        log::error!("Failed to initialize notification manager: {}", e);
                    }
                }
            });

            // Set models directory to use app_data_dir (unified storage location)
            audio::live_levels::init(_app.handle().clone());
            island::init(_app.handle());
            // The main window can be captured (bug screenshots) unless the user
            // chose to hide it; the saved setting is applied by the UI
            island::apply_content_protection(_app.handle(), false);
            whisper_engine::commands::set_models_directory(&_app.handle());

            // Initialize Whisper engine on startup
            tauri::async_runtime::spawn(async {
                if let Err(e) = whisper_engine::commands::whisper_init().await {
                    log::error!("Failed to initialize Whisper engine on startup: {}", e);
                }
            });

            // Set Parakeet models directory
            parakeet_engine::commands::set_models_directory(&_app.handle());

            // Initialize Parakeet engine on startup
            tauri::async_runtime::spawn(async {
                if let Err(e) = parakeet_engine::commands::parakeet_init().await {
                    log::error!("Failed to initialize Parakeet engine on startup: {}", e);
                }
            });

            // Initialize ModelManager for summary engine (async, non-blocking)
            let app_handle_for_model_manager = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match summary::summary_engine::commands::init_model_manager_at_startup(&app_handle_for_model_manager).await {
                    Ok(_) => log::info!("ModelManager initialized successfully at startup"),
                    Err(e) => {
                        log::warn!("Failed to initialize ModelManager at startup: {}", e);
                        log::warn!("ModelManager will be lazy-initialized on first use");
                    }
                }
            });

            // Trigger system audio permission request on startup (similar to microphone permission)
            // #[cfg(target_os = "macos")]
            // {
            //     tauri::async_runtime::spawn(async {
            //         if let Err(e) = audio::permissions::trigger_system_audio_permission() {
            //             log::warn!("Failed to trigger system audio permission: {}", e);
            //         }
            //     });
            // }

            // Initialize database (handles first launch detection and conditional setup)
            tauri::async_runtime::block_on(async {
                database::setup::initialize_database_on_startup(&_app.handle()).await
            })
            .expect("Failed to initialize database");

            // Initialize bundled templates directory for dynamic template discovery
            log::info!("Initializing bundled templates directory...");
            if let Ok(resource_path) = _app.handle().path().resource_dir() {
                let templates_dir = resource_path.join("templates");
                log::info!("Setting bundled templates directory to: {:?}", templates_dir);
                summary::templates::set_bundled_templates_dir(templates_dir);
            } else {
                log::warn!("Failed to resolve resource directory for templates");
            }

            // Load the user's glossary (names, positions, abbreviations) used by
            // Ukrainian transcription and protocol generation.
            match _app.handle().path().app_data_dir() {
                Ok(dir) => {
                    ukrainian::init_glossary(&dir);
                    // The user's own meeting types: copies of built-in ones with other parts or fields
                    summary::templates::set_custom_templates_dir(dir.join("meeting-types"));
                    summary::pack::load(&dir);
                    // Meeting reminders from the user's calendar.
                    calendar::init(&dir);
                    calendar::start_reminder_loop(_app.handle().clone());
                }
                Err(e) => log::warn!("Failed to resolve app data dir for glossary: {}", e),
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if window.label() == "main" {
                    island::on_main_focus(window.app_handle(), *focused);
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    if let Err(e) = window.hide() {
                        log::error!("Failed to hide main window on close request: {}", e);
                    } else {
                        log::info!("Main window hidden to tray on close request");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            learned_events::api_learned_summary,
            learned_events::api_auto_fixes,
            learned_events::api_learned_rules,
            waveform::api_waveform,
            start_recording,
            stop_recording,
            is_recording,
            get_transcription_status,
            read_audio_file,
            save_transcript,
            analytics::commands::init_analytics,
            analytics::commands::disable_analytics,
            analytics::commands::track_event,
            analytics::commands::identify_user,
            analytics::commands::track_meeting_started,
            analytics::commands::track_recording_started,
            analytics::commands::track_recording_stopped,
            analytics::commands::track_meeting_deleted,
            analytics::commands::track_settings_changed,
            analytics::commands::track_feature_used,
            analytics::commands::is_analytics_enabled,
            analytics::commands::start_analytics_session,
            analytics::commands::end_analytics_session,
            analytics::commands::track_daily_active_user,
            analytics::commands::track_user_first_launch,
            analytics::commands::is_analytics_session_active,
            analytics::commands::track_summary_generation_started,
            analytics::commands::track_summary_generation_completed,
            analytics::commands::track_summary_regenerated,
            analytics::commands::track_model_changed,
            analytics::commands::track_custom_prompt_used,
            analytics::commands::track_meeting_ended,
            analytics::commands::track_analytics_enabled,
            analytics::commands::track_analytics_disabled,
            analytics::commands::track_analytics_transparency_viewed,
            whisper_engine::commands::whisper_init,
            whisper_engine::commands::whisper_get_available_models,
            whisper_engine::commands::whisper_load_model,
            whisper_engine::commands::whisper_get_current_model,
            whisper_engine::commands::whisper_is_model_loaded,
            whisper_engine::commands::whisper_has_available_models,
            whisper_engine::commands::whisper_validate_model_ready,
            whisper_engine::commands::whisper_transcribe_audio,
            whisper_engine::commands::whisper_get_models_directory,
            whisper_engine::commands::whisper_download_model,
            whisper_engine::commands::whisper_cancel_download,
            whisper_engine::commands::whisper_delete_corrupted_model,
            // Parakeet engine commands
            parakeet_engine::commands::parakeet_init,
            parakeet_engine::commands::parakeet_get_available_models,
            parakeet_engine::commands::parakeet_load_model,
            parakeet_engine::commands::parakeet_get_current_model,
            parakeet_engine::commands::parakeet_is_model_loaded,
            parakeet_engine::commands::parakeet_has_available_models,
            parakeet_engine::commands::parakeet_validate_model_ready,
            parakeet_engine::commands::parakeet_transcribe_audio,
            parakeet_engine::commands::parakeet_get_models_directory,
            parakeet_engine::commands::parakeet_download_model,
            parakeet_engine::commands::parakeet_retry_download,
            parakeet_engine::commands::parakeet_cancel_download,
            parakeet_engine::commands::parakeet_delete_corrupted_model,
            parakeet_engine::commands::open_parakeet_models_folder,
            // Parallel processing commands
            whisper_engine::parallel_commands::initialize_parallel_processor,
            whisper_engine::parallel_commands::start_parallel_processing,
            whisper_engine::parallel_commands::pause_parallel_processing,
            whisper_engine::parallel_commands::resume_parallel_processing,
            whisper_engine::parallel_commands::stop_parallel_processing,
            whisper_engine::parallel_commands::get_parallel_processing_status,
            whisper_engine::parallel_commands::get_system_resources,
            whisper_engine::parallel_commands::check_resource_constraints,
            whisper_engine::parallel_commands::calculate_optimal_workers,
            whisper_engine::parallel_commands::prepare_audio_chunks,
            whisper_engine::parallel_commands::test_parallel_processing_setup,
            get_audio_devices,
            trigger_microphone_permission,
            start_recording_with_devices,
            start_recording_with_devices_and_meeting,
            start_audio_level_monitoring,
            stop_audio_level_monitoring,
            is_audio_level_monitoring,
            audio::recording_commands::set_mic_muted,
            audio::recording_commands::get_mic_muted,
            audio::recording_commands::set_call_muted,
            audio::recording_commands::get_call_muted,
            audio::recording_commands::set_audio_sources,
            audio::recording_commands::get_audio_sources,
            audio::recording_commands::start_mic_preview,
            audio::recording_commands::stop_mic_preview,
            // Recording pause/resume commands
            audio::recording_commands::pause_recording,
            audio::recording_commands::resume_recording,
            audio::recording_commands::is_recording_paused,
            audio::recording_commands::get_recording_state,
            audio::recording_commands::get_meeting_folder_path,
            // Reload sync commands (retrieve transcript history and meeting name)
            audio::recording_commands::get_transcript_history,
            audio::recording_commands::get_recording_meeting_name,
            // Playback device detection (Bluetooth warning)
            audio::recording_commands::get_active_audio_output,
            // Audio recovery commands (for transcript recovery feature)
            audio::incremental_saver::recover_audio_from_checkpoints,
            audio::incremental_saver::cleanup_checkpoints,
            audio::incremental_saver::has_audio_checkpoints,
            console_utils::show_console,
            console_utils::hide_console,
            console_utils::toggle_console,
            ollama::get_ollama_models,
            ollama::pull_ollama_model,
            ollama::delete_ollama_model,
            ollama::get_ollama_model_context,
            api::api_get_meetings,
            api::api_set_meeting_status,
            api::api_get_organization_name,
            api::api_set_organization_name,
            power::api_is_low_power_mode,
            power::api_open_battery_settings,
            island::island_focus_main,
            island::island_action,
            island::island_set_width,
            island::island_has_native_glass,
            api::api_get_hide_from_screen_share,
            api::api_meeting_audio_path,
            api::api_player_time_points,
            api::api_copy_text,
            summary::pack::api_import_pack,
            api::api_set_hide_from_screen_share,
            api::api_search_transcripts,
            api::api_get_profile,
            api::api_save_profile,
            api::api_update_profile,
            api::api_get_model_config,
            api::api_save_model_config,
            api::api_get_api_key,
            // api::api_get_auto_generate_setting,
            // api::api_save_auto_generate_setting,
            api::api_get_transcript_config,
            api::api_save_transcript_config,
            api::api_get_transcript_api_key,
            api::api_delete_meeting,
            api::api_get_meeting,
            api::api_get_meeting_metadata,
            api::api_get_meeting_transcripts,
            speakers::commands::identify_speakers,
            speakers::commands::rename_speaker,
            speakers::commands::speaker_models_available,
            speakers::commands::get_meeting_speakers,
            speakers::commands::get_likely_same_speakers,
            speakers::commands::get_speaker_roles,
            speakers::commands::get_other_voices,
            speakers::commands::split_speaker_voice,
            speakers::commands::merge_speakers,
            speakers::overrides::get_speaker_choices,
            speakers::saved::get_saved_speakers,
            speakers::saved::get_saved_speaker_samples,
            speakers::saved::set_saved_speaker,
            speakers::saved::set_voice_sample_excluded,
            speakers::saved::merge_saved_speakers,
            speakers::saved::forget_saved_speakers,
            speakers::saved::restore_forgotten_speakers,
            speakers::overrides::reassign_line_speaker,
            speakers::overrides::undo_line_speaker,
            speakers::lines::get_line_words,
            speakers::lines::split_segment,
            speakers::lines::merge_segments,
            speakers::lines::assign_words_speaker,
            speakers::lines::set_segments_reviewed,
            speakers::lines::api_mark_all_reviewed,
            speakers::lines::get_review_progress,
            speakers::lines::find_voice_changes,
            speakers::lines::get_review_overview,
            speakers::journal::undo_edit,
            speakers::time_map::api_align_meeting_timeline,
            speakers::aligner::api_align_meeting_words,
            speakers::aligner::api_realign_line,
            speakers::aligner::api_align_focus,
            app_menu::api_confirm,
            app_menu::api_pick_folder,
            app_menu::api_import_models,
            updates::api_update_settings,
            updates::api_set_update_settings,
            updates::api_update_check,
            updates::api_update_install,
            updates::api_update_found,
            updates::api_update_from_folder,
            summary::numbers::api_check_document_numbers,
            speakers::aligner::api_perf_enabled,
            speakers::aligner::api_perf_log,
            speakers::aligner::api_get_word_highlight,
            speakers::aligner::api_set_word_highlight,
            ukrainian::commands::api_word_alternatives,
            speakers::journal::redo_edit,
            speakers::journal::get_edit_history,
            speakers::commands::set_speaker_role,
            speakers::commands::reject_speaker_suggestion,
            speakers::commands::start_speaker_recognition,
            speakers::commands::speaker_recognition_running,
            speakers::commands::speaker_auto_status,
            speakers::commands::stop_speaker_recognition,
            speakers::commands::get_auto_document,
            speakers::commands::set_auto_document,
            speakers::download::download_speaker_models,
            api::api_save_meeting_title,
            api::api_save_transcript,
            api::open_meeting_folder,
            api::test_backend_connection,
            api::debug_backend_connection,
            api::open_external_url,
            // Custom OpenAI commands
            api::api_save_custom_openai_config,
            api::api_get_custom_openai_config,
            api::api_test_custom_openai_connection,
            // Summary commands
            summary::commands::api_process_transcript,
            summary::commands::api_get_summary,
            summary::commands::api_save_meeting_summary,
            summary::commands::api_get_meeting_summary_language,
            summary::commands::api_save_meeting_summary_language,
            summary::commands::api_get_meeting_detected_summary_language,
            summary::commands::api_save_meeting_detected_summary_language,
            summary::commands::api_detect_transcript_summary_language,
            summary::commands::api_cancel_summary,
            // Template commands
            summary::template_commands::api_list_templates,
            summary::template_commands::api_suggest_template,
            summary::template_commands::api_get_meeting_type,
            summary::template_commands::api_learned_meetings,
            summary::template_commands::api_forget_learned_meeting,
            summary::template_commands::api_voice_profile_count,
            summary::template_commands::api_save_meeting_type,
            summary::template_commands::api_delete_meeting_type,
            summary::template_commands::api_export_meeting_type,
            summary::template_commands::api_document_variants,
            summary::template_commands::api_show_document_variant,
            summary::template_commands::api_shown_document,
            summary::template_commands::api_keep_shown_document,
            summary::template_commands::api_summary_stage,
            summary::template_commands::api_get_template_details,
            summary::template_commands::api_validate_template,
            ukrainian::commands::api_get_glossary,
            ukrainian::commands::api_edit_transcript_line,
            ukrainian::commands::api_get_doubts,
            ukrainian::commands::api_resolve_doubt,
            ukrainian::commands::api_undo_transcript_edit,
            ukrainian::commands::api_remember_corrections,
            ukrainian::commands::api_corrections,
            ukrainian::commands::api_stenogram_markdown,
            ukrainian::commands::api_stenogram_plain,
            ukrainian::commands::api_forget_inline_rule,
            ukrainian::commands::api_set_correction_enabled,
            ukrainian::commands::api_delete_correction,
            ukrainian::commands::api_apply_corrections_to_meeting,
            ukrainian::docx_export::api_export_protocol_docx,
            ukrainian::pdf_export::api_export_protocol_pdf,
            ukrainian::commands::api_set_glossary,
            ukrainian::commands::api_glossary_suggestions,
            ukrainian::commands::api_accept_glossary_suggestion,
            ukrainian::commands::api_dismiss_glossary_suggestion,
            ukrainian::commands::api_learned_terms,
            ukrainian::commands::api_forget_term,
            ukrainian::commands::api_learn_from_meetings,
            calendar::commands::api_calendar_status,
            calendar::commands::api_calendar_save_settings,
            calendar::commands::api_calendar_import_file,
            calendar::commands::api_calendar_remove_file,
            calendar::commands::api_calendar_refresh,
            // Built-in AI commands
            summary::summary_engine::commands::builtin_ai_list_models,
            summary::summary_engine::commands::builtin_ai_get_model_info,
            summary::summary_engine::commands::builtin_ai_download_model,
            summary::summary_engine::commands::builtin_ai_cancel_download,
            summary::summary_engine::commands::builtin_ai_delete_model,
            summary::summary_engine::commands::builtin_ai_is_model_ready,
            summary::summary_engine::commands::builtin_ai_get_available_summary_model,
            summary::summary_engine::commands::builtin_ai_get_recommended_model,
            audio::recording_preferences::get_recording_preferences,
            audio::recording_preferences::set_recording_preferences,
            audio::recording_preferences::get_default_recordings_folder_path,
            audio::recording_preferences::open_recordings_folder,
            audio::recording_preferences::select_recording_folder,
            audio::recording_preferences::get_available_audio_backends,
            audio::recording_preferences::get_current_audio_backend,
            audio::recording_preferences::set_audio_backend,
            audio::recording_preferences::get_audio_backend_info,
            // Language preference commands
            set_language_preference,
            // Notification system commands
            notifications::commands::get_notification_settings,
            notifications::commands::set_notification_settings,
            notifications::commands::request_notification_permission,
            notifications::commands::show_notification,
            notifications::commands::show_test_notification,
            notifications::commands::is_dnd_active,
            notifications::commands::get_system_dnd_status,
            notifications::commands::set_manual_dnd,
            notifications::commands::set_notification_consent,
            notifications::commands::clear_notifications,
            notifications::commands::is_notification_system_ready,
            notifications::commands::initialize_notification_manager_manual,
            notifications::commands::test_notification_with_auto_consent,
            notifications::commands::get_notification_stats,
            // System audio capture commands
            audio::system_audio_commands::start_system_audio_capture_command,
            audio::system_audio_commands::list_system_audio_devices_command,
            audio::system_audio_commands::check_system_audio_permissions_command,
            audio::system_audio_commands::start_system_audio_monitoring,
            audio::system_audio_commands::stop_system_audio_monitoring,
            audio::system_audio_commands::get_system_audio_monitoring_status,
            // Screen Recording permission commands
            audio::permissions::check_screen_recording_permission_command,
            audio::permissions::request_screen_recording_permission_command,
            audio::permissions::trigger_system_audio_permission_command,
            // Database import commands
            database::commands::check_first_launch,
            database::commands::select_legacy_database_path,
            database::commands::detect_legacy_database,
            database::commands::check_default_legacy_database,
            database::commands::check_homebrew_database,
            database::commands::import_and_initialize_database,
            database::commands::initialize_fresh_database,
            // Database and Models path commands
            database::commands::get_database_directory,
            database::commands::open_database_folder,
            whisper_engine::commands::open_models_folder,
            // Onboarding commands
            onboarding::get_onboarding_status,
            onboarding::save_onboarding_status_cmd,
            onboarding::reset_onboarding_status_cmd,
            onboarding::complete_onboarding,
            // System settings commands
            #[cfg(target_os = "macos")]
            utils::open_system_settings,
            // Retranscription commands
            audio::retranscription::start_retranscription_command,
            audio::retranscription::cancel_retranscription_command,
            audio::retranscription::is_retranscription_in_progress_command,
            // Import audio commands
            audio::import::select_and_validate_audio_command,
            audio::import::validate_audio_file_command,
            audio::import::start_import_audio_command,
            audio::import::cancel_import_command,
            audio::import::is_import_in_progress_command,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            match event {
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    tray::focus_main_window(_app_handle);
                }
                tauri::RunEvent::Exit => {
                    log::info!("Application exiting, cleaning up resources...");
                    tauri::async_runtime::block_on(async {
                        // Clean up database connection and checkpoint WAL
                        if let Some(app_state) = _app_handle.try_state::<state::AppState>() {
                            log::info!("Starting database cleanup...");
                            if let Err(e) = app_state.db_manager.cleanup().await {
                                log::error!("Failed to cleanup database: {}", e);
                            } else {
                                log::info!("Database cleanup completed successfully");
                            }
                        } else {
                            log::warn!("AppState not available for database cleanup (likely first launch)");
                        }

                        // Clean up sidecar
                        log::info!("Cleaning up sidecar...");
                        if let Err(e) = summary::summary_engine::force_shutdown_sidecar().await {
                            log::error!("Failed to force shutdown sidecar: {}", e);
                        }
                    });
                    log::info!("Application cleanup complete");
                }
                _ => {}
            }
        });
}

/// Test runs only: hooks that put the window in a state without clicks.
/// Compiled only with `--features test-hooks`; the release build has none of
/// them (no env var can make it run a script).
#[cfg(feature = "test-hooks")]
fn test_hooks(app: &tauri::AppHandle) {
    // Test runs only (PROTOKOL_TEST_ROUTE=/settings…, PROTOKOL_TEST_THEME=light|dark):
    // the window opens that page in that theme, so screens are checked without clicking
    if let Ok(route) = std::env::var("PROTOKOL_TEST_ROUTE") {
        let theme = std::env::var("PROTOKOL_TEST_THEME").unwrap_or_default();
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if let Some(w) = handle.get_webview_window("main") {
                let theme_js = if theme.is_empty() { String::new() } else { format!("localStorage.setItem('protokol-theme', {:?});", theme) };
                let _ = w.eval(&format!("{theme_js} window.location.assign({:?});", route));
            }
        });
    }
    // Test runs only (PROTOKOL_TEST_JS=<script>, PROTOKOL_TEST_JS_AFTER=<seconds>, 14 by
    // default): the script runs in the page, after the test route has loaded, to put a
    // screen in a state (a popover open, a panel shown) without synthetic clicks
    if let Ok(script) = std::env::var("PROTOKOL_TEST_JS") {
        let after: u64 = std::env::var("PROTOKOL_TEST_JS_AFTER").ok().and_then(|s| s.parse().ok()).unwrap_or(14);
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(after));
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.eval(&script);
            }
        });
    }
    // Test runs only (PROTOKOL_TEST_IMPORT=<audio file>): the import dialog opens with that file
    if let Ok(file) = std::env::var("PROTOKOL_TEST_IMPORT") {
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(6));
            if let Some(w) = handle.get_webview_window("main") {
                let _ = w.eval(&format!("window.dispatchEvent(new CustomEvent('protokol:test-import', {{ detail: {:?} }}))", file));
            }
        });
    }
}

/// Where tests on copies of real data must find them (never the live data)
#[cfg(test)]
pub(crate) fn test_copy_root() -> String {
    std::env::var("PROTOKOL_TEST_ROOT").unwrap_or_else(|_| "protokol-copies".to_string())
}
