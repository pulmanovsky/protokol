//! What Settings sees and does with learning, and the loop that starts a
//! run at night.
//!
//! In the app's data folder, `learning/`:
//! - `settings.json`: the mode, once the user answered the offer; when
//!   «Не зараз» was pressed; the last result the user has seen;
//! - `engine/`: the trainer, once installed ([`super::engine`]);
//! - `work/`: a run's files while it lasts.
//!
//! Models and the history of runs are under `models/tuned/` ([`super::versions`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Timelike;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::night::{self, Setup, Stage};
use super::runner;
use super::schedule::{self, Mode, Now, Wait};
use super::versions::{self, Active, Outcome, Run};
use super::{dataset, mac};

/// The model that is trained: the accurate one, which the organisation uses
const MODEL_NAME: &str = "large-v3";
const MODEL_FILE: &str = "ggml-large-v3.bin";
/// How often the loop looks whether a night run may start
const LOOK_EVERY: Duration = Duration::from_secs(300);
/// Runs shown in the history
const HISTORY_SHOWN: usize = 30;

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// The model in use changed: recognition loads it again once no recording is on
static RELOAD: AtomicBool = AtomicBool::new(false);
static STAGE: Mutex<Option<Stage>> = Mutex::new(None);
/// When the run on now started, and when its training began (for «Залишилося близько …»)
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
static TRAINING_SINCE: Mutex<Option<Instant>> = Mutex::new(None);

fn recording() -> bool {
    crate::audio::recording_commands::recording_now()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    /// `None` until the user answered the offer; until then runs start only by hand
    mode: Option<Mode>,
    /// When «Не зараз» was pressed (RFC 3339)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offer_later_at: Option<String>,
    /// The newest run whose result the user has seen (closed its notice)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seen_run: Option<String>,
}

#[derive(Debug, Clone)]
struct Paths {
    learning: PathBuf,
    models: PathBuf,
}

fn paths(app: &AppHandle) -> Result<Paths, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(Paths { learning: data.join("learning"), models: data.join("models") })
}

