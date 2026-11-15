//! Versions of the recognition model fine-tuned on this Mac, which one is in
//! use, and the history of every training run.
//!
//! Under the models folder:
//! - `tuned/<id>/<model file>`: a model the check accepted, with
//!   `tuned/<id>/about.json` (how it was made and measured);
//! - `tuned/active`: which model recognition uses, an id or `base` (the
//!   original model);
//! - `tuned/history.jsonl`: every run, accepted or not, numbers only;
//! - `tuned/changes.jsonl`: when recognition switched to which model, so
//!   «Якість розпізнавання» can tell which model heard a meeting.
//!
//! At most [`KEEP`] accepted models are kept: the one in use and the one
//! before it. The original model is never touched, so going back to it is
//! always possible. Without `tuned/active` the single file
//! `<name>-tuned.bin` beside the original (0.3.24, copied by hand) is used
//! as before.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// Accepted models kept on disk (each about 3 GB)
pub const KEEP: usize = 2;
/// Smaller files are not a real model (an empty or broken copy)
const MIN_MODEL_BYTES: u64 = 100 * 1024 * 1024;
const BASE: &str = "base";

/// How a run ended
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// More accurate on the held-out meetings: in use now
    Accepted,
    /// Not more accurate: the model in use stays
    Rejected,
    /// Stopped before the end (by the user, or the Mac was struggling)
    Stopped,
    /// Something broke; the reason says what
    Failed,
}

/// One training run, as kept in the history. Numbers only: no meeting
/// titles, texts or names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// `2026-11-15T01-30`: when it started, local time; sorts by time
    pub id: String,
    pub outcome: Outcome,
    /// Why it was not accepted, in a few words
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Checked speech trained on and held out (hours)
    pub train_hours: f64,
    pub held_hours: f64,
    pub train_meetings: usize,
    pub held_meetings: usize,
    /// All checked speech at the time of the run (hours): the next run waits
    /// for enough new speech after this
    pub checked_hours: f64,
    /// Mistakes per 100 words on the held-out meetings: the model in use and the new one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wer_before: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wer_after: Option<f64>,
    /// The same per held-out meeting, in date order
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_meeting: Vec<(f64, f64)>,
    pub minutes: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_memory_gb: Option<f64>,
    /// Kinds of mistakes and how the underlines did on the held-out
    /// meetings: the model in use and the new one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<super::measure::Detail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<super::measure::Detail>,
    /// The underline bars chosen for the new model (`measure::tune`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<crate::ukrainian::doubt::Thresholds>,
}

/// Which model recognition uses
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Active {
    /// Nothing chosen yet: the hand-copied `<name>-tuned.bin` if there is one
    Unset,
    /// The original model
    Base,
    /// An accepted version by its id
    Version(String),
}

fn tuned_dir(models: &Path) -> PathBuf {
    models.join("tuned")
}

fn is_model(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.len() > MIN_MODEL_BYTES).unwrap_or(false)
}

/// An id is a folder name made by this module: no path tricks
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id != BASE && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == 'T')
}

/// The id for a run starting now (local time)
pub fn new_id(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y-%m-%dT%H-%M").to_string()
}

pub fn active(models: &Path) -> Active {
    match std::fs::read_to_string(tuned_dir(models).join("active")) {
        Ok(s) => match s.trim() {
            BASE => Active::Base,
            id if valid_id(id) => Active::Version(id.to_string()),
            _ => Active::Unset,
        },
        Err(_) => Active::Unset,
    }
}

/// The model file recognition should load instead of `models/<filename>`:
/// `Some(path)` for a fine-tuned one, `None` for the original
pub fn model_file(models: &Path, filename: &str) -> Option<PathBuf> {
    match active(models) {
        Active::Base => None,
        Active::Version(id) => {
            let path = tuned_dir(models).join(id).join(filename);
            // A version folder that lost its model falls back to the original
            is_model(&path).then_some(path)
        }
        Active::Unset => hand_copied(models, filename),
    }
}

/// Written next to the old file and renamed over it, so a crash never
/// leaves half a file
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The file copied by hand beside the original (0.3.24), when it is there
pub fn hand_copied(models: &Path, filename: &str) -> Option<PathBuf> {
    let path = models.join(format!("{}-tuned.bin", filename.strip_suffix(".bin")?));
    is_model(&path).then_some(path)
}

