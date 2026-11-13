//! The Meet reports read back: who was lit as speaking and how loud the call
//! was, frame by frame (100 ms), for one recording's span of time.
//!
//! A report (one line of `<app data>/meet/<window opened>.jsonl`) holds the
//! frames since the last report: per stream the levels, per frame the lit
//! tiles (indices into the report's `names`) and, since version 2, the wall
//! clock of each frame. Once a recording has used them, its frames are kept
//! in its own folder (`meet.json`), so recognising it again later does not
//! depend on the reports still being there.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use log::{info, warn};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The recording folder's copy of its call
pub const FOLDER_FILE: &str = "meet.json";

static SESSIONS_DIRS: OnceCell<Vec<PathBuf>> = OnceCell::new();

/// Where the reports are (set once when the app starts)
pub fn set_sessions_dirs(dirs: Vec<PathBuf>) {
    let _ = SESSIONS_DIRS.set(dirs);
}

pub fn sessions_dirs() -> Vec<PathBuf> {
    SESSIONS_DIRS.get().cloned().unwrap_or_default()
}

/// A call's frames on the wall clock
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MeetCall {
    pub v: u32,
    /// Participant names as shown on their tiles
    pub names: Vec<String>,
    /// Wall clock of each frame (ms since 1970)
    pub t: Vec<f64>,
    /// The loudest participant stream in each frame (RMS × 10 000)
    pub level: Vec<u32>,
    /// Participants lit as speaking in each frame (indices into `names`)
    pub lit: Vec<Vec<usize>>,
}

impl MeetCall {
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    fn name_id(&mut self, name: &str) -> usize {
        match self.names.iter().position(|n| n == name) {
            Some(i) => i,
            None => {
                self.names.push(name.to_string());
                self.names.len() - 1
            }
        }
    }

    /// Adds one report's frames
    fn add_report(&mut self, r: &Value) {
        let Some(lit) = r.get("lit").and_then(|v| v.as_array()) else { return };
        let n = lit.len();
        if n == 0 {
            return;
        }
        let step = r.get("stepMs").and_then(|v| v.as_f64()).unwrap_or(100.0);
        let report_t = r.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let ft: Vec<f64> = r.get("ft").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
        // Version 1 had no frame times: the last frame was just before the report
        let times: Vec<f64> = if ft.len() == n { ft } else { (0..n).map(|j| report_t - (n - 1 - j) as f64 * step).collect() };
        let local: Vec<String> =
            r.get("names").and_then(|v| v.as_array()).map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default();
        let mut level = vec![0u32; n];
        if let Some(streams) = r.get("streams").and_then(|v| v.as_array()) {
            for s in streams {
                let Some(lv) = s.get("levels").and_then(|v| v.as_array()) else { continue };
                // A stream that joined during the report has fewer, the last frames'
                let skip = n.saturating_sub(lv.len());
                for (j, x) in lv.iter().enumerate() {
                    let v = x.as_f64().unwrap_or(0.0).max(0.0) as u32;
                    if let Some(slot) = level.get_mut(skip + j) {
                        *slot = (*slot).max(v);
                    }
                }
            }
        }
        for j in 0..n {
            let ids: Vec<usize> = lit[j]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_u64()).filter_map(|i| local.get(i as usize).cloned()).filter(|n| !n.is_empty()).collect::<Vec<_>>())
                .unwrap_or_default()
                .iter()
                .map(|name| self.name_id(name))
                .collect();
            self.t.push(times[j]);
            self.level.push(level[j]);
            self.lit.push(ids);
        }
    }

    /// Only the frames within [from, to] (ms), names renumbered to those used
    fn span(mut self, from: f64, to: f64) -> MeetCall {
        let mut order: Vec<usize> = (0..self.t.len()).filter(|&i| self.t[i] >= from && self.t[i] <= to).collect();
        order.sort_by(|&a, &b| self.t[a].total_cmp(&self.t[b]));
        let mut out = MeetCall { v: 1, ..Default::default() };
        let names = std::mem::take(&mut self.names);
        for i in order {
            out.t.push(self.t[i]);
            out.level.push(self.level[i]);
            let ids = self.lit[i].iter().map(|&k| out.name_id(&names[k])).collect();
            out.lit.push(ids);
        }
        out
    }
}

fn read_reports(path: &Path, from: f64, to: f64, call: &mut MeetCall) -> usize {
    let Ok(file) = fs::File::open(path) else { return 0 };
    let mut used = 0;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(r) = serde_json::from_str::<Value>(&line) else { continue };
        let t = r.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0);
        // A report holds about 2 s of frames before its time
        if t < from || t - 10_000.0 > to {
            continue;
        }
        call.add_report(&r);
        used += 1;
    }
    used
}