fn read_settings(learning: &Path) -> Settings {
    std::fs::read_to_string(learning.join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_settings(learning: &Path, s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(learning).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let tmp = learning.join("settings.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, learning.join("settings.json")).map_err(|e| e.to_string())
}

/// The original model the app downloaded: every run starts from it, never
/// from a fine-tuned one
fn original_model(models: &Path) -> Option<PathBuf> {
    let path = models.join(MODEL_FILE);
    std::fs::metadata(&path).is_ok_and(|m| m.len() > 100 * 1024 * 1024).then_some(path)
}

/// The Whisper model recognition is set to, `None` for another engine
async fn model_in_use(pool: &sqlx::SqlitePool) -> Option<String> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'").fetch_optional(pool).await.ok()?;
    match row {
        Some((provider, model)) if provider == "localWhisper" || provider == "whisper" => Some(model),
        Some(_) => None,
        None => Some(crate::config::DEFAULT_WHISPER_MODEL.to_string()),
    }
}

/// Checked speech (hours) in all meetings
async fn checked_hours(pool: &sqlx::SqlitePool) -> f64 {
    match dataset::meetings(pool).await {
        Ok(m) => m.iter().map(|m| m.checked_s).sum::<f64>() / 3600.0,
        Err(e) => {
            log::warn!("learning: could not count the checked speech: {e}");
            0.0
        }
    }
}

/// What [`schedule::check`] decides from, read now
async fn now(p: &Paths, pool: &sqlx::SqlitePool, by_hand: bool) -> Now {
    let history = versions::history(&p.models);
    let checked_hours = checked_hours(pool).await;
    let models = p.models.clone();
    let (memory_gb, free_disk_gb, on_power, idle_minutes) =
        tokio::task::spawn_blocking(move || (mac::memory_gb(), mac::free_disk_gb(&models), mac::on_power(), mac::idle_minutes()))
            .await
            .unwrap_or((0.0, 0.0, false, 0.0));
    let local = chrono::Local::now();
    Now {
        mode: read_settings(&p.learning).mode.unwrap_or(Mode::Manual),
        by_hand,
        apple_silicon: mac::apple_silicon(),
        memory_gb,
        engine_ready: super::engine::trainer(&p.learning).is_some() && original_model(&p.models).is_some(),
        model_fits: model_in_use(pool).await.as_deref() == Some(MODEL_NAME),
        free_disk_gb,
        checked_hours,
        new_hours: night::new_hours(&history, checked_hours),
        local_hour: local.hour(),
        on_power,
        idle_minutes,
        recording: recording(),
        since_unfinished_hours: night::since_unfinished_hours(&history, local),
    }
}

/// The prompt recognition uses now, so the model learns with it
async fn prompt() -> String {
    let engine = crate::whisper_engine::commands::WHISPER_ENGINE.lock().unwrap().as_ref().cloned();
    if let Some(engine) = engine {
        if let Some(p) = engine.initial_prompt(Some("uk")).await {
            return p;
        }
    }
    crate::ukrainian::whisper_initial_prompt(Some("uk"), &crate::ukrainian::glossary_terms()).unwrap_or_default()
}

/// Recognition loads the model now in use (after a run or a choice in Settings)
async fn reload_recognition() {
    let engine = crate::whisper_engine::commands::WHISPER_ENGINE.lock().unwrap().as_ref().cloned();
    if let Some(engine) = engine {
        match engine.reload_model().await {
            Ok(()) => log::info!("learning: recognition loaded the model now in use"),
            Err(e) => log::warn!("learning: recognition could not load the model now in use: {e}"),
        }
    }
}

/// Keeps the Mac from sleeping while a run lasts, and no longer than the app
fn keep_awake() -> Option<std::process::Child> {
    std::process::Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .spawn()
        .ok()
}

/// Starts a run in the background; `false` when one is on already or the
/// trainer is missing
fn start_run(app: AppHandle, pool: sqlx::SqlitePool, p: Paths) -> bool {
    let (Some(trainer), Some(base_weights)) = (super::engine::trainer(&p.learning), original_model(&p.models)) else { return false };
    if RUNNING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return false;
    }
    STOP.store(false, Ordering::SeqCst);
    *STAGE.lock().unwrap() = Some(Stage::Preparing);
    *STARTED.lock().unwrap() = Some(Instant::now());
    *TRAINING_SINCE.lock().unwrap() = None;
    tauri::async_runtime::spawn(async move {
        let setup = Setup {
            models_dir: p.models.clone(),
            model_name: MODEL_NAME.into(),
            model_file: MODEL_FILE.into(),
            base_weights,
            trainer,
            work_root: p.learning.join("work"),
            mac_memory_gb: tokio::task::spawn_blocking(mac::memory_gb).await.unwrap_or(0.0),
        };
        let mut awake = keep_awake();
        let prompt = prompt().await;
        let shown = app.clone();
        let result = night::run_once(&pool, &setup, prompt, recording, || STOP.load(Ordering::SeqCst), move |s: Stage| {
            if matches!(s, Stage::Training { .. }) {
                TRAINING_SINCE.lock().unwrap().get_or_insert_with(Instant::now);
            }
            *STAGE.lock().unwrap() = Some(s.clone());
            let _ = shown.emit("learning-stage", &s);
        })
        .await;
        if let Some(c) = awake.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        match result {
            Ok(run) => {
                log::info!("learning: run {} ended {:?} ({:?} → {:?})", run.id, run.outcome, run.wer_before, run.wer_after);
                if run.outcome == Outcome::Accepted {
                    RELOAD.store(true, Ordering::SeqCst);
                }
                let _ = app.emit("learning-finished", &run);
            }
            Err(e) => log::warn!("learning: the run could not be finished: {e}"),
        }
        *STAGE.lock().unwrap() = None;
        *STARTED.lock().unwrap() = None;
        *TRAINING_SINCE.lock().unwrap() = None;
        RUNNING.store(false, Ordering::SeqCst);
    });
    true
}

/// Started once at launch: stops what a crashed run left, then every few
/// minutes starts a night run when [`schedule::check`] allows, and has
/// recognition load a new model in use once no recording is on
pub fn start_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Ok(p) = paths(&app) {
            let work = p.learning.join("work");
            let _ = tokio::task::spawn_blocking(move || {
                runner::stop_leftovers(&work);
                let _ = std::fs::remove_dir_all(&work);
            })
            .await;
        }
        let mut last_look: Option<Instant> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if RELOAD.load(Ordering::SeqCst) && !recording() {
                RELOAD.store(false, Ordering::SeqCst);
                reload_recognition().await;
            }
            if RUNNING.load(Ordering::SeqCst) || last_look.is_some_and(|t| t.elapsed() < LOOK_EVERY) {
                continue;
            }
            let (Ok(p), Some(state)) = (paths(&app), app.try_state::<crate::state::AppState>()) else { continue };
            last_look = Some(Instant::now());
            let pool = state.db_manager.pool().clone();
            if schedule::check(&now(&p, &pool, false).await).is_ok() {
                log::info!("learning: a night run starts");
                start_run(app.clone(), pool, p);
            }
        }
    });
}