/// Switches recognition to an accepted version, back to the original, or
/// (`Unset`) back to the file copied by hand
pub fn set_active(models: &Path, filename: &str, choice: &Active) -> Result<()> {
    let dir = tuned_dir(models);
    std::fs::create_dir_all(&dir)?;
    let value = match choice {
        Active::Base => BASE.to_string(),
        Active::Version(id) => {
            if !valid_id(id) || !is_model(&dir.join(id).join(filename)) {
                return Err(anyhow!("no model for version {id}"));
            }
            id.clone()
        }
        Active::Unset => {
            if hand_copied(models, filename).is_none() {
                return Err(anyhow!("no model copied by hand"));
            }
            match std::fs::remove_file(dir.join("active")) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            note_change(models, UNSET, chrono::Utc::now());
            return Ok(());
        }
    };
    write_atomic(&dir.join("active"), &value)?;
    note_change(models, &value, chrono::Utc::now());
    Ok(())
}

const UNSET: &str = "unset";

/// One switch of the model in use
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// RFC 3339, UTC
    pub at: String,
    /// `base`, `unset` (the file copied by hand) or a version's id
    pub model: String,
}

/// Adds a switch to `tuned/changes.jsonl`; a failure only costs the
/// colours of «Якість розпізнавання», so it is logged, not returned
fn note_change(models: &Path, model: &str, at: chrono::DateTime<chrono::Utc>) {
    use std::io::Write;
    let line = serde_json::to_string(&Change { at: at.to_rfc3339(), model: model.to_string() }).unwrap_or_default();
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tuned_dir(models).join("changes.jsonl"))
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = written {
        log::warn!("learning: the switch of the model was not noted: {e}");
    }
}

