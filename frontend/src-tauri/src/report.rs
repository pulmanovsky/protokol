//! «Повідомити про проблему»: one zip on the Desktop with what is needed to
//! understand a problem, put together on this Mac. Nothing is sent: the
//! person looks through every file first, saves the report and passes it on
//! themselves (Signal, mail, a USB stick) to whoever looks after Protokol.
//!
//! In the zip (`Звіт Протоколу ЗВ-XXXX/`):
//! - `опис.txt`: the person's words, when, the marks they made;
//! - `система.txt`: version, macOS, Mac, memory, free space, models, the
//!   kinds of the sound devices;
//! - `журнал/`: the journal of the last 7 days (see `logfile`);
//! - `нарада/` (when a recording is chosen): its own journal, numbers from
//!   the database, the Meet reports of its time and the Meet frames it used,
//!   with participant names replaced by «Учасник 1…N»;
//! - `падіння/`: macOS's reports of Protokol's crashes of the last 7 days;
//! - `зміст.txt`: the list of files and what is never in a report.
//!
//! Never in a report: sound, transcript and document text, meeting titles,
//! names of people, the Mac's user name. Every journal line is cleaned again
//! on the way in (lines of 0.4.3 and earlier were written before some of the
//! cleaning existed).

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::logfile;

const CODE_CHARS: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const STAGING_PREFIX: &str = "protokol-report-";
/// Meet reports of one recording, at most
const MEET_LIMIT: u64 = 30 * 1024 * 1024;

fn new_code() -> String {
    use rand::Rng;
    let mut r = rand::thread_rng();
    (0..4).map(|_| CODE_CHARS[r.gen_range(0..CODE_CHARS.len())] as char).collect()
}

fn valid_code(code: &str) -> bool {
    code.len() == 4 && code.bytes().all(|b| CODE_CHARS.contains(&b))
}

fn staging(code: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{STAGING_PREFIX}{code}"))
}

/// «Звіт Протоколу ЗВ-7K3F»: the zip's name and its folder inside
pub fn report_name(code: &str) -> String {
    format!("Звіт Протоколу ЗВ-{code}")
}

#[derive(Serialize, Clone, Debug)]
pub struct ReportFile {
    /// Path inside the report, «журнал/protokol-2026-11-21.log»
    pub name: String,
    pub bytes: u64,
    /// What it is, for the form
    pub about: String,
}

#[derive(Serialize, Debug)]
pub struct PreparedReport {
    pub code: String,
    pub files: Vec<ReportFile>,
    pub marks: Vec<logfile::Mark>,
}

#[derive(Serialize, Debug)]
pub struct SavedReport {
    pub path: String,
    pub code: String,
    pub bytes: u64,
}

// ===== Cleaning =====

fn home() -> String {
    dirs::home_dir().map(|h| h.to_string_lossy().trim_end_matches('/').to_string()).unwrap_or_default()
}

/// A journal line as it goes into a report: speech lines left out, titles,
/// folders, device and user names taken out again
fn clean_line(line: &str, home: &str) -> Option<String> {
    if logfile::private(line) {
        return None;
    }
    let mut s = logfile::scrub(line);
    if home.len() > 1 && s.contains(home) {
        s = s.replace(home, "~");
    }
    Some(s)
}

/// Lines of one kind that go into a copy in one minute, at most
const SAME_PER_MINUTE: u32 = 5;

/// «2026-11-21T22:19» and the line's kind: the line without its time and
/// numbers. None for a line without a time (the rest of a multi-line entry).
fn kind_of(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let stamp = rest.get(..20)?;
    if !stamp.ends_with('Z') || stamp.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let kind: String = rest[20..].chars().filter(|c| !c.is_ascii_digit()).take(200).collect();
    Some((stamp[..16].to_string(), kind))
}

/// The same kind of line past [`SAME_PER_MINUTE`] in one minute stays out of a
/// copy and is counted. The journal of 0.4.3 and earlier had no such limit:
/// one warning could come 40 000 times a day.
#[derive(Default)]
struct Collapse {
    /// kind → (minute, lines of it this minute)
    seen: HashMap<String, (String, u32)>,
    /// kind → (left out, the first of them)
    left_out: HashMap<String, (u64, String)>,
}

impl Collapse {
    fn keep(&mut self, minute: &str, kind: &str, line: &str) -> bool {
        let e = self.seen.entry(kind.to_string()).or_insert_with(|| (String::new(), 0));
        if e.0 != minute {
            *e = (minute.to_string(), 0);
        }
        e.1 += 1;
        if e.1 <= SAME_PER_MINUTE {
            return true;
        }
        self.left_out.entry(kind.to_string()).or_insert_with(|| (0, line.trim_end().to_string())).0 += 1;
        false
    }

