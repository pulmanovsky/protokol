//! When a recording was paused, on the wall clock.
//!
//! The recording itself has no pauses in it: its sound goes on from where it
//! was paused. What is kept beside it by the wall clock (a Meet call's lit
//! tiles, its captions and its separate streams) is put on that same clock
//! without pauses by these moments. They are written to `pauses.json` in the
//! recording's folder whenever the recording is paused or goes on, so they
//! are still known when the recording is read later.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{info, warn};
use serde::{Deserialize, Serialize};

pub const FILE: &str = "pauses.json";

/// The paused spans [from, to) of one recording (wall clock, ms since 1970),
/// in order. A pause that never ended goes on for ever.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pauses(Vec<(f64, f64)>);

impl Pauses {
    pub fn new(mut spans: Vec<(f64, f64)>) -> Self {
        spans.retain(|(a, b)| b > a);
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out: Vec<(f64, f64)> = Vec::new();
        for (a, b) in spans {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        Pauses(out)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn count(&self) -> usize {
        self.0.len()
    }

    /// All the paused time (ms); a pause that never ended adds nothing
    pub fn total(&self) -> f64 {
        self.0.iter().filter(|(_, b)| b.is_finite()).map(|(a, b)| b - a).sum()
    }

    /// Was the recording paused at this moment?
    pub fn within(&self, t: f64) -> bool {
        self.0.iter().any(|(a, b)| t >= *a && t < *b)
    }

    /// The paused time (ms) before this moment
    fn before(&self, t: f64) -> f64 {
        self.0.iter().map(|(a, b)| (t.min(*b) - a).max(0.0)).sum()
    }

    /// A moment of the wall clock on the recording's clock (the wall clock
    /// with the pauses taken out); a moment inside a pause is the moment the
    /// pause began
    pub fn clamp(&self, t: f64) -> f64 {
        t - self.before(t)
    }

    /// A moment of the wall clock on the recording's clock; None inside a pause
    pub fn on_recording(&self, t: f64) -> Option<f64> {
        (!self.within(t)).then(|| self.clamp(t))
    }

    /// A span of sound that starts at `t` and lasts `len_ms`, judged by its
    /// middle: where it starts on the recording's clock, None when it
    /// sounded during a pause
    pub fn span_on_recording(&self, t: f64, len_ms: f64) -> Option<f64> {
        let middle = t + len_ms / 2.0;
        (!self.within(middle)).then(|| t - self.before(middle))
    }
}

#[derive(Serialize, Deserialize)]
struct Kept {
    v: u32,
    /// [from, to] of each pause; `to` is null while the pause lasts
    pauses: Vec<(f64, Option<f64>)>,
}

/// The recording that runs now (or ran last) and its pauses
struct Live {
    folder: PathBuf,
    spans: Vec<(f64, Option<f64>)>,
    stopped: bool,
}

impl Live {
    fn new(folder: &Path) -> Self {
        Live { folder: folder.to_path_buf(), spans: Vec::new(), stopped: false }
    }

    fn open(&mut self) -> Option<&mut (f64, Option<f64>)> {
        self.spans.last_mut().filter(|s| s.1.is_none())
    }

    fn pause(&mut self, t: f64) {
        if self.stopped || self.open().is_some() {
            return;
        }
        self.spans.push((t, None));
        save(&self.folder, &self.spans);
    }

    fn resume(&mut self, t: f64) {
        if self.stopped {
            return;
        }
        if let Some(open) = self.open() {
            open.1 = Some(t.max(open.0));
            save(&self.folder, &self.spans);
        }
    }

    /// A pause that still lasts ends at the stop
    fn stop(&mut self, t: f64) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some(open) = self.open() {
            open.1 = Some(t.max(open.0));
            save(&self.folder, &self.spans);
        }
        if !self.spans.is_empty() {
            info!("The recording was paused {} time(s), {:.1} s in all", self.spans.len(), pauses_of(&self.spans).total() / 1000.0);
        }
    }
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn now_ms() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

fn pauses_of(spans: &[(f64, Option<f64>)]) -> Pauses {
    Pauses::new(spans.iter().map(|(a, b)| (*a, b.unwrap_or(f64::INFINITY))).collect())
}

fn save(folder: &Path, spans: &[(f64, Option<f64>)]) {
    let tmp = folder.join(".pauses.json.tmp");
    let done = serde_json::to_vec(&Kept { v: 1, pauses: spans.to_vec() })
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        .and_then(|text| fs::write(&tmp, text))
        .and_then(|_| fs::rename(&tmp, folder.join(FILE)));
    if let Err(e) = done {
        warn!("The recording's pauses could not be written: {e}");
    }
}

fn read(folder: &Path) -> Pauses {
    fs::read(folder.join(FILE)).ok().and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok()).map(|k| pauses_of(&k.pauses)).unwrap_or_default()
}

/// A recording starts in this folder
pub fn recording_started(folder: &Path) {
    *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Live::new(folder));
}