/// Every switch of the model in use, oldest first
pub fn changes(models: &Path) -> Vec<Change> {
    std::fs::read_to_string(tuned_dir(models).join("changes.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Whether a fine-tuned model was in use at `at`: the last switch before
/// it says; before any switch, the file copied by hand counts from when it
/// was copied (`copied_at`)
pub fn tuned_by(changes: &[Change], copied_at: Option<chrono::DateTime<chrono::Utc>>, at: chrono::DateTime<chrono::Utc>) -> bool {
    let last = changes
        .iter()
        .filter_map(|c| Some((chrono::DateTime::parse_from_rfc3339(&c.at).ok()?.with_timezone(&chrono::Utc), c.model.as_str())))
        .filter(|(t, _)| *t <= at)
        .max_by_key(|(t, _)| *t);
    match last {
        Some((_, BASE)) => false,
        Some((_, UNSET)) => copied_at.is_some(),
        Some(_) => true,
        None => copied_at.is_some_and(|c| c <= at),
    }
}

/// When the file copied by hand was put beside the original
pub fn copied_at(models: &Path, filename: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let path = hand_copied(models, filename)?;
    let m = std::fs::metadata(path).ok()?;
    m.created().or_else(|_| m.modified()).ok().map(chrono::DateTime::<chrono::Utc>::from)
}

/// Accepted versions on disk, oldest first
pub fn versions(models: &Path, filename: &str) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(tuned_dir(models))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|id| valid_id(id) && is_model(&tuned_dir(models).join(id).join(filename)))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// The run that made a version (its numbers)
pub fn about(models: &Path, id: &str) -> Option<Run> {
    if !valid_id(id) {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(tuned_dir(models).join(id).join("about.json")).ok()?).ok()
}

/// The underline bars of a model file: those chosen for the version it is
/// (its `about.json` beside it), the defaults for any other file
pub fn bars_beside(model: &Path) -> crate::ukrainian::doubt::Thresholds {
    model
        .parent()
        .and_then(|dir| std::fs::read_to_string(dir.join("about.json")).ok())
        .and_then(|text| serde_json::from_str::<Run>(&text).ok())
        .and_then(|run| run.thresholds)
        .unwrap_or(crate::ukrainian::doubt::Thresholds::DEFAULT)
}

/// Every run, newest first
pub fn history(models: &Path) -> Vec<Run> {
    let mut runs: Vec<Run> = std::fs::read_to_string(tuned_dir(models).join("history.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    runs.reverse();
    runs
}

/// The newest run with a verdict (in use, or not more accurate), if the
/// user has not closed its notice yet (`seen`: the newest run closed)
pub fn unseen(history: &[Run], seen: Option<&str>) -> Option<Run> {
    let newest = history.iter().find(|r| matches!(r.outcome, Outcome::Accepted | Outcome::Rejected))?;
    seen.map_or(true, |s| newest.id.as_str() > s).then(|| newest.clone())
}

/// Adds a run to the history
pub fn record(models: &Path, run: &Run) -> Result<()> {
    use std::io::Write;
    let dir = tuned_dir(models);
    std::fs::create_dir_all(&dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("history.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(run)?)?;
    Ok(())
}

/// A model the check accepted: moved in as a new version, put in use,
/// written to the history; versions beyond [`KEEP`] are removed (never the
/// one in use)
pub fn accept(models: &Path, filename: &str, candidate: &Path, run: &Run) -> Result<()> {
    if run.outcome != Outcome::Accepted || !valid_id(&run.id) {
        return Err(anyhow!("not an accepted run"));
    }
    if !is_model(candidate) {
        return Err(anyhow!("the new model file is missing or broken"));
    }
    let dir = tuned_dir(models).join(&run.id);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(filename);
    // Same disk in practice (both in the app's data folder); copy otherwise
    if std::fs::rename(candidate, &target).is_err() {
        std::fs::copy(candidate, &target).context("copy the new model")?;
        let _ = std::fs::remove_file(candidate);
    }
    write_atomic(&dir.join("about.json"), &serde_json::to_string_pretty(run)?)?;
    set_active(models, filename, &Active::Version(run.id.clone()))?;
    record(models, run)?;
    prune(models, filename);
    Ok(())
}

/// Keeps the [`KEEP`] newest versions and the one in use
fn prune(models: &Path, filename: &str) {
    let in_use = match active(models) {
        Active::Version(id) => Some(id),
        _ => None,
    };
    let all = versions(models, filename);
    let keep_from = all.len().saturating_sub(KEEP);
    for id in &all[..keep_from] {
        if Some(id) != in_use.as_ref() {
            let _ = std::fs::remove_dir_all(tuned_dir(models).join(id));
        }
    }
}

/// A run with the pilot's numbers, for tests
#[cfg(test)]
pub fn sample_run(id: &str, outcome: Outcome) -> Run {
    Run {
        id: id.into(),
        outcome,
        reason: None,
        train_hours: 3.0,
        held_hours: 0.75,
        train_meetings: 10,
        held_meetings: 3,
        checked_hours: 3.75,
        wer_before: Some(17.5),
        wer_after: Some(15.8),
        per_meeting: vec![(16.2, 14.2)],
        minutes: 50.0,
        peak_memory_gb: Some(14.0),
        before: None,
        after: None,
        thresholds: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "ggml-large-v3.bin";

    fn fake_model(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = std::fs::File::create(path).unwrap();
        f.set_len(MIN_MODEL_BYTES + 1).unwrap();
    }

    fn run(id: &str, outcome: Outcome) -> Run {
        Run {
            id: id.into(),
            outcome,
            reason: None,
            train_hours: 3.0,
            held_hours: 0.75,
            train_meetings: 10,
            held_meetings: 3,
            checked_hours: 3.75,
            wer_before: Some(17.5),
            wer_after: Some(15.8),
            per_meeting: vec![(16.2, 14.2)],
            minutes: 50.0,
            peak_memory_gb: Some(14.0),
            before: None,
            after: None,
            thresholds: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("protokol-versions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn without_a_choice_the_hand_copied_file_is_used() {
        let m = tmp("legacy");
        assert_eq!(model_file(&m, FILE), None);
        fake_model(&m.join("ggml-large-v3-tuned.bin"));
        assert_eq!(model_file(&m, FILE), Some(m.join("ggml-large-v3-tuned.bin")));
        set_active(&m, FILE, &Active::Base).unwrap();
        assert_eq!(model_file(&m, FILE), None, "«base» turns the hand-copied file off too");
    }

    #[test]
    fn an_accepted_model_is_used_and_two_are_kept() {
        let m = tmp("accept");
        for id in ["2026-11-15T01-30", "2026-11-22T01-30", "2026-11-29T01-30"] {
            let cand = m.join("work").join(id).join("new.bin");
            fake_model(&cand);
            accept(&m, FILE, &cand, &run(id, Outcome::Accepted)).unwrap();
            assert_eq!(model_file(&m, FILE), Some(m.join("tuned").join(id).join(FILE)));
            assert!(!cand.exists());
        }
        assert_eq!(versions(&m, FILE), vec!["2026-11-22T01-30", "2026-11-29T01-30"]);
        assert_eq!(history(&m).len(), 3);
        assert_eq!(history(&m)[0].id, "2026-11-29T01-30");
        assert_eq!(about(&m, "2026-11-29T01-30").map(|r| r.id), Some("2026-11-29T01-30".to_string()));
        assert_eq!(about(&m, "2026-11-15T01-30"), None, "a removed version has no numbers");
        assert_eq!(about(&m, "../tuned"), None);
    }

    #[test]
    fn a_version_brings_its_own_underline_bars() {
        use crate::ukrainian::doubt::Thresholds;
        let m = tmp("bars");
        let cand = m.join("w").join("new.bin");
        fake_model(&cand);
        let mut r = run("2026-11-22T01-30", Outcome::Accepted);
        r.thresholds = Some(Thresholds::from_normal(0.6));
        accept(&m, FILE, &cand, &r).unwrap();
        assert_eq!(bars_beside(&model_file(&m, FILE).unwrap()), Thresholds::from_normal(0.6));
        // The original and a hand-copied file keep the defaults
        assert_eq!(bars_beside(&m.join(FILE)), Thresholds::DEFAULT);
        assert_eq!(bars_beside(&m.join("ggml-large-v3-tuned.bin")), Thresholds::DEFAULT);
    }

    #[test]
    fn going_back_keeps_the_version_in_use_through_pruning() {
        let m = tmp("back");
        for id in ["2026-11-15T01-30", "2026-11-22T01-30"] {
            let cand = m.join("w").join(id).join("new.bin");
            fake_model(&cand);
            accept(&m, FILE, &cand, &run(id, Outcome::Accepted)).unwrap();
        }
        set_active(&m, FILE, &Active::Version("2026-11-15T01-30".into())).unwrap();
        let cand = m.join("w3").join("new.bin");
        fake_model(&cand);
        accept(&m, FILE, &cand, &run("2026-11-29T01-30", Outcome::Accepted)).unwrap();
        // the new one is in use; the oldest goes, since it is no longer in use
        assert_eq!(versions(&m, FILE), vec!["2026-11-22T01-30", "2026-11-29T01-30"]);
    }

    #[test]
    fn a_rejected_run_only_goes_to_the_history() {
        let m = tmp("reject");
        let mut r = run("2026-11-22T01-30", Outcome::Rejected);
        r.reason = Some("not more accurate".into());
        record(&m, &r).unwrap();
        assert_eq!(history(&m), vec![r.clone()]);
        assert!(accept(&m, FILE, &m.join("x.bin"), &r).is_err());
        assert_eq!(active(&m), Active::Unset);
    }

    #[test]
    fn going_back_to_the_hand_copied_file() {
        let m = tmp("unset");
        assert!(set_active(&m, FILE, &Active::Unset).is_err(), "nothing to go back to");
        fake_model(&m.join("ggml-large-v3-tuned.bin"));
        set_active(&m, FILE, &Active::Base).unwrap();
        assert_eq!(model_file(&m, FILE), None);
        set_active(&m, FILE, &Active::Unset).unwrap();
        assert_eq!(model_file(&m, FILE), Some(m.join("ggml-large-v3-tuned.bin")));
        assert_eq!(changes(&m).iter().map(|c| c.model.as_str()).collect::<Vec<_>>(), vec!["base", "unset"]);
    }

    #[test]
    fn which_model_heard_a_meeting() {
        use chrono::TimeZone;
        let t = |d: u32, h: u32| chrono::Utc.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap();
        let c = |d: u32, h: u32, model: &str| Change { at: t(d, h).to_rfc3339(), model: model.into() };
        // Nothing switched, nothing copied: the original
        assert!(!tuned_by(&[], None, t(20, 9)));
        // Copied by hand on the 15th: meetings after it, not before
        assert!(!tuned_by(&[], Some(t(15, 11)), t(14, 9)));
        assert!(tuned_by(&[], Some(t(15, 11)), t(16, 9)));
        // A version from the 22nd night, back to the original on the 24th
        let log = [c(22, 1, "2026-11-22T01-30"), c(24, 12, "base")];
        assert!(!tuned_by(&log, None, t(21, 9)));
        assert!(tuned_by(&log, None, t(23, 9)));
        assert!(!tuned_by(&log, None, t(25, 9)));
        assert!(tuned_by(&[c(24, 12, "unset")], Some(t(15, 11)), t(25, 9)));
    }

    #[test]
    fn a_strange_active_file_is_ignored() {
        let m = tmp("strange");
        std::fs::create_dir_all(m.join("tuned")).unwrap();
        std::fs::write(m.join("tuned/active"), "../../etc").unwrap();
        assert_eq!(active(&m), Active::Unset);
        assert!(set_active(&m, FILE, &Active::Version("2026-01-01T00-00".into())).is_err());
    }

    #[test]
    fn a_result_is_shown_until_its_notice_is_closed() {
        let run = sample_run;
        let history = vec![run("2026-11-23T01-30", Outcome::Stopped), run("2026-11-22T01-30", Outcome::Rejected), run("2026-11-15T01-30", Outcome::Accepted)];
        assert_eq!(unseen(&history, None).map(|r| r.id), Some("2026-11-22T01-30".to_string()));
        assert_eq!(unseen(&history, Some("2026-11-15T01-30")).map(|r| r.id), Some("2026-11-22T01-30".to_string()));
        assert_eq!(unseen(&history, Some("2026-11-22T01-30")), None);
        assert_eq!(unseen(&[run("2026-11-23T01-30", Outcome::Failed)], None), None, "a failed run has no notice");
    }
}