    /// The note at the end of a copy: how many lines stayed out, the most
    /// frequent first, each example cleaned as a line of the copy
    fn note(&self, home: &str) -> Option<String> {
        let total: u64 = self.left_out.values().map(|(n, _)| n).sum();
        if total == 0 {
            return None;
        }
        let mut top: Vec<&(u64, String)> = self.left_out.values().collect();
        top.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut s = format!("\n--- Однакові рядки понад {SAME_PER_MINUTE} за хвилину в копію не ввійшли: {total}. Найчастіші (перший з них):\n");
        for (n, sample) in top.into_iter().take(20) {
            let sample = clean_line(sample, home).unwrap_or_else(|| "(рядок зі словами з наради)".into());
            s.push_str(&format!("×{n} {}\n", sample.trim_end()));
        }
        Some(s)
    }
}

/// A text file copied line by line through [`clean_line`]. With `since`
/// («2026-11-15»), lines of earlier days stay out; lines of one kind past
/// [`SAME_PER_MINUTE`] a minute stay out too and are counted at the end.
fn copy_clean(src: &Path, dst: &Path, home: &str, since: Option<&str>) -> std::io::Result<u64> {
    let file = fs::File::open(src)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = std::io::BufWriter::new(fs::File::create(dst)?);
    let mut buf = Vec::new();
    let mut reader = BufReader::new(file);
    let mut collapse = Collapse::default();
    // The rest of a multi-line entry follows its first line
    let mut keeping = true;
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        if let Some((minute, kind)) = kind_of(&line) {
            keeping = since.map_or(true, |d| minute.as_str() >= d) && collapse.keep(&minute, &kind, &line);
        }
        if !keeping {
            continue;
        }
        if let Some(l) = clean_line(&line, home) {
            out.write_all(l.as_bytes())?;
        }
    }
    if let Some(note) = collapse.note(home) {
        out.write_all(note.as_bytes())?;
    }
    out.flush()?;
    Ok(fs::metadata(dst).map(|m| m.len()).unwrap_or(0))
}

/// Participant names in Meet reports → «Учасник N» (the secretary → «Я»), in
/// the order they first appear; the meeting's code → «…»
#[derive(Default)]
struct Names {
    map: HashMap<String, String>,
    order: Vec<String>,
}

impl Names {
    fn learn(&mut self, report: &Value) {
        let me = report.get("self").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(me) = me.as_ref() {
            self.map.entry(me.clone()).or_insert_with(|| "Я".to_string());
        }
        for n in report.get("names").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
            let n = n.trim();
            if n.is_empty() || self.map.contains_key(n) {
                continue;
            }
            self.order.push(n.to_string());
            let label = format!("Учасник {}", self.order.len());
            self.map.insert(n.to_string(), label);
        }
    }

    /// The report with names replaced, as one line
    fn hide(&self, report: &mut Value) -> String {
        let swap = |v: &mut Value| {
            if let Some(s) = v.as_str() {
                if let Some(l) = self.map.get(s.trim()) {
                    *v = Value::String(l.clone());
                }
            }
        };
        if let Some(v) = report.get_mut("self") {
            swap(v);
        }
        if let Some(arr) = report.get_mut("names").and_then(|v| v.as_array_mut()) {
            arr.iter_mut().for_each(swap);
        }
        if let Some(m) = report.get_mut("meeting") {
            if m.is_string() {
                *m = Value::String("…".into());
            }
        }
        if let Some(arr) = report.get_mut("errors").and_then(|v| v.as_array_mut()) {
            let home = home();
            for e in arr.iter_mut() {
                if let Some(s) = e.as_str() {
                    *e = Value::String(crate::feedback::scrub(s, &home));
                }
            }
        }
        let mut line = serde_json::to_string(report).unwrap_or_default();
        // Anywhere else a name might sit (longest first, so a full name goes whole)
        let mut names: Vec<(&String, &String)> = self.map.iter().collect();
        names.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        for (name, label) in names {
            if name.chars().count() >= 2 && line.contains(name.as_str()) {
                line = line.replace(name.as_str(), label);
            }
        }
        line
    }
}

/// A recording's Meet frames file (`meet.json`) with names replaced
fn hide_call(text: &str) -> Option<String> {
    let mut v: Value = serde_json::from_str(text).ok()?;
    let mut names = Names::default();
    names.learn(&v);
    Some(names.hide(&mut v))
}

/// The Meet reports between `from` and `to` (ms since 1970), names replaced
fn meet_reports(from: f64, to: f64, dst: &Path) -> Option<u64> {
    let mut picked: Vec<Value> = vec![];
    let mut bytes = 0u64;
    for dir in crate::meet::frames::sessions_dirs() {
        let mut files: Vec<PathBuf> = fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl")).collect();
        files.sort();
        for path in files {
            let modified = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64).unwrap_or(f64::MAX);
            if modified < from {
                continue;
            }
            let Ok(f) = fs::File::open(&path) else { continue };
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let t = v.get("t").and_then(|t| t.as_f64()).unwrap_or(0.0);
                if t < from || t > to {
                    continue;
                }
                bytes += line.len() as u64;
                if bytes > MEET_LIMIT {
                    break;
                }
                picked.push(v);
            }
        }
    }
    if picked.is_empty() {
        return None;
    }
    let mut names = Names::default();
    picked.iter().for_each(|v| names.learn(v));
    let mut out = String::new();
    for mut v in picked {
        out.push_str(&names.hide(&mut v));
        out.push('\n');
    }
    fs::create_dir_all(dst.parent()?).ok()?;
    fs::write(dst, &out).ok()?;
    Some(out.len() as u64)
}

