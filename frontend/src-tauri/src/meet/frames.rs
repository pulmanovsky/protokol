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
    /// Meet's own captions, when they were on (the last version of each)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captions: Vec<super::captions::Caption>,
    /// The captions' language as Meet showed it last («Ukrainian (Ukraine)»)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub captions_lang: String,
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

    /// Adds one report's frames; false when it has none to add
    fn add_report(&mut self, r: &Value) -> bool {
        // Captions are text, not who is lit: kept from a hidden page too
        super::captions::add_report(&mut self.captions, r);
        if let Some(lang) = r.get("ccLang").and_then(|v| v.as_str()).filter(|l| !l.trim().is_empty()) {
            self.captions_lang = lang.trim().to_string();
        }
        // While the page is hidden Meet can stop updating the tiles and keep
        // the last speaker's lit: such frames say nothing of who speaks
        if frozen(r) {
            return false;
        }
        let Some(lit) = r.get("lit").and_then(|v| v.as_array()) else { return false };
        let n = lit.len();
        if n == 0 {
            return false;
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
        true
    }

    /// Only the frames within [from, to] (ms), names renumbered to those used
    fn span(mut self, from: f64, to: f64) -> MeetCall {
        let mut order: Vec<usize> = (0..self.t.len()).filter(|&i| self.t[i] >= from && self.t[i] <= to).collect();
        order.sort_by(|&a, &b| self.t[a].total_cmp(&self.t[b]));
        let mut out = MeetCall { v: 1, ..Default::default() };
        out.captions = std::mem::take(&mut self.captions).into_iter().filter(|c| c.t1 >= from && c.t0 <= to).collect();
        out.captions.sort_by(|a, b| a.t0.total_cmp(&b.t0));
        out.captions_lang = std::mem::take(&mut self.captions_lang);
        let names = std::mem::take(&mut self.names);
        for i in order {
            out.t.push(self.t[i]);
            out.level.push(self.level[i]);
            let ids = self.lit[i].iter().map(|&k| out.name_id(&names[k])).collect();
            out.lit.push(ids);
        }
        out
    }

    /// The call on the recording's clock: the wall clock with the recording's
    /// pauses taken out. Frames of a pause are left out and the later ones
    /// follow the earlier at once, as the recording's sound does; a caption
    /// seen only during a pause is left out too.
    fn without(mut self, pauses: &crate::audio::pauses::Pauses) -> MeetCall {
        if pauses.is_empty() {
            return self;
        }
        let kept: Vec<(usize, f64)> = self.t.iter().enumerate().filter_map(|(i, t)| pauses.on_recording(*t).map(|t| (i, t))).collect();
        self.level = kept.iter().map(|(i, _)| self.level[*i]).collect();
        self.lit = kept.iter().map(|(i, _)| std::mem::take(&mut self.lit[*i])).collect();
        self.t = kept.into_iter().map(|(_, t)| t).collect();
        self.captions.retain(|c| !(pauses.within(c.t0) && pauses.within(c.t1) && pauses.clamp(c.t0) == pauses.clamp(c.t1)));
        for c in &mut self.captions {
            c.t0 = pauses.clamp(c.t0);
            c.t1 = pauses.clamp(c.t1);
            for g in &mut c.grew {
                g.1 = pauses.clamp(g.1);
            }
        }
        self
    }
}

/// A report sent while the Meet page was hidden (capture.js `visible`) whose
/// lit tiles cannot be trusted. Since report version 9 capture.js keeps Meet
/// running in a minimised window, and a live page changes classes whenever
/// someone is lit (a few changes per report, none in silence); a page that
/// stood still with a tile lit is frozen. Older reports from a hidden page
/// came from a page that froze.
fn frozen(r: &Value) -> bool {
    if r.get("visible").and_then(|v| v.as_str()) != Some("hidden") {
        return false;
    }
    match r.get("changes").and_then(|v| v.as_u64()) {
        None => true,
        Some(0) => r.get("lit").and_then(|v| v.as_array()).is_some_and(|frames| frames.iter().any(|f| f.as_array().is_some_and(|a| !a.is_empty()))),
        Some(_) => false,
    }
}

