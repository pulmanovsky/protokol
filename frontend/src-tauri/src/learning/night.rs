//! One whole training run, from the checked meetings to a model in use (or
//! not), and what the schedule needs to know from past runs.
//!
//! The work folder holds the pieces of the recordings and the trainer's
//! files only while the run lasts; it is removed at the end, whatever the
//! outcome. Only the accepted model and the history stay.
//!
//! After an accepted run the caller reloads recognition so the new model is
//! used from the next recording on.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use super::dataset::{self, PieceFile};
use super::gate;
use super::job::{Job, Progress};
use super::measure;
use super::runner::{self, End, Trainer};
use super::versions::{self, Outcome, Run};

/// Every this many training pieces one goes to watching the loss instead
const VALID_EVERY: usize = 25;

/// Where things are and what to run
#[derive(Debug, Clone)]
pub struct Setup {
    /// The app's Whisper models folder
    pub models_dir: PathBuf,
    /// The model the organisation uses and its file (`large-v3`, `ggml-large-v3.bin`)
    pub model_name: String,
    pub model_file: String,
    /// The original whisper.cpp model every run starts from
    pub base_weights: PathBuf,
    pub trainer: Trainer,
    /// A folder for work files, on the same disk as the models
    pub work_root: PathBuf,
    pub mac_memory_gb: f64,
}

/// What the run is doing, for the progress shown in Settings
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Stage {
    Preparing,
    Training { share: f64, epoch: u32, epochs: u32 },
    Checking,
    Finished { outcome: Outcome },
}

/// New checked speech (hours) since the last run that got as far as the check
/// and counted the speech as now; with none, all of it is new
pub fn new_hours(history: &[Run], checked_hours: f64) -> f64 {
    history
        .iter()
        .find(|r| matches!(r.outcome, Outcome::Accepted | Outcome::Rejected) && r.data_rule == super::dataset::DATA_RULE)
        .map(|r| (checked_hours - r.checked_hours).max(0.0))
        .unwrap_or(checked_hours)
}

/// Hours since the newest run, if it was stopped or failed (`history` newest first)
pub fn since_unfinished_hours(history: &[Run], now: chrono::DateTime<chrono::Local>) -> Option<f64> {
    let last = history.first()?;
    if !matches!(last.outcome, Outcome::Stopped | Outcome::Failed) {
        return None;
    }
    let at = chrono::NaiveDateTime::parse_from_str(&last.id, "%Y-%m-%dT%H-%M").ok()?;
    let at = at.and_local_timezone(chrono::Local).single()?;
    Some((now - at).num_minutes() as f64 / 60.0)
}

fn hours(pieces: &[PieceFile]) -> f64 {
    pieces.iter().map(|p| p.seconds).sum::<f64>() / 3600.0
}

/// Runs once. `prompt`: the prompt recognition uses now (dictionary words).
pub async fn run_once(
    pool: &sqlx::SqlitePool,
    setup: &Setup,
    prompt: String,
    recording: impl Fn() -> bool + Sync,
    stop_asked: impl Fn() -> bool + Sync,
    mut stage: impl FnMut(Stage) + Send,
) -> Result<Run> {
    let started = Instant::now();
    let id = versions::new_id(chrono::Local::now());
    let work = setup.work_root.join(&id);
    std::fs::create_dir_all(&work)?;
    stage(Stage::Preparing);
    let result = prepare_and_train(pool, setup, prompt, &id, &work, &recording, &stop_asked, &mut stage).await;
    let mut run = match result {
        Ok(r) => r,
        Err(e) => failed(&id, format!("{e}")),
    };
    run.minutes = started.elapsed().as_secs_f64() / 60.0;
    let candidate = work.join("ggml-model.bin");
    if run.outcome == Outcome::Accepted {
        versions::accept(&setup.models_dir, &setup.model_file, &candidate, &run)?;
    } else {
        versions::record(&setup.models_dir, &run)?;
    }
    let _ = std::fs::remove_dir_all(&work);
    stage(Stage::Finished { outcome: run.outcome });
    Ok(run)
}