/// A crash report of macOS, without the user's name and the Mac's ids
fn clean_crash(text: &str, home: &str) -> String {
    static IDS: once_cell::sync::Lazy<Option<regex::Regex>> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r#""(crashReporterKey|sleepWakeUUID|sessionID|deviceIdentifierForVendor|userID)"\s*:\s*("[^"]*"|\d+)"#).ok());
    let mut s = text.replace("\\/", "/");
    if home.len() > 1 {
        s = s.replace(home, "~");
    }
    s = logfile::scrub(&s);
    if let Some(re) = IDS.as_ref() {
        s = re.replace_all(&s, "\"$1\":\"…\"").into_owned();
    }
    s
}

/// macOS's crash reports of Protokol (and its helper) of the last `days` days
fn crash_reports(days: u64) -> Vec<PathBuf> {
    let Some(dir) = dirs::home_dir().map(|h| h.join("Library/Logs/DiagnosticReports")) else { return vec![] };
    let since = std::time::SystemTime::now() - Duration::from_secs(days * 24 * 3600);
    let mut out = vec![];
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".ips") || !e.metadata().and_then(|m| m.modified()).map(|m| m >= since).unwrap_or(false) {
            continue;
        }
        let head = fs::File::open(e.path()).ok().and_then(|f| BufReader::new(f).lines().next()).and_then(Result::ok).unwrap_or_default();
        let ours = head.contains(crate::identity::IDENTIFIER) || head.contains("\"app_name\":\"llama-helper\"") || head.contains("\"app_name\" : \"llama-helper\"");
        if ours {
            out.push(e.path());
        }
    }
    out.sort();
    out
}

// ===== System =====

/// The kind of the default microphone and output, never their names
fn sound_devices() -> String {
    use cpal::traits::{DeviceTrait, HostTrait};
    let names = crate::audio::devices::lookup::within(Duration::from_secs(3), "Default audio devices", || {
        let host = cpal::default_host();
        (host.default_input_device().and_then(|d| d.name().ok()), host.default_output_device().and_then(|d| d.name().ok()))
    });
    let label = |n: Option<String>| n.map(|n| logfile::device_label(&n)).unwrap_or_else(|| "немає".into());
    match names {
        Some((input, output)) => format!("мікрофон {}, вихід звуку {}", label(input), label(output)),
        None => "не вдалося прочитати (пристрій не відповідає)".into(),
    }
}

fn local_now() -> String {
    chrono::Local::now().format("%d.%m.%Y %H:%M (UTC%:z)").to_string()
}