/// Reports in the span read, and of them those from a frozen page
fn read_reports(path: &Path, from: f64, to: f64, call: &mut MeetCall) -> (usize, usize) {
    let Ok(file) = fs::File::open(path) else { return (0, 0) };
    let (mut used, mut hidden_ones) = (0, 0);
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(r) = serde_json::from_str::<Value>(&line) else { continue };
        let t = r.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0);
        // A report holds about 2 s of frames before its time
        if t < from || t - 10_000.0 > to {
            continue;
        }
        if !call.add_report(&r) && frozen(&r) {
            hidden_ones += 1;
        }
        used += 1;
    }
    (used, hidden_ones)
}

/// The call's frames between `from` and `to` (ms since 1970) from the
/// reports in `dirs`. Files last written before `from` are not opened.
pub fn load_span(dirs: &[PathBuf], from: f64, to: f64) -> Option<MeetCall> {
    let mut call = MeetCall::default();
    let (mut files, mut reports, mut hidden_ones) = (0, 0, 0);
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
            let (used, hidden_here) = read_reports(&path, from, to, &mut call);
            if used > 0 {
                files += 1;
                reports += used;
                hidden_ones += hidden_here;
            }
        }
    }
    if hidden_ones > 0 {
        warn!("MEET-HIDDEN {hidden_ones} of {reports} Meet report(s) came while the Meet page was hidden and stood still; left out");
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
/// time it was recorded (then copied into its folder). It is on the
/// recording's clock: where the recording was paused, the call's frames go
/// on from that moment as the recording's sound does (`audio::pauses`).
pub fn for_recording(folder: &Path, duration_s: f64) -> Option<MeetCall> {
    if let Some(call) = load_folder(folder) {
        return Some(call);
    }
    let start = recording_start_ms(folder)?;
    let pauses = crate::audio::pauses::of(folder);
    // Two minutes on both sides: the clocks are lined up by the sound later
    let call = load_span(&sessions_dirs(), start - 120_000.0, start + duration_s * 1000.0 + pauses.total() + 120_000.0)?.without(&pauses);
    if call.is_empty() {
        return None;
    }
    if !pauses.is_empty() {
        info!("Meet: the recording was paused {} time(s), {:.1} s in all; the call's frames go on from where it was paused", pauses.count(), pauses.total() / 1000.0);
    }
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
    fn a_paused_recording_has_the_call_without_its_pauses() {
        // Frames every 100 ms for 10 s; the recording was paused from 3 s to 7 s
        let mut call = MeetCall { v: 2, names: vec!["Анна".to_string(), "Борис".to_string()], ..Default::default() };
        for f in 0..100 {
            call.t.push(1_000_000.0 + f as f64 * 100.0);
            call.level.push(f as u32);
            call.lit.push(vec![if f < 50 { 0 } else { 1 }]);
        }
        let caption = |key: &str, t0: f64, t1: f64| crate::meet::captions::Caption { key: key.to_string(), who: "Анна".to_string(), text: "так".to_string(), t0, t1, grew: vec![(1, t1)] };
        call.captions = vec![caption("a", 1_001_000.0, 1_002_000.0), caption("b", 1_004_000.0, 1_005_000.0), caption("c", 1_006_500.0, 1_008_000.0)];
        let pauses = crate::audio::pauses::Pauses::new(vec![(1_003_000.0, 1_007_000.0)]);
        let got = call.clone().without(&pauses);
        // 3 s before the pause and 3 s after it, one after the other
        assert_eq!(got.t.len(), 60);
        assert_eq!((got.t[29], got.t[30], got.t[59]), (1_002_900.0, 1_003_000.0, 1_005_900.0));
        assert_eq!((got.level[29], got.level[30]), (29, 70));
        assert_eq!((got.lit[29].clone(), got.lit[30].clone()), (vec![0], vec![1]));
        // The caption of the pause is gone; the one that went on after it starts where the recording did
        assert_eq!(got.captions.iter().map(|c| (c.key.as_str(), c.t0, c.t1)).collect::<Vec<_>>(), vec![("a", 1_001_000.0, 1_002_000.0), ("c", 1_003_000.0, 1_004_000.0)]);
        assert_eq!(got.captions[1].grew, vec![(1, 1_004_000.0)]);
        // Never paused: as it was
        assert_eq!(call.clone().without(&Default::default()), call);

        // From the reports to the recording's folder
        let dir = std::env::temp_dir().join(format!("meet-frames-paused-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let start = chrono::DateTime::from_timestamp(1_700_000_010, 0).unwrap();
        fs::write(dir.join("metadata.json"), serde_json::json!({ "created_at": start.to_rfc3339() }).to_string()).unwrap();
        let t0 = start.timestamp_millis() as f64;
        fs::write(dir.join(crate::audio::pauses::FILE), serde_json::json!({ "v": 1, "pauses": [[t0 + 3_000.0, t0 + 7_000.0]] }).to_string()).unwrap();
        let mut call = MeetCall { v: 2, names: vec!["Анна".to_string()], ..Default::default() };
        for f in 0..100 {
            call.t.push(t0 + f as f64 * 100.0);
            call.level.push(0);
            call.lit.push(vec![0]);
        }
        let got = call.without(&crate::audio::pauses::of(&dir));
        assert_eq!((got.t.len(), got.t[30]), (60, t0 + 3_000.0));
        let _ = fs::remove_dir_all(&dir);
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

    #[test]
    fn frames_from_a_frozen_hidden_page_are_left_out() {
        let mut call = MeetCall::default();
        // An older page froze while hidden, keeping the last speaker lit
        let mut hidden = report(10_000.0, &["Анна"], vec![vec![0], vec![0]], vec![vec![5, 6]], Some(vec![9_900.0, 10_000.0]));
        hidden["visible"] = serde_json::json!("hidden");
        assert!(!call.add_report(&hidden));
        // A page that stood still with a tile lit
        hidden["changes"] = serde_json::json!(0);
        assert!(!call.add_report(&hidden));
        assert!(call.is_empty());
        // Meet kept changing the page: its tiles are live, however few the changes
        let mut live = report(11_000.0, &["Анна"], vec![vec![0]], vec![vec![5]], Some(vec![11_000.0]));
        live["visible"] = serde_json::json!("hidden");
        live["changes"] = serde_json::json!(3);
        assert!(call.add_report(&live));
        assert_eq!(call.t, vec![11_000.0]);
        // Silence: nothing lit, nothing changed
        let mut quiet = report(11_100.0, &["Анна"], vec![vec![]], vec![vec![0]], Some(vec![11_100.0]));
        quiet["visible"] = serde_json::json!("hidden");
        quiet["changes"] = serde_json::json!(0);
        assert!(call.add_report(&quiet));
        assert_eq!(call.t, vec![11_000.0, 11_100.0]);
        call = MeetCall::default();
        let mut shown = report(12_000.0, &["Анна", "Борис"], vec![vec![1]], vec![vec![5]], Some(vec![12_000.0]));
        shown["visible"] = serde_json::json!("visible");
        assert!(call.add_report(&shown));
        assert_eq!(call.t, vec![12_000.0]);
        assert_eq!(call.names, vec!["Борис".to_string()]);
        assert_eq!(call.lit, vec![vec![0]]);

        // Read from a file: the hidden report's frames are not in the span
        let dir = std::env::temp_dir().join(format!("meet-hidden-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut a = report(5_000_000.0, &["Анна"], vec![vec![0]], vec![vec![10]], Some(vec![5_000_000.0]));
        a["visible"] = serde_json::json!("hidden");
        a["changes"] = serde_json::json!(0);
        let b = report(5_002_000.0, &["Борис"], vec![vec![0]], vec![vec![10]], Some(vec![5_002_000.0]));
        fs::write(dir.join("a.jsonl"), format!("{a}\n{b}\n")).unwrap();
        let span = load_span(&[dir.clone()], 4_000_000.0, 6_000_000.0).unwrap();
        assert_eq!(span.t, vec![5_002_000.0]);
        assert_eq!(span.names, vec!["Борис".to_string()]);
        // Only hidden reports: nothing to go on
        fs::write(dir.join("a.jsonl"), format!("{a}\n")).unwrap();
        assert!(load_span(&[dir.clone()], 4_000_000.0, 6_000_000.0).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