/// What the «Розпізнавання мовлення» group in Settings shows
#[derive(Debug, Serialize)]
pub struct Status {
    /// `None` until the user answered the offer
    pub mode: Option<Mode>,
    pub can_train_here: bool,
    pub engine_ready: bool,
    /// About how much the engine download is (MB), for the question before it
    pub engine_download_mb: u32,
    pub running: bool,
    pub stage: Option<Stage>,
    /// Why a night run would not start now (`None`: it would)
    pub wait: Option<Wait>,
    /// Why the button would not start a run now (`None`: it would)
    pub wait_by_hand: Option<Wait>,
    pub checked_hours: f64,
    pub new_hours: f64,
    /// `base`, `unset` or the id of the version in use
    pub active: String,
    /// A fine-tuned model is in use (a version, or the hand-copied one while nothing is chosen)
    pub tuned_in_use: bool,
    /// Accepted versions on disk with their numbers, newest first
    pub versions: Vec<Run>,
    /// Every run, newest first
    pub history: Vec<Run>,
    /// Ask «Донавчити розпізнавання на ваших нарадах?» now
    pub offer: bool,
    /// The newest run that ended with a verdict (in use, or not more
    /// accurate) whose notice the user has not closed yet
    pub unseen: Option<Run>,
    /// Minutes since the run on now started, and since its training began
    pub run_minutes: Option<f64>,
    pub training_minutes: Option<f64>,
    /// Percent of the engine download on now
    pub engine_download: Option<u32>,
    /// A model copied by hand is beside the original (one to go back to)
    pub hand_copied: bool,
}

