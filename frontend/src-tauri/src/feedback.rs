//! «Повідомити про проблему»: the technical facts the feedback form offers to attach.
//! Nothing is sent from here: the form shows the text, and the person opens a
//! prefilled GitHub issue themselves.

use serde::Serialize;
use std::process::Command;

#[derive(Serialize, Default)]
pub struct FeedbackInfo {
    pub macos: String,
    pub mac_model: String,
    pub memory: String,
    pub free_disk: String,
    /// Error lines from the log of the last day, scrubbed (see `scrub`)
    pub log_errors: Vec<String>,
}

const MAX_LOG_LINES: usize = 20;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// «Mac mini (Apple M2)» and «16 GB» from system_profiler
fn hardware() -> (String, String) {
    let Some(json) = run("system_profiler", &["-json", "SPHardwareDataType"]) else {
        return (run("sysctl", &["-n", "hw.model"]).unwrap_or_default(), String::new());
    };
    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
    let hw = &v["SPHardwareDataType"][0];
    let name = hw["machine_name"].as_str().unwrap_or("Mac");
    let chip = hw["chip_type"].as_str().or_else(|| hw["cpu_type"].as_str()).unwrap_or("");
    let model = if chip.is_empty() { name.to_string() } else { format!("{name} ({chip})") };
    let memory = hw["physical_memory"].as_str().unwrap_or("").replace("GB", "ГБ");
    (model, memory)
}

/// Free space on the disk that holds the home folder, «112 ГБ»
fn free_disk() -> String {
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
    run("df", &["-k", &home])
        .and_then(|out| {
            let line = out.lines().nth(1)?.to_string();
            let kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
            Some(format!("{} ГБ", kb / 1024 / 1024))
        })
        .unwrap_or_default()
}

/// Takes out of a log line what could name a person or a meeting: the home folder,
/// quoted values, file paths and any Cyrillic text (meeting titles and names are Ukrainian)
pub(crate) fn scrub(line: &str, home: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static QUOTED: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    static CYRILLIC: OnceLock<Regex> = OnceLock::new();
    let mut s = if home.len() > 1 { line.replace(home, "~") } else { line.to_string() };
    let quoted = QUOTED.get_or_init(|| Regex::new(r#"'[^']*'|"[^"]*"|«[^»]*»"#).unwrap());
    s = quoted.replace_all(&s, "[…]").into_owned();
    // Cyrillic first: the placeholders below are Cyrillic too
    let cyr = CYRILLIC.get_or_init(|| Regex::new(r"\p{Cyrillic}(?:[\p{Cyrillic}\p{N}\s.,'’ʼ-]*[\p{Cyrillic}\p{N}])?").unwrap());
    s = cyr.replace_all(&s, "[текст]").into_owned();
    let path = PATH.get_or_init(|| Regex::new(r"~?(/[^/\s:,;)]+){2,}").unwrap());
    s = path
        .replace_all(&s, |c: &regex::Captures| {
            let p = &c[0];
            match p.rsplit_once('.') {
                Some((_, ext)) if ext.len() <= 5 && !ext.contains('/') => format!("[файл].{ext}"),
                _ => "[шлях]".to_string(),
            }
        })
        .into_owned();
    s.chars().take(300).collect::<String>().trim_end().to_string()
}

/// ERROR lines of the last 24 hours, newest last
fn log_errors() -> Vec<String> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let dir = home.join("Library/Logs").join(crate::identity::IDENTIFIER);
    let since = (chrono::Utc::now() - chrono::Duration::hours(24)).format("%Y-%m-%dT%H:%M:%S").to_string();
    let home_s = home.to_string_lossy().to_string();
    let mut lines: Vec<String> = vec![];
    for name in ["protokol.1.log", "protokol.log"] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else { continue };
        for line in text.lines() {
            if !line.contains(" ERROR ") || crate::logfile::private(line) {
                continue;
            }
            // «[2026-11-11T09:41:07Z ERROR target] message»
            let stamp = line.trim_start_matches('[').get(..19).unwrap_or("");
            if stamp < since.as_str() {
                continue;
            }
            lines.push(scrub(line, &home_s));
        }
    }
    let skip = lines.len().saturating_sub(MAX_LOG_LINES);
    lines.into_iter().skip(skip).collect()
}

#[tauri::command]
pub async fn api_feedback_info() -> FeedbackInfo {
    tokio::task::spawn_blocking(|| {
        let (mac_model, memory) = hardware();
        FeedbackInfo {
            macos: run("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}")).unwrap_or_default(),
            mac_model,
            memory,
            free_disk: free_disk(),
            log_errors: log_errors(),
        }
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::scrub;

    #[test]
    fn names_titles_and_paths_are_taken_out() {
        let home = "/Users/someone";
        let s = scrub(
            "[2026-11-11T09:41:07Z ERROR app_lib::api::api] save failed for meeting: Нарада 01.02.2026 10:00 at /Users/someone/Movies/rec/Нарада/audio.mp4",
            home,
        );
        assert!(!s.contains("Нарада"), "{s}");
        assert!(!s.contains("someone"), "{s}");
        assert!(s.contains("[файл].mp4"), "{s}");
        assert!(s.contains("ERROR app_lib::api::api"), "{s}");
        let q = scrub("[2026-11-11T09:41:07Z ERROR x] speaker 'Ivan P.' not found", home);
        assert!(!q.contains("Ivan"), "{q}");
    }
}