fn failed(id: &str, reason: String) -> Run {
    Run {
        id: id.to_string(),
        outcome: Outcome::Failed,
        reason: Some(reason),
        train_hours: 0.0,
        held_hours: 0.0,
        train_meetings: 0,
        held_meetings: 0,
        checked_hours: 0.0,
        data_rule: super::dataset::DATA_RULE,
        wer_before: None,
        wer_after: None,
        per_meeting: Vec::new(),
        minutes: 0.0,
        peak_memory_gb: None,
        before: None,
        after: None,
        thresholds: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_and_train(
    pool: &sqlx::SqlitePool,
    setup: &Setup,
    prompt: String,
    id: &str,
    work: &Path,
    recording: &(impl Fn() -> bool + Sync),
    stop_asked: &(impl Fn() -> bool + Sync),
    stage: &mut (impl FnMut(Stage) + Send),
) -> Result<Run> {
    let meetings = dataset::meetings(pool).await?;
    let checked_hours = meetings.iter().map(|m| m.checked_s).sum::<f64>() / 3600.0;
    let split = dataset::split(&meetings).ok_or_else(|| anyhow!("too little checked speech"))?;

    // Pieces of the training meetings (rewritten lines left out) and of the held-out ones
    let mut train: Vec<PieceFile> = Vec::new();
    for m in &split.train {
        let samples = match dataset::recording(pool, m).await {
            Ok(s) => s,
            Err(e) => {
                log::warn!("learning: a training meeting has no usable recording ({e})");
                continue;
            }
        };
        let rewritten = dataset::rewritten_lines(pool, m).await?;
        let pieces: Vec<_> = crate::quality::pairs::checked_pieces(pool, m).await?.into_iter().filter(|p| !p.lines.iter().any(|l| rewritten.contains(l))).collect();
        train.extend(dataset::write_pieces(&samples, &pieces, &work.join("train").join(m))?);
    }
    let mut held: Vec<Vec<PieceFile>> = Vec::new();
    for m in &split.held {
        let samples = dataset::recording(pool, m).await?;
        let pieces = crate::quality::pairs::checked_pieces(pool, m).await?;
        held.push(dataset::write_pieces(&samples, &pieces, &work.join("held").join(m))?);
    }
    let (valid, train): (Vec<_>, Vec<_>) = train.into_iter().enumerate().partition(|(k, _)| k % VALID_EVERY == VALID_EVERY - 1);
    let valid: Vec<PieceFile> = valid.into_iter().map(|(_, p)| p).collect();
    let train: Vec<PieceFile> = train.into_iter().map(|(_, p)| p).collect();

    let mut run = failed(id, String::new());
    run.reason = None;
    run.checked_hours = checked_hours;
    run.train_hours = hours(&train) + hours(&valid);
    run.held_hours = held.iter().map(|h| hours(h)).sum();
    run.train_meetings = split.train.len();
    run.held_meetings = split.held.len();

    // Training
    let mut job = Job::like_v5(setup.base_weights.clone(), work.to_path_buf(), prompt);
    job.precision = super::job::Precision::for_mac(setup.mac_memory_gb);
    dataset::write_jsonl(&job.train, &train)?;
    dataset::write_jsonl(&job.valid, &valid)?;
    let job_path = work.join("job.json");
    std::fs::write(&job_path, serde_json::to_string_pretty(&job)?)?;
    let epochs = job.epochs;
    let end = runner::run(&setup.trainer, &job_path, setup.mac_memory_gb, recording, stop_asked, |p| {
        if let Some(share) = super::job::share_done(p, epochs) {
            let epoch = match p {
                Progress::Step { epoch, .. } => *epoch,
                _ => epochs,
            };
            stage(Stage::Training { share, epoch, epochs });
        }
    })
    .await?;
    match end {
        End::Done { peak_memory_gb, .. } => run.peak_memory_gb = Some(peak_memory_gb),
        End::Watched { why } => {
            run.outcome = Outcome::Stopped;
            run.reason = Some(format!("{why:?}"));
            return Ok(run);
        }
        End::StoppedByUser => {
            run.outcome = Outcome::Stopped;
            run.reason = Some("by the user".into());
            return Ok(run);
        }
        End::Failed { reason } => {
            run.reason = Some(reason);
            return Ok(run);
        }
    }

    // The check: the model in use and the new one on the held-out meetings
    stage(Stage::Checking);
    let (current, bars_now) = measure::hear(setup.models_dir.clone(), &setup.model_name, &held, recording).await?;
    let folder = measure::model_folder(work, "candidate", &setup.model_file, &job.ggml_out)?;
    let (candidate, _) = measure::hear(folder, &setup.model_name, &held, recording).await?;
    let verdict = gate::decide(&measure::scores(&current), &measure::scores(&candidate));
    run.wer_before = Some(verdict.before);
    run.wer_after = Some(verdict.after);
    run.per_meeting = verdict.per_meeting.clone();
    // The new model's underlines point to as many of its mistakes as the
    // ones in use do now, with bars of its own
    let dictionary = measure::dictionary_words(&crate::ukrainian::glossary_terms());
    let before = measure::detail(&current, bars_now, &dictionary);
    let bars = measure::tune(&candidate, before.caught_share());
    run.before = Some(before);
    run.after = Some(measure::detail(&candidate, bars, &dictionary));
    run.thresholds = Some(bars);
    if verdict.accept {
        run.outcome = Outcome::Accepted;
        // `run_once` moves it in under the version's folder
        if job.ggml_out != work.join("ggml-model.bin") {
            std::fs::rename(&job.ggml_out, work.join("ggml-model.bin"))?;
        }
    } else {
        run.outcome = Outcome::Rejected;
        run.reason = Some(format!("{:?}", verdict.reason));
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, outcome: Outcome, checked_hours: f64) -> Run {
        let mut run = failed(id, String::new());
        run.outcome = outcome;
        run.checked_hours = checked_hours;
        run
    }

    #[test]
    fn new_speech_counts_from_the_last_run_that_reached_the_check() {
        assert_eq!(new_hours(&[], 3.0), 3.0);
        let h = vec![r("2026-11-23T01-30", Outcome::Failed, 0.0), r("2026-11-22T01-30", Outcome::Rejected, 5.0), r("2026-11-15T01-30", Outcome::Accepted, 3.75)];
        assert!((new_hours(&h, 6.5) - 1.5).abs() < 1e-9);
        // A run of 0.4.0 counted older meetings too: all speech counted now is new
        let mut old = r("2026-11-16T01-30", Outcome::Rejected, 5.0);
        old.data_rule = 0;
        assert!((new_hours(&[old], 2.2) - 2.2).abs() < 1e-9);
    }

    #[test]
    fn a_stopped_run_waits_before_the_next_try() {
        use chrono::TimeZone;
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 23, 9, 30, 0).single().unwrap();
        let h = vec![r("2026-11-23T01-30", Outcome::Stopped, 5.0)];
        assert_eq!(since_unfinished_hours(&h, now), Some(8.0));
        let h = vec![r("2026-11-23T01-30", Outcome::Accepted, 5.0)];
        assert_eq!(since_unfinished_hours(&h, now), None);
    }
}