/// «розпізнавання: Whisper large-v3 · документ: gemma4:26b»: the labels are
/// the app's own words and stay; the model names are cleaned as outside text
fn models_line(models: &str, home: &str) -> String {
    const LABELS: [&str; 2] = ["розпізнавання", "документ"];
    models
        .split(" · ")
        .filter_map(|part| {
            let (label, model) = part.split_once(": ")?;
            let label = LABELS.iter().find(|l| **l == label.trim())?;
            let model = crate::feedback::scrub(model.trim(), home);
            (!model.is_empty()).then(|| format!("{label}: {model}"))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn system_text(models: Option<&str>) -> String {
    let (model, memory) = crate::feedback::hardware();
    let macos = crate::feedback::run("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}")).unwrap_or_else(|| "macOS ?".into());
    let models = models.map(|m| models_line(m, &home())).filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "невідомо".into());
    format!(
        "Протокол {}\n{macos}\n{model}, {memory} пам'яті, {} вільно\nМоделі: {models}\nПристрої звуку: {}\nЗвіт складено: {}\n",
        env!("CARGO_PKG_VERSION"),
        crate::feedback::free_disk(),
        sound_devices(),
        local_now(),
    )
}

/// One line about this Mac when the app starts (the journal says where it ran)
pub fn log_system() {
    let (model, memory) = crate::feedback::hardware();
    let macos = crate::feedback::run("sw_vers", &["-productVersion"]).unwrap_or_default();
    log::info!("SYSTEM macOS {macos}, {model}, {memory}, {} free, UTC{}", crate::feedback::free_disk(), chrono::Local::now().format("%:z"));
}

/// The devices on this Mac, so their names stay out of the journal from the start
pub fn hide_devices_on_this_mac() {
    use cpal::traits::{DeviceTrait, HostTrait};
    let names = crate::audio::devices::lookup::within(Duration::from_secs(5), "Audio device names", || {
        cpal::default_host().devices().map(|d| d.filter_map(|d| d.name().ok()).collect::<Vec<_>>()).unwrap_or_default()
    });
    if let Some(names) = names {
        logfile::hide_device_names(names);
    }
}

// ===== The recording =====

/// The time span the Meet reports of a recording are taken from: the longest
/// of start to save by the clock, the length saved in metadata.json and the
/// end of the last transcript line. The clock runs to the save, so it also
/// holds the stop; the saved length can be far short: when live
/// transcription lags, it is the live transcript's end. The report shows
/// each of them under its own name, not one of them as the length of the
/// sound.
fn recording_secs(start_ms: Option<f64>, end_ms: Option<f64>, saved: Option<f64>, last_line: Option<f64>) -> Option<f64> {
    let clock = match (start_ms, end_ms) {
        (Some(s), Some(e)) if e > s => Some((e - s) / 1000.0),
        _ => None,
    };
    [clock, saved, last_line].into_iter().flatten().filter(|x| x.is_finite() && *x > 0.0).reduce(f64::max)
}

async fn meeting_files<R: Runtime>(app: &AppHandle<R>, meeting_id: &str, dir: &Path, files: &mut Vec<ReportFile>, home: &str) {
    let Ok(pool) = crate::speakers::commands::pool(app) else { return };
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as("SELECT folder_path, created_at FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(&pool).await.ok().flatten();
    let Some((folder, created)) = row else { return };
    let folder = folder.filter(|f| !f.is_empty()).map(PathBuf::from);
    let meeting_dir = dir.join("нарада");
    let _ = fs::create_dir_all(&meeting_dir);

    // Numbers from the database
    let mut summary = format!("Нарада: {meeting_id}\nСтворено: {}\n", created.unwrap_or_default());
    let mut last_line: Option<f64> = None;
    if let Ok((lines, speakers, without, manual, read, end)) = sqlx::query_as::<_, (i64, i64, i64, i64, i64, Option<f64>)>(
        "SELECT COUNT(*), COUNT(DISTINCT NULLIF(speaker, '')), COALESCE(SUM(CASE WHEN speaker IS NULL OR speaker = '' THEN 1 ELSE 0 END), 0), COALESCE(SUM(speaker_manual), 0), COALESCE(SUM(CASE WHEN reviewed_at IS NOT NULL THEN 1 ELSE 0 END), 0), MAX(audio_end_time) FROM transcripts WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    {
        summary.push_str(&format!(
            "Рядків стенограми: {lines}\nСпікерів у рядках: {speakers}\nРядків без спікера: {without}\nРядків зі спікером, заданим вручну: {manual}\nПеревірених рядків: {read}\nКінець останнього рядка: {:.0} с\n",
            end.unwrap_or(0.0)
        ));
        last_line = end;
    }
    if let Ok((voices, named, secs)) = sqlx::query_as::<_, (i64, i64, Option<f64>)>(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN name IS NOT NULL AND name != '' THEN 1 ELSE 0 END), 0), SUM(speech_seconds) FROM meeting_speakers WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    {
        summary.push_str(&format!("Голосів: {voices}, з іменем: {named}, мовлення: {:.0} с\n", secs.unwrap_or(0.0)));
    }
    if let Ok(Some((status, time, error))) = sqlx::query_as::<_, (String, Option<f64>, Option<String>)>("SELECT status, processing_time, error FROM summary_processes WHERE meeting_id = ?")
        .bind(meeting_id)
        .fetch_optional(&pool)
        .await
    {
        summary.push_str(&format!("Документ: {status}, {:.0} с", time.unwrap_or(0.0)));
        if let Some(e) = error.filter(|e| !e.trim().is_empty()) {
            summary.push_str(&format!(", помилка: {}", crate::feedback::scrub(&e, home)));
        }
        summary.push('\n');
    }

    let mut span: Option<(f64, f64)> = None;
    if let Some(folder) = folder.as_ref() {
        if let Ok(text) = fs::read_to_string(folder.join("metadata.json")) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let get = |k: &str| v.get(k).map(|x| x.to_string().trim_matches('"').to_string()).unwrap_or_default();
                let ms = |k: &str| v.get(k).and_then(|c| c.as_str()).and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok()).map(|t| t.timestamp_millis() as f64);
                let (start, saved) = (ms("created_at"), v.get("duration_seconds").and_then(|d| d.as_f64()));
                let secs = recording_secs(start, ms("completed_at"), saved, last_line);
                let whole = |x: Option<f64>| x.map(|x| format!("{x:.0} с")).unwrap_or_else(|| "невідомо".into());
                summary.push_str(&format!(
                    "Запис: початок {}, збережено {}, від початку до збереження {} (у metadata.json {}), стан {}, частота {} Гц\n",
                    get("created_at"),
                    get("completed_at"),
                    whole(recording_secs(start, ms("completed_at"), None, None)),
                    whole(saved),
                    get("status"),
                    get("sample_rate")
                ));
                let secs = secs.unwrap_or(3.0 * 3600.0);
                span = start.map(|s| (s - 120_000.0, s + secs * 1000.0 + 120_000.0));
            }
        }
        let files_in: Vec<String> = fs::read_dir(folder).into_iter().flatten().flatten().filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            (!name.starts_with('.')).then(|| format!("{name} ({size} Б)"))
        }).collect();
        summary.push_str(&format!("Файли в папці запису: {}\n", files_in.join(", ")));
    }
    let _ = fs::write(meeting_dir.join("зведення.txt"), &summary);
    files.push(ReportFile { name: "нарада/зведення.txt".into(), bytes: summary.len() as u64, about: "Числа про нараду: рядки, голоси, документ, тривалість".into() });

    if let Some(folder) = folder.as_ref() {
        let journal = folder.join(logfile::MEETING_FILE);
        if journal.exists() {
            if let Ok(bytes) = copy_clean(&journal, &meeting_dir.join("журнал наради.log"), home, None) {
                files.push(ReportFile { name: "нарада/журнал наради.log".into(), bytes, about: "Що програма робила з цим записом, від початку до документа".into() });
            }
        }
        if let Some(text) = fs::read_to_string(folder.join(crate::meet::frames::FOLDER_FILE)).ok().and_then(|t| hide_call(&t)) {
            if fs::write(meeting_dir.join("meet-кадри.json"), &text).is_ok() {
                files.push(ReportFile { name: "нарада/meet-кадри.json".into(), bytes: text.len() as u64, about: "Хто світився в Meet кожні 100 мс; імена замінено на «Учасник N»".into() });
            }
        }
    }
    if let Some((from, to)) = span {
        if let Some(bytes) = meet_reports(from, to, &meeting_dir.join("meet-звіти.jsonl")) {
            files.push(ReportFile { name: "нарада/meet-звіти.jsonl".into(), bytes, about: "Звіти вікна Meet за час запису; імена замінено на «Учасник N»".into() });
        }
    }
}

