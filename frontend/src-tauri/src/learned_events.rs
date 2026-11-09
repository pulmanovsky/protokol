//! What the app learned, as it happens: a «learned» event for the window
//! (kind word | voice | document, from, to, count) and a small local log
//! (learned_log.jsonl in the app's data folder, kept under 1 MB) for
//! «Пам'ять»: rules learned since a time and fixes applied by itself
//! per meeting. Nothing leaves the computer.

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, Wry};

static APP: OnceCell<AppHandle<Wry>> = OnceCell::new();
static LOG: OnceCell<PathBuf> = OnceCell::new();
const LOG_MAX: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Learned {
    pub at: String,
    /// word (a correction rule), voice (a voice profile), document (examples
    /// from an approved document), applied (rules applied by the app itself)
    pub kind: String,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    pub count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting: Option<String>,
}

pub fn init(app: &AppHandle<Wry>) {
    let _ = APP.set(app.clone());
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = LOG.set(dir.join("learned_log.jsonl"));
    }
}

/// Something was learned (or applied): to the window and the log
pub fn note(kind: &str, from: &str, to: &str, count: usize, meeting: Option<&str>) {
    if count == 0 {
        return;
    }
    let item = Learned { at: chrono::Utc::now().to_rfc3339(), kind: kind.into(), from: from.into(), to: to.into(), count, meeting: meeting.map(String::from) };
    if let Some(path) = LOG.get() {
        append(path, &item);
    }
    if let Some(app) = APP.get() {
        let _ = app.emit("learned", &item);
    }
}

fn append(path: &std::path::Path, item: &Learned) {
    if std::fs::metadata(path).map(|m| m.len() > LOG_MAX).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
    }
    if let (Ok(mut f), Ok(line)) = (std::fs::OpenOptions::new().create(true).append(true).open(path), serde_json::to_string(item)) {
        let _ = writeln!(f, "{line}");
    }
}

/// Words a learned rule fixed in a meeting by itself (auto_fixes.json next
/// to the log: meeting → rules with how many times each was applied)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoFix {
    pub from: String,
    pub to: String,
    pub count: usize,
    /// When the rule was learned (from the log), for «У пам'яті з DD.MM.»
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_at: Option<String>,
}

type AutoFixes = std::collections::BTreeMap<String, Vec<AutoFix>>;

fn fixes_path() -> Option<PathBuf> {
    LOG.get().map(|p| p.with_file_name("auto_fixes.json"))
}

fn merge_fixes(all: &mut AutoFixes, meeting: &str, hits: &[(String, String)]) {
    let list = all.entry(meeting.to_string()).or_default();
    for (from, to) in hits {
        match list.iter_mut().find(|f| f.from.to_lowercase() == from.to_lowercase() && f.to == *to) {
            Some(f) => f.count += 1,
            None => list.push(AutoFix { from: from.clone(), to: to.clone(), count: 1, learned_at: None }),
        }
    }
}

/// Rules applied by the app itself in `meeting` (recognition, or rules applied to its text)
pub fn record_auto_fixes(meeting: &str, hits: &[(String, String)]) {
    if hits.is_empty() {
        return;
    }
    if let Some(path) = fixes_path() {
        let mut all: AutoFixes = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        merge_fixes(&mut all, meeting, hits);
        if let Ok(json) = serde_json::to_vec_pretty(&all) {
            let _ = std::fs::write(&path, json);
        }
    }
    note("applied", "", "", hits.len(), Some(meeting));
}

/// When each rule was first learned, from the log («word» entries)
fn learned_dates(log: &str) -> std::collections::HashMap<(String, String), String> {
    let mut out = std::collections::HashMap::new();
    for item in log.lines().filter_map(|l| serde_json::from_str::<Learned>(l).ok()).filter(|i| i.kind == "word") {
        out.entry((item.from.to_lowercase(), item.to.clone())).or_insert(item.at);
    }
    out
}

#[tauri::command]
pub async fn api_auto_fixes(meeting_id: String) -> Result<Vec<AutoFix>, String> {
    let all: AutoFixes = fixes_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let dates = learned_dates(&LOG.get().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default());
    Ok(all
        .get(&meeting_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut f| {
            f.learned_at = dates.get(&(f.from.to_lowercase(), f.to.clone())).cloned();
            f
        })
        .collect())
}

/// Each rule in «Пам'ять»: when it was learned, from which meeting, how many
/// times it was applied since
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuleUse {
    pub from: String,
    pub to: String,
    pub learned_at: Option<String>,
    pub meeting: Option<String>,
    pub applied: usize,
}

#[tauri::command]
pub async fn api_learned_rules() -> Result<Vec<RuleUse>, String> {
    let log = LOG.get().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let all: AutoFixes = fixes_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    Ok(rule_uses(&log, &all))
}