/// The call's frames between `from` and `to` (ms since 1970) from the
/// reports in `dirs`. Files last written before `from` are not opened.
pub fn load_span(dirs: &[PathBuf], from: f64, to: f64) -> Option<MeetCall> {
    let mut call = MeetCall::default();
    let mut files = 0;
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as f64)
                .unwrap_or(f64::MAX);
            if modified < from {
                continue;
            }
            if read_reports(&path, from, to, &mut call) > 0 {
                files += 1;
            }
        }
    }
    let call = call.span(from, to);
    if call.is_empty() {
        return None;
    }
    info!("Meet: {} frame(s) from {files} report file(s) for this recording", call.t.len());
    Some(call)
}

pub fn load_folder(folder: &Path) -> Option<MeetCall> {
    let text = fs::read_to_string(folder.join(FOLDER_FILE)).ok()?;
    serde_json::from_str(&text).ok().filter(|c: &MeetCall| !c.is_empty())
}

pub fn save_folder(folder: &Path, call: &MeetCall) {
    match serde_json::to_string(call) {
        Ok(text) => {
            if let Err(e) = fs::write(folder.join(FOLDER_FILE), text) {
                warn!("Meet: could not keep the call with the recording: {e}");
            }
        }
        Err(e) => warn!("Meet: {e}"),
    }
}

/// When a recording started (ms since 1970), from its `metadata.json`
pub fn recording_start_ms(folder: &Path) -> Option<f64> {
    let text = fs::read_to_string(folder.join("metadata.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let created = v.get("created_at")?.as_str()?;
    let t = chrono::DateTime::parse_from_rfc3339(created).ok()?;
    Some(t.timestamp_millis() as f64)
}

/// The call that went with a recording: its own copy, or the reports of the
/// time it was recorded (then copied into its folder)
pub fn for_recording(folder: &Path, duration_s: f64) -> Option<MeetCall> {
    if let Some(call) = load_folder(folder) {
        return Some(call);
    }
    let start = recording_start_ms(folder)?;
    // Two minutes on both sides: the clocks are lined up by the sound later
    let call = load_span(&sessions_dirs(), start - 120_000.0, start + duration_s * 1000.0 + 120_000.0)?;
    save_folder(folder, &call);
    Some(call)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(t: f64, names: &[&str], lit: Vec<Vec<usize>>, levels: Vec<Vec<i64>>, ft: Option<Vec<f64>>) -> Value {
        let mut r = serde_json::json!({
            "v": 2, "t": t, "stepMs": 100, "names": names, "lit": lit,
            "streams": levels.into_iter().enumerate().map(|(i, l)| serde_json::json!({"i": i, "levels": l})).collect::<Vec<_>>(),
        });
        if let Some(ft) = ft {
            r["ft"] = serde_json::json!(ft);
        }
        r
    }

    #[test]
    fn reports_become_frames() {
        let mut call = MeetCall::default();
        // Version 1: times counted back from the report; a stream that joined late
        call.add_report(&report(10_000.0, &["Анна", "Борис"], vec![vec![], vec![0], vec![0, 1]], vec![vec![0, 50, 60], vec![-1, 70]], None));
        assert_eq!(call.t, vec![9_800.0, 9_900.0, 10_000.0]);
        assert_eq!(call.level, vec![0, 50, 70]);
        assert_eq!(call.lit, vec![vec![], vec![0], vec![0, 1]]);
        // Version 2: the frames' own times; names by text, not by the report's indices
        call.add_report(&report(12_000.0, &["Борис", "Анна"], vec![vec![0], vec![1]], vec![vec![5, 6]], Some(vec![11_900.0, 12_000.0])));
        assert_eq!(call.t[3..], [11_900.0, 12_000.0]);
        assert_eq!(call.lit[3..], [vec![1], vec![0]]);
        let span = call.clone().span(9_850.0, 11_950.0);
        assert_eq!(span.t, vec![9_900.0, 10_000.0, 11_900.0]);
        assert_eq!(span.names, vec!["Анна".to_string(), "Борис".to_string()]);
        assert_eq!(span.lit, vec![vec![0], vec![0, 1], vec![1]]);
    }

    #[test]
    fn spans_from_files_and_folder_copy() {
        let dir = std::env::temp_dir().join(format!("meet-frames-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let line = report(5_000_000.0, &["Анна"], vec![vec![0], vec![]], vec![vec![10, 0]], Some(vec![4_999_900.0, 5_000_000.0]));
        fs::write(dir.join("a.jsonl"), format!("{line}\nnot json\n")).unwrap();
        let call = load_span(&[dir.clone()], 4_000_000.0, 6_000_000.0).unwrap();
        assert_eq!(call.t.len(), 2);
        assert!(load_span(&[dir.clone()], 7_000_000.0, 8_000_000.0).is_none());
        save_folder(&dir, &call);
        assert_eq!(load_folder(&dir).unwrap(), call);
        fs::write(dir.join("metadata.json"), r#"{"created_at":"2026-11-12T22:36:55.117Z"}"#).unwrap();
        assert_eq!(recording_start_ms(&dir), Some(1_794_523_015_117.0));
        let _ = fs::remove_dir_all(&dir);
    }
}