// ===== Commands =====

fn clear_old_staging(keep: Option<&str>) {
    for e in fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(STAGING_PREFIX) && keep.map_or(true, |k| !name.ends_with(k)) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

fn file_list(dir: &Path) -> Vec<(String, u64)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push((rel.to_string_lossy().to_string(), e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    let mut out = vec![];
    walk(dir, dir, &mut out);
    out.sort();
    out
}

const NEVER: &str = "Ніколи не додається: звук, тексти стенограм і документів, назви нарад, імена людей і назви їхніх пристроїв, ім'я користувача Mac.";

/// Puts the report's files together (without the person's words yet), for
/// the form to show before anything is saved
#[tauri::command]
pub async fn api_report_prepare<R: Runtime>(app: AppHandle<R>, meeting_id: Option<String>, models: Option<String>) -> Result<PreparedReport, String> {
    let code = new_code();
    clear_old_staging(None);
    let dir = staging(&code);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    logfile::set_recordings_folder(&crate::audio::recording_preferences::recordings_folder());
    logfile::write_black_box("report");
    log::info!("REPORT ЗВ-{code} being prepared{}", if meeting_id.is_some() { " with a recording" } else { "" });
    let home = home();
    let mut files: Vec<ReportFile> = vec![];

    let sys = {
        let models = models.clone();
        tokio::task::spawn_blocking(move || system_text(models.as_deref())).await.unwrap_or_default()
    };
    let _ = fs::write(dir.join("система.txt"), &sys);
    files.push(ReportFile { name: "система.txt".into(), bytes: sys.len() as u64, about: "Версія, macOS, Mac, пам'ять, вільне місце, моделі, вид пристроїв звуку".into() });

    let (dir2, home2) = (dir.clone(), home.clone());
    let journal = tokio::task::spawn_blocking(move || {
        let mut out = vec![];
        let since = (chrono::Utc::now() - chrono::Duration::days(logfile::KEEP_DAYS)).format("%Y-%m-%d").to_string();
        for src in logfile::recent_files(logfile::KEEP_DAYS) {
            let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let about = if name.starts_with("protokol-") { "Журнал програми за день" } else { "Журнал попередньої версії програми, останні 7 днів" };
            if let Ok(bytes) = copy_clean(&src, &dir2.join("журнал").join(&name), &home2, Some(&since)) {
                out.push(ReportFile { name: format!("журнал/{name}"), bytes, about: about.into() });
            }
        }
        for src in crash_reports(logfile::KEEP_DAYS as u64) {
            let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if let Ok(text) = fs::read_to_string(&src) {
                let text = clean_crash(&text, &home2);
                let _ = fs::create_dir_all(dir2.join("падіння"));
                if fs::write(dir2.join("падіння").join(&name), &text).is_ok() {
                    out.push(ReportFile { name: format!("падіння/{name}"), bytes: text.len() as u64, about: "Звіт macOS про падіння програми".into() });
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default();
    files.extend(journal);

    if let Some(id) = meeting_id.as_deref().filter(|id| !id.is_empty()) {
        meeting_files(&app, id, &dir, &mut files, &home).await;
    }

    files.insert(0, ReportFile { name: "опис.txt".into(), bytes: 0, about: "Ваш опис, час звіту і позначки".into() });
    files.push(ReportFile { name: "зміст.txt".into(), bytes: 0, about: "Перелік файлів звіту".into() });
    Ok(PreparedReport { code, files, marks: logfile::marks_since(logfile::KEEP_DAYS) })
}

fn inside(code: &str, name: &str) -> Result<PathBuf, String> {
    if !valid_code(code) || name.contains("..") || name.starts_with('/') {
        return Err("Немає такого файлу".into());
    }
    let p = staging(code).join(name);
    if p.is_file() {
        Ok(p)
    } else {
        Err("Немає такого файлу".into())
    }
}

/// «Переглянути»: the file opens in TextEdit
#[tauri::command]
pub async fn api_report_open(code: String, name: String) -> Result<(), String> {
    let p = inside(&code, &name)?;
    std::process::Command::new("open").arg("-e").arg(&p).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// «Показати всі файли»: the report's folder in Finder
#[tauri::command]
pub async fn api_report_show_all(code: String) -> Result<(), String> {
    if !valid_code(&code) {
        return Err("Немає такого звіту".into());
    }
    crate::settings_extra::reveal_in_finder(&staging(&code))
}

/// The form closed without saving
#[tauri::command]
pub async fn api_report_discard(code: String) {
    if valid_code(&code) {
        let _ = fs::remove_dir_all(staging(&code));
    }
}

fn description(code: &str, what: &str, expected: &str, meeting: Option<&str>, marks: &[logfile::Mark]) -> String {
    let mut s = format!("{}\nСкладено: {}\nПротокол {}\n\nЩо сталося:\n{}\n\nЩо мало статися:\n{}\n", report_name(code), local_now(), env!("CARGO_PKG_VERSION"), what.trim(), if expected.trim().is_empty() { "—" } else { expected.trim() });
    if let Some(m) = meeting {
        s.push_str(&format!("\nНарада: {m}\n"));
    }
    if !marks.is_empty() {
        s.push_str("\nПозначки «Позначити проблему» (UTC):\n");
        for m in marks {
            s.push_str(&format!("#{} {} ({}){}\n", m.n, m.t, m.source, m.note.as_deref().map(|n| format!(": {n}")).unwrap_or_default()));
        }
    }
    s
}

fn zip_dir(dir: &Path, root: &str, out: &Path) -> Result<(), String> {
    let file = fs::File::create(out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (rel, _) in file_list(dir) {
        zip.start_file(format!("{root}/{rel}"), opts).map_err(|e| e.to_string())?;
        let mut f = fs::File::open(dir.join(&rel)).map_err(|e| e.to_string())?;
        std::io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(format!("{name}.zip"));
    if !first.exists() {
        return first;
    }
    (2..100).map(|i| dir.join(format!("{name} ({i}).zip"))).find(|p| !p.exists()).unwrap_or(first)
}

/// «Зберегти звіт»: the person's words go in, the zip goes to the Desktop
#[tauri::command]
pub async fn api_report_save(code: String, what: String, expected: String, meeting_id: Option<String>) -> Result<SavedReport, String> {
    if !valid_code(&code) {
        return Err("Звіт уже закрито. Почніть знову".into());
    }
    let dir = staging(&code);
    if !dir.is_dir() {
        return Err("Звіт уже закрито. Почніть знову".into());
    }
    let marks = logfile::marks_since(logfile::KEEP_DAYS);
    let text = description(&code, &what, &expected, meeting_id.as_deref(), &marks);
    fs::write(dir.join("опис.txt"), &text).map_err(|e| e.to_string())?;
    let mut list = format!("{}\n\n", report_name(&code));
    for (rel, bytes) in file_list(&dir) {
        list.push_str(&format!("{rel} — {bytes} Б\n"));
    }
    list.push_str(&format!("\n{NEVER}\n"));
    fs::write(dir.join("зміст.txt"), list).map_err(|e| e.to_string())?;

    let desktop = dirs::desktop_dir().or_else(dirs::home_dir).ok_or("Немає робочого столу")?;
    let out = free_path(&desktop, &report_name(&code));
    let (d, o, root) = (dir.clone(), out.clone(), report_name(&code));
    tokio::task::spawn_blocking(move || zip_dir(&d, &root, &o)).await.map_err(|e| e.to_string())??;
    let bytes = fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let count = file_list(&dir).len();
    let _ = fs::remove_dir_all(&dir);
    log::info!("REPORT ЗВ-{code} saved: {count} file(s), {} KB", bytes / 1024);
    Ok(SavedReport { path: out.to_string_lossy().to_string(), code, bytes })
}

fn saved_report(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.starts_with("Звіт Протоколу ЗВ-") && name.ends_with(".zip") && p.is_file() {
        Ok(p)
    } else {
        Err("Файл звіту не знайдено".into())
    }
}

/// «Показати у Finder»
#[tauri::command]
pub async fn api_report_reveal(path: String) -> Result<(), String> {
    let p = saved_report(&path)?;
    std::process::Command::new("open").arg("-R").arg(&p).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// «Поділитися…»: the system's share menu (Mail, Messages, AirDrop…) next to the button
#[tauri::command]
pub async fn api_report_share<R: Runtime>(app: AppHandle<R>, path: String, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    let p = saved_report(&path)?;
    #[cfg(target_os = "macos")]
    {
        let window = app.get_webview_window("main").ok_or("Немає вікна")?;
        let w = window.clone();
        window
            .run_on_main_thread(move || unsafe { share_picker(&w, &p, x, y, width, height) })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, p, x, y, width, height);
        Err("Лише на Mac".into())
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct NsPoint {
    x: f64,
    y: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct NsSize {
    width: f64,
    height: f64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct NsRect {
    origin: NsPoint,
    size: NsSize,
}

#[cfg(target_os = "macos")]
unsafe impl objc::Encode for NsRect {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{CGRect={CGPoint=dd}{CGSize=dd}}") }
    }
}

/// NSSharingServicePicker over the window's content view; the rect comes
/// from the page (top-left origin), the view may count from the bottom
#[cfg(target_os = "macos")]
unsafe fn share_picker<R: Runtime>(window: &tauri::WebviewWindow<R>, path: &Path, x: f64, y: f64, w: f64, h: f64) {
    use objc::runtime::{Object, BOOL, NO};
    use objc::{class, msg_send, sel, sel_impl};
    let Ok(view) = window.ns_view() else { return };
    let view = view as *mut Object;
    if view.is_null() {
        return;
    }
    let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) else { return };
    let s: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
    let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: s];
    let items: *mut Object = msg_send![class!(NSArray), arrayWithObject: url];
    let picker: *mut Object = msg_send![class!(NSSharingServicePicker), alloc];
    let picker: *mut Object = msg_send![picker, initWithItems: items];
    if picker.is_null() {
        return;
    }
    let flipped: BOOL = msg_send![view, isFlipped];
    let bounds: NsRect = msg_send![view, bounds];
    let top = if flipped == NO { bounds.size.height - y - h } else { y };
    let rect = NsRect { origin: NsPoint { x, y: top }, size: NsSize { width: w.max(1.0), height: h.max(1.0) } };
    // NSRectEdgeMinY: under the button when the view counts from the bottom
    let edge: u64 = if flipped == NO { 1 } else { 3 };
    let _: () = msg_send![picker, showRelativeToRect: rect ofView: view preferredEdge: edge];
    // The picker stays alive while its menu is open (one small object per share)
}

// ===== The journal from the windows =====

/// «Позначити проблему» from the menu, the menu bar icon, the recording panel or the Meet window
#[tauri::command]
pub async fn api_journal_mark<R: Runtime>(app: AppHandle<R>, note: Option<String>, source: Option<String>) -> logfile::Mark {
    mark_now(&app, note.as_deref(), source.as_deref().unwrap_or("window"))
}

pub fn mark_now<R: Runtime>(app: &AppHandle<R>, note: Option<&str>, source: &str) -> logfile::Mark {
    let mark = logfile::mark(note, source);
    let _ = app.emit("journal-mark", &mark);
    mark
}

/// The marks of the last 7 days
#[tauri::command]
pub async fn api_journal_marks() -> Vec<logfile::Mark> {
    logfile::marks_since(logfile::KEEP_DAYS)
}

static UI_LINES: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

/// An error in a window (a script error, a failed promise): one line, its
/// text cleaned hard (no Ukrainian text, quoted values or paths), 30 a minute at most
#[tauri::command]
pub async fn api_journal_ui(kind: String, message: String, source: Option<String>) {
    {
        let mut g = UI_LINES.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match g.as_mut() {
            Some((since, n)) if now.duration_since(*since) < Duration::from_secs(60) => {
                if *n >= 30 {
                    return;
                }
                *n += 1;
            }
            _ => *g = Some((now, 1)),
        }
    }
    let home = home();
    let kind: String = kind.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    let message = crate::feedback::scrub(&message, &home);
    let source = source.map(|s| crate::feedback::scrub(&s, &home)).unwrap_or_default();
    log::warn!(target: "app_lib::ui", "UI-ERROR {kind}: {message}{}", if source.is_empty() { String::new() } else { format!(" ({source})") });
}

/// A page shown in the main window (its path only, no query): for the black box
#[tauri::command]
pub async fn api_journal_page(path: String) {
    let path: String = path.split(['?', '#']).next().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || "/-_".contains(*c)).take(60).collect();
    log::debug!(target: "app_lib::ui", "page {path}");
}

/// The last run ended with an error: once, for the start page to offer a report
#[tauri::command]
pub async fn api_journal_last_crash() -> Option<String> {
    logfile::last_crash()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meet_names_become_participants_and_the_link_goes() {
        let mut a = serde_json::json!({"t": 1.0, "self": "Олена Тест", "names": ["Олена Тест", "Гість Перший", "Гість Другий"], "meeting": "abc-defg-hij", "errors": ["tile: /Users/someone/x.js failed"], "lit": [[1]]});
        let mut b = serde_json::json!({"t": 2.0, "names": ["Гість Другий", "Гість Третій"], "classHits": {"Гість Третій": 1}});
        let mut names = Names::default();
        names.learn(&a);
        names.learn(&b);
        let la = names.hide(&mut a);
        let lb = names.hide(&mut b);
        for n in ["Олена", "Перший", "Другий", "Третій", "abc-defg-hij", "someone"] {
            assert!(!la.contains(n) && !lb.contains(n), "{n}: {la} {lb}");
        }
        assert!(la.contains("\"self\":\"Я\""), "{la}");
        assert!(la.contains("Учасник 1") && la.contains("Учасник 2"), "{la}");
        // The same person keeps the same label in every report
        assert!(lb.contains("\"names\":[\"Учасник 2\",\"Учасник 3\"]"), "{lb}");
    }

    #[test]
    fn crash_reports_lose_the_user_and_the_macs_ids() {
        let text = r#"{"app_name":"Протокол","bundleID":"ua.protokol.app","crashReporterKey":"ABC-123","userID":501}
{"procPath":"\/Users\/olena\/Desktop\/Протокол.app\/Contents\/MacOS\/protokol","sessionID":"XYZ"}"#;
        let s = clean_crash(text, "/Users/olena");
        assert!(!s.contains("olena"), "{s}");
        assert!(!s.contains("ABC-123") && !s.contains("XYZ") && !s.contains("501"), "{s}");
        assert!(s.contains("~/Desktop/Протокол.app"), "{s}");
    }

    #[test]
    fn journal_lines_are_cleaned_again_on_the_way_in() {
        assert_eq!(clean_line("[t INFO  app_lib::whisper_engine] text='Доброго ранку'\n", "/Users/olena"), None);
        let l = clean_line("[t INFO  app_lib::api::api] Fetched meeting title: Склад 03.11\n", "/Users/olena").unwrap();
        assert!(!l.contains("Склад"), "{l}");
        let p = clean_line("[t INFO  x] at /Users/olena/Library/x\n", "/Users/olena").unwrap();
        assert!(!p.contains("olena"), "{p}");
    }

    #[test]
    fn codes_and_names() {
        let c = new_code();
        assert!(valid_code(&c), "{c}");
        assert!(!valid_code("0OI1"));
        assert!(!valid_code("../x"));
        assert_eq!(report_name("7K3F"), "Звіт Протоколу ЗВ-7K3F");
        let d = description("7K3F", "Не з'явились спікери", "", Some("meeting-1"), &[logfile::Mark { n: 2, t: "2026-11-21T10:42:00Z".into(), note: None, source: "menu".into() }]);
        assert!(d.contains("Що мало статися:\n—"), "{d}");
        assert!(d.contains("#2 2026-11-21T10:42:00Z (menu)"), "{d}");
    }

    #[test]
    fn model_labels_stay_and_model_names_are_cleaned() {
        assert_eq!(models_line("розпізнавання: Whisper large-v3 · документ: gemma4:26b", "/Users/olena"), "розпізнавання: Whisper large-v3 · документ: gemma4:26b");
        let m = models_line("розпізнавання: Whisper /Users/olena/models/моя-модель.bin · хтось: x", "/Users/olena");
        assert!(m.starts_with("розпізнавання: Whisper ") && !m.contains("olena") && !m.contains("моя") && !m.contains("хтось"), "{m}");
    }

    #[test]
    fn the_report_span_is_the_longest_known() {
        // From start to save, and a short length saved from a lagging live transcript
        assert_eq!(recording_secs(Some(0.0), Some(807_000.0), Some(163.98999999999998), Some(704.0)), Some(807.0));
        assert_eq!(recording_secs(Some(0.0), Some(807_000.0), None, None), Some(807.0));
        assert_eq!(recording_secs(None, None, Some(30.5), None), Some(30.5));
        assert_eq!(recording_secs(Some(5.0), Some(1.0), None, Some(f64::NAN)), None);
    }

    #[test]
    fn old_days_and_repeats_stay_out_of_a_copy() {
        let dir = std::env::temp_dir().join(format!("protokol-copy-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let src = dir.join("protokol.log");
        let mut text = String::from("[2026-11-01T09:00:00Z INFO  app_lib::x] an old day\n  and its second line\n");
        for i in 0..40 {
            text.push_str(&format!("[2026-11-21T10:00:{:02}Z WARN  app_lib::audio::vad] frame {i} too short\n", i));
        }
        text.push_str("[2026-11-21T10:01:00Z WARN  app_lib::audio::vad] frame 41 too short\n");
        text.push_str("[2026-11-21T10:01:01Z INFO  app_lib::whisper_engine] text='Доброго ранку'\n");
        text.push_str("[2026-11-21T10:01:02Z INFO  app_lib::x] at /Users/olena/Library/x\n  second line kept\n");
        fs::write(&src, text).unwrap();
        let dst = dir.join("out.log");
        copy_clean(&src, &dst, "/Users/olena", Some("2026-11-15")).unwrap();
        let out = fs::read_to_string(&dst).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(!out.contains("an old day") && !out.contains("and its second line"), "{out}");
        // 5 in the first minute, 1 in the next, 35 counted at the end
        assert_eq!(out.lines().filter(|l| l.starts_with('[') && l.contains("too short")).count(), 6, "{out}");
        assert!(out.contains("в копію не ввійшли: 35") && out.contains("×35 [2026-11-21T10:00:05Z WARN  app_lib::audio::vad] frame 5 too short"), "{out}");
        assert!(!out.contains("Доброго") && !out.contains("olena") && out.contains("second line kept"), "{out}");
    }
}