/// The recording that runs was paused just now
pub fn paused() {
    if let Some(live) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        live.pause(now_ms());
    }
}

/// The recording that runs goes on from now
pub fn resumed() {
    if let Some(live) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        live.resume(now_ms());
    }
}

/// The recording in this folder stops
pub fn recording_stopped(folder: &Path) {
    if let Some(live) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_mut().filter(|l| l.folder == folder) {
        live.stop(now_ms());
    }
}

/// The pauses of the recording in this folder: as they are now when it is
/// the one that runs, from its `pauses.json` otherwise (none when it has no
/// such file: it was never paused, or it was made before pauses were kept)
pub fn of(folder: &Path) -> Pauses {
    if let Some(live) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|l| l.folder == folder) {
        return pauses_of(&live.spans);
    }
    read(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wall_clock_without_its_pauses() {
        let p = Pauses::new(vec![(30_000.0, 40_000.0), (10_000.0, 15_000.0), (40_000.0, 42_000.0), (50_000.0, 50_000.0)]);
        // In order, joined where they touch, empty ones left out
        assert_eq!(p, Pauses(vec![(10_000.0, 15_000.0), (30_000.0, 42_000.0)]));
        assert_eq!(p.total(), 17_000.0);
        assert_eq!(p.on_recording(9_000.0), Some(9_000.0));
        assert_eq!(p.on_recording(12_000.0), None);
        assert_eq!(p.clamp(12_000.0), 10_000.0);
        assert_eq!(p.on_recording(15_000.0), Some(10_000.0));
        assert_eq!(p.on_recording(20_000.0), Some(15_000.0));
        assert_eq!(p.on_recording(41_999.0), None);
        assert_eq!(p.on_recording(60_000.0), Some(43_000.0));
        // A block of sound goes by its middle
        assert_eq!(p.span_on_recording(9_960.0, 85.0), None);
        assert_eq!(p.span_on_recording(9_950.0, 85.0), Some(9_950.0));
        assert_eq!(p.span_on_recording(14_950.0, 85.0), None);
        assert_eq!(p.span_on_recording(14_960.0, 85.0), Some(9_960.0));
        // Never paused: the wall clock as it is
        assert_eq!(Pauses::default().on_recording(5.0), Some(5.0));
        // A pause that never ended
        let open = Pauses::new(vec![(1_000.0, f64::INFINITY)]);
        assert_eq!((open.total(), open.on_recording(999.0), open.on_recording(1e15)), (0.0, Some(999.0), None));
    }

    #[test]
    fn pauses_are_kept_with_the_recording() {
        let folder = std::env::temp_dir().join(format!("protokol-pauses-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        let mut live = Live::new(&folder);
        // Never paused: no file
        assert!(read(&folder).is_empty() && !folder.join(FILE).exists());
        live.pause(5_000.0);
        // Paused twice in a row, resumed twice in a row: once each
        live.pause(6_000.0);
        assert!(read(&folder).within(1e12));
        live.resume(9_000.0);
        live.resume(9_500.0);
        live.pause(20_000.0);
        assert!(read(&folder).within(30_000.0));
        // Stopped while paused: the pause ends at the stop
        live.stop(26_000.0);
        let want = Pauses(vec![(5_000.0, 9_000.0), (20_000.0, 26_000.0)]);
        assert_eq!(read(&folder), want);
        // Nothing more is added once it has stopped
        live.pause(40_000.0);
        live.stop(50_000.0);
        assert_eq!(read(&folder), want);
        // Another recording runs: this one's pauses come from its folder
        assert_eq!(of(&folder), want);
        let _ = fs::remove_dir_all(&folder);
    }
}