fn days_since(rfc3339: Option<&str>) -> Option<f64> {
    let at = chrono::DateTime::parse_from_rfc3339(rfc3339?).ok()?;
    Some((chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_minutes() as f64 / 1440.0)
}

fn minutes_since(at: &Mutex<Option<Instant>>) -> Option<f64> {
    at.lock().unwrap().map(|t| t.elapsed().as_secs_f64() / 60.0)
}

#[tauri::command]
pub async fn api_learning_status(app: AppHandle, state: tauri::State<'_, crate::state::AppState>) -> Result<Status, String> {
    let p = paths(&app)?;
    let pool = state.db_manager.pool();
    let mut n = now(&p, pool, false).await;
    let wait = schedule::check(&n).err();
    n.by_hand = true;
    let wait_by_hand = schedule::check(&n).err();
    let mut history = versions::history(&p.models);
    history.truncate(HISTORY_SHOWN);
    let mut ids = versions::versions(&p.models, MODEL_FILE);
    ids.reverse();
    let settings = read_settings(&p.learning);
    let can_train_here = schedule::can_train_here(n.apple_silicon, n.memory_gb);
    let running = RUNNING.load(Ordering::SeqCst);
    Ok(Status {
        mode: settings.mode,
        offer: !running
            && schedule::offer(settings.mode, can_train_here, n.model_fits, n.checked_hours, days_since(settings.offer_later_at.as_deref())),
        unseen: versions::unseen(&history, settings.seen_run.as_deref()),
        run_minutes: minutes_since(&STARTED),
        training_minutes: minutes_since(&TRAINING_SINCE),
        engine_download: super::engine::downloading(),
        hand_copied: versions::hand_copied(&p.models, MODEL_FILE).is_some(),
        can_train_here,
        engine_ready: n.engine_ready,
        engine_download_mb: super::engine::DOWNLOAD_MB,
        running,
        stage: STAGE.lock().unwrap().clone(),
        wait,
        wait_by_hand,
        checked_hours: n.checked_hours,
        new_hours: n.new_hours,
        active: match versions::active(&p.models) {
            Active::Unset => "unset".into(),
            Active::Base => "base".into(),
            Active::Version(id) => id,
        },
        tuned_in_use: versions::model_file(&p.models, MODEL_FILE).is_some(),
        versions: ids.iter().filter_map(|id| versions::about(&p.models, id)).collect(),
        history,
    })
}

/// The answer to the offer, or a change in Settings
#[tauri::command]
pub fn api_learning_set_mode(app: AppHandle, mode: Mode) -> Result<(), String> {
    let p = paths(&app)?;
    let mut s = read_settings(&p.learning);
    s.mode = Some(mode);
    write_settings(&p.learning, &s)
}

/// «Не зараз» on the offer: it comes back a week later
#[tauri::command]
pub fn api_learning_offer_later(app: AppHandle) -> Result<(), String> {
    let p = paths(&app)?;
    let mut s = read_settings(&p.learning);
    s.offer_later_at = Some(chrono::Utc::now().to_rfc3339());
    write_settings(&p.learning, &s)
}

/// The user closed the notice of a run's result
#[tauri::command]
pub fn api_learning_seen(app: AppHandle, id: String) -> Result<(), String> {
    let p = paths(&app)?;
    let mut s = read_settings(&p.learning);
    if s.seen_run.as_deref().map_or(true, |seen| id.as_str() > seen) {
        s.seen_run = Some(id);
        write_settings(&p.learning, &s)?;
    }
    Ok(())
}

/// «Якість розпізнавання»: corrected words per checked meeting
#[tauri::command]
pub async fn api_learning_quality(app: AppHandle, state: tauri::State<'_, crate::state::AppState>) -> Result<super::corrected::Quality, String> {
    let p = paths(&app)?;
    super::corrected::quality(state.db_manager.pool(), &p.models, MODEL_FILE).await.map_err(|e| e.to_string())
}

/// «Покращити розпізнавання зараз»: `Some(wait)` says why a run cannot start now
#[tauri::command]
pub async fn api_learning_start_now(app: AppHandle, state: tauri::State<'_, crate::state::AppState>) -> Result<Option<Wait>, String> {
    if RUNNING.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let p = paths(&app)?;
    let pool = state.db_manager.pool().clone();
    if let Err(wait) = schedule::check(&now(&p, &pool, true).await) {
        return Ok(Some(wait));
    }
    log::info!("learning: a run started by hand");
    start_run(app, pool, p);
    Ok(None)
}

/// Stops the run on: nothing changes in recognition
#[tauri::command]
pub fn api_learning_stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// Which model recognition uses: `base` (the original), `unset` (the file
/// copied by hand) or a version's id
#[tauri::command]
pub fn api_learning_use(app: AppHandle, choice: String) -> Result<(), String> {
    let p = paths(&app)?;
    let choice = match choice.as_str() {
        "base" => Active::Base,
        "unset" => Active::Unset,
        _ => Active::Version(choice),
    };
    versions::set_active(&p.models, MODEL_FILE, &choice).map_err(|e| e.to_string())?;
    RELOAD.store(true, Ordering::SeqCst);
    Ok(())
}

/// Keeps a copy of the report as it left: `how` is `issue` or `file`,
/// `lines` the label and value of each line the user left in
#[tauri::command]
pub fn api_learning_report_sent(app: AppHandle, how: String, lines: Vec<(String, String)>) -> Result<(), String> {
    let p = paths(&app)?;
    super::report::keep_sent(&p.learning, chrono::Local::now(), &how, lines).map(|_| ()).map_err(|e| e.to_string())
}

/// «Зберегти у файл»: the report as plain text where the user chooses;
/// `false` when nothing was chosen
#[tauri::command]
pub async fn api_learning_report_save(app: AppHandle, lines: Vec<(String, String)>) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let p = paths(&app)?;
    let name = format!("Звіт про якість розпізнавання {}.txt", chrono::Local::now().format("%Y-%m-%d"));
    let picker = app.clone();
    let picked = tokio::task::spawn_blocking(move || picker.dialog().file().set_file_name(&name).add_filter("Текст", &["txt"]).blocking_save_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    std::fs::write(&path, super::report::as_text(&lines)).map_err(|e| e.to_string())?;
    super::report::keep_sent(&p.learning, chrono::Local::now(), "file", lines).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Every report sent, newest first, as it left
#[tauri::command]
pub fn api_learning_reports_sent(app: AppHandle) -> Result<Vec<super::report::Sent>, String> {
    Ok(super::report::sent(&paths(&app)?.learning))
}

/// The lines of the report on learning, for the user to look through
/// before sending any of them (`learning::report`)
#[tauri::command]
pub async fn api_learning_report(app: AppHandle, state: tauri::State<'_, crate::state::AppState>) -> Result<Vec<super::report::Line>, String> {
    let p = paths(&app)?;
    let (model, memory) = tokio::task::spawn_blocking(crate::feedback::hardware).await.unwrap_or_default();
    let mac = [model, memory].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    let applied = crate::learned_events::api_learned_summary(None).await.map(|s| s.applied.iter().map(|(_, n)| n).sum::<usize>()).unwrap_or(0);
    let undone = crate::ukrainian::corrections::all_corrections().iter().filter(|c| c.hint_only).count();
    Ok(super::report::lines(&super::report::Facts {
        app_version: app.package_info().version.to_string(),
        mac,
        checked_hours: checked_hours(state.db_manager.pool()).await,
        history: versions::history(&p.models),
        auto_fixes: Some((applied, undone)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_is_kept_and_unanswered_at_first() {
        let learning = std::env::temp_dir().join(format!("protokol-learning-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&learning);
        assert_eq!(read_settings(&learning).mode, None);
        write_settings(&learning, &Settings { mode: Some(Mode::Manual), ..Default::default() }).unwrap();
        assert_eq!(read_settings(&learning).mode, Some(Mode::Manual));
        let _ = std::fs::remove_dir_all(&learning);
    }
}