fn rule_uses(log: &str, all: &AutoFixes) -> Vec<RuleUse> {
    let mut out: Vec<RuleUse> = Vec::new();
    for item in log.lines().filter_map(|l| serde_json::from_str::<Learned>(l).ok()).filter(|i| i.kind == "word") {
        if !out.iter().any(|r| r.from.to_lowercase() == item.from.to_lowercase() && r.to == item.to) {
            out.push(RuleUse { from: item.from, to: item.to, learned_at: Some(item.at), meeting: item.meeting, applied: 0 });
        }
    }
    for f in all.values().flatten() {
        if let Some(r) = out.iter_mut().find(|r| r.from.to_lowercase() == f.from.to_lowercase() && r.to == f.to) {
            r.applied += f.count;
        }
    }
    out.reverse(); // newest first
    out
}

#[derive(Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// New correction rules, voices and document lessons since the time asked
    pub words: usize,
    pub voices: usize,
    pub documents: usize,
    /// Fixes the app made by itself, per meeting
    pub applied: Vec<(String, usize)>,
}

/// What the log says since `since` (RFC 3339; everything when absent)
pub fn summary_of(lines: &str, since: Option<&str>) -> Summary {
    let since = since.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let mut out = Summary::default();
    for item in lines.lines().filter_map(|l| serde_json::from_str::<Learned>(l).ok()) {
        if let (Some(s), Ok(at)) = (since, chrono::DateTime::parse_from_rfc3339(&item.at)) {
            if at < s {
                continue;
            }
        }
        match item.kind.as_str() {
            "word" => out.words += item.count,
            "voice" => out.voices += item.count,
            "document" => out.documents += item.count,
            "applied" => {
                let m = item.meeting.unwrap_or_default();
                match out.applied.iter_mut().find(|(id, _)| *id == m) {
                    Some((_, n)) => *n += item.count,
                    None => out.applied.push((m, item.count)),
                }
            }
            _ => {}
        }
    }
    out
}

#[tauri::command]
pub async fn api_learned_summary(since: Option<String>) -> Result<Summary, String> {
    let text = LOG.get().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    Ok(summary_of(&text, since.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_since_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learned_log.jsonl");
        let at = |s: &str| format!("2026-11-08T{s}:00+00:00");
        for (t, kind, count, meeting) in [("06:00", "word", 1, None), ("08:00", "word", 2, None), ("08:05", "voice", 1, Some("m1")), ("08:10", "applied", 3, Some("m1")), ("08:20", "applied", 2, Some("m1")), ("08:30", "applied", 1, Some("m2")), ("08:40", "document", 5, Some("m1"))] {
            append(&path, &Learned { at: at(t), kind: kind.into(), from: String::new(), to: String::new(), count, meeting: meeting.map(String::from) });
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let s = summary_of(&text, Some(&at("07:00")));
        assert_eq!(s, Summary { words: 2, voices: 1, documents: 5, applied: vec![("m1".into(), 5), ("m2".into(), 1)] });
        assert_eq!(summary_of(&text, None).words, 3);
        // A broken line is skipped
        assert_eq!(summary_of(&format!("{text}not json\n"), None).words, 3);
    }

    #[test]
    fn rules_with_their_uses() {
        let log = [
            Learned { at: "2026-11-07T08:00:00+00:00".into(), kind: "word".into(), from: "репеті".into(), to: "ЦНАП".into(), count: 1, meeting: Some("m0".into()) },
            Learned { at: "2026-11-08T08:00:00+00:00".into(), kind: "word".into(), from: "прат".into(), to: "ПрАТ".into(), count: 1, meeting: None },
        ]
        .iter()
        .map(|l| serde_json::to_string(l).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
        let mut all = AutoFixes::new();
        merge_fixes(&mut all, "m1", &[("Репеті".into(), "ЦНАП".into()), ("репеті".into(), "ЦНАП".into())]);
        merge_fixes(&mut all, "m2", &[("репеті".into(), "ЦНАП".into())]);
        assert_eq!(all["m1"].len(), 1, "the same rule, counted");
        assert_eq!(all["m1"][0].count, 2);
        let uses = rule_uses(&log, &all);
        assert_eq!(uses.len(), 2);
        assert_eq!((uses[0].from.as_str(), uses[0].applied), ("прат", 0), "newest first");
        assert_eq!((uses[1].to.as_str(), uses[1].applied, uses[1].meeting.as_deref()), ("ЦНАП", 3, Some("m0")));
        assert_eq!(learned_dates(&log).get(&("репеті".to_string(), "ЦНАП".to_string())).map(String::as_str), Some("2026-11-07T08:00:00+00:00"));
    }
}
