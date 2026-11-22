//! The app's journal: what the app did and in what state, never what was said.
//!
//! - Terminal: every line from info up, as before.
//! - Files: `~/Library/Logs/ua.protokol.app/protokol-<day>.log`, one per day
//!   (UTC, as the times in the lines), kept [`KEEP_DAYS`] days and 50 MB in
//!   all. After 20 MB a day's file takes only warnings and errors.
//! - Lines that could hold what was said in a meeting (recognised text from
//!   the speech modules) go to the terminal only. Meeting titles, recording
//!   folders, notification texts and the Mac's user name are taken out of the
//!   lines written to files (see [`scrub`]).
//! - The same line again and again (the same words, other numbers) is written
//!   5 times a minute at most; then one line says how many more there were.
//! - Black box: the app's own debug lines and the repeats left out are kept
//!   in memory (the last 2 000) and written out only when an error happens,
//!   the person marks a problem or a report is made: the detail around a
//!   problem, without a large file every day.
//! - A recording's own journal: while a recording is made or processed, the
//!   lines also go to `diagnostics.log` in its folder (see [`meeting_scope`]),
//!   so a meeting of last month can still be looked into.
//! - A panic is written with where it happened. If the app then closed, the
//!   next start says so (see [`last_crash`]).
//! - «Позначити проблему»: [`mark`] writes the moment (and the person's note
//!   in a separate file, `marks.jsonl`, for the problem report only).
//!
//! Short codes in capitals (REC-START, MEET-WINDOW, USER-MARK, PANIC…) mark
//! the events worth searching for.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use log::{Level, LevelFilter, Log, Metadata, Record};
use serde::{Deserialize, Serialize};

/// Days of journal files kept, and how far back a problem report looks
pub const KEEP_DAYS: i64 = 7;
/// After this a day's file takes warnings and errors only
const DAY_LIMIT: u64 = 20 * 1024 * 1024;
/// All the day files together
const TOTAL_LIMIT: u64 = 50 * 1024 * 1024;
/// One recording's journal
const MEETING_LIMIT: u64 = 5 * 1024 * 1024;
/// Lines kept in memory for the black box
const BLACK_BOX_LINES: usize = 2000;
/// The same line is written this many times per window, then counted
const REPEAT_KEEP: u32 = 5;
const REPEAT_WINDOW: Duration = Duration::from_secs(60);
/// Lines different enough to be told apart (the repeat counter forgets the rest)
const REPEAT_KEYS: usize = 512;
/// An error writes the black box out at most this often
const ERROR_BOX_EVERY: Duration = Duration::from_secs(30);
/// A recording's lines keep going to its journal this long after it stops:
/// the transcript, the speakers and the document are made in this time
const AFTER_RECORDING: Duration = Duration::from_secs(60 * 60);

/// A recording's journal, in its folder
pub const MEETING_FILE: &str = "diagnostics.log";
const MARKS_FILE: &str = "marks.jsonl";
/// Exists while the app runs; left behind when it did not close normally
const RUNNING_FILE: &str = "running";
/// Written by a panic
const PANICKED_FILE: &str = "panicked";
/// The files of 0.4.3 and earlier (one file, one older copy)
const LEGACY_FILES: [&str; 2] = ["protokol.log", "protokol.1.log"];

/// Modules whose info lines carry recognised speech; never written to the file
const PRIVATE_TARGETS: &[&str] = &["app_lib::audio::transcription", "app_lib::whisper_engine", "app_lib::parakeet_engine"];
const PRIVATE_MARKS: &[&str] = &["text='", "transcribed:", "transcription result", "Clean transcript text", "Flushing final partial"];

pub(crate) fn private(line: &str) -> bool {
    PRIVATE_TARGETS.iter().any(|t| line.contains(t)) || PRIVATE_MARKS.iter().any(|m| line.contains(m))
}

/// What a file line must not carry, and what stands in its place. The
/// recording folder is named after the meeting («Нарада … (14)»).
static SCRUB: once_cell::sync::Lazy<Vec<(regex::Regex, &'static str)>> = once_cell::sync::Lazy::new(|| {
    [
        (r"/Users/[^/\s]+/", "~/"),
        (r"(Movies/Protokol[^/\n]*/)[^/\x22\n]+", "${1}…"),
        (r"(?i)(meeting title: )[^\n]*", "${1}…"),
        (r#"(?i)(meeting(?:_name)?: Some\(")[^"]*"#, "${1}…"),
        (r"(?i)(notification: )[^\n]*", "${1}…"),
        (r"«[^»\n]*»", "«…»"),
    ]
    .into_iter()
    .filter_map(|(re, to)| regex::Regex::new(re).ok().map(|re| (re, to)))
    .collect()
});

/// The recordings folder chosen in Settings, when it is not the usual one:
/// the folder inside it is named after the meeting as well
static RECORDINGS: Mutex<Option<(String, regex::Regex)>> = Mutex::new(None);

/// Settings → Сховище: where the recordings are, so their folder names
/// (meeting titles) are taken out of the lines too
pub fn set_recordings_folder(folder: &Path) {
    let mut s = folder.to_string_lossy().trim_end_matches('/').to_string();
    if let Some(home) = dirs::home_dir().map(|h| h.to_string_lossy().trim_end_matches('/').to_string()) {
        if let Some(rest) = s.strip_prefix(&home) {
            s = format!("~{rest}");
        }
    }
    // The usual place is in SCRUB already; a folder too short to be sure of is left
    if s.contains("Movies/Protokol") || s.len() < 4 {
        *lock(&RECORDINGS) = None;
        return;
    }
    let mark = s.trim_start_matches('~').to_string();
    if let Ok(re) = regex::Regex::new(&format!(r"({}/)[^/\x22\n]+", regex::escape(&mark))) {
        *lock(&RECORDINGS) = Some((mark, re));
    }
}

/// Audio devices are often named after their owner («AirPods Олени»): each
/// name seen is replaced by its kind and a short tag, the same one each time
static DEVICES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// A quoted name shortly after a word about a device («Using default
/// microphone: 'AirPods …'», «Bluetooth detected for '…'»), and «for device:
/// <name>». Looked for in the message only, not in the module's name
/// (app_lib::audio::…), so other quoted values of audio lines are left alone.
static DEVICE_NAME: once_cell::sync::Lazy<Vec<regex::Regex>> = once_cell::sync::Lazy::new(|| {
    [
        r"(?i)(?:device|microphone|system audio|bluetooth|built-in|usb|detected for|type for)[^'\n]{0,40}'([^'\n]{3,80})'",
        r"(?i)for device: ([^\n,']{3,80})",
    ]
        .into_iter()
        .filter_map(|re| regex::Regex::new(re).ok())
        .collect()
});

/// What a device is, from its name: built in, Bluetooth, virtual…
pub(crate) fn device_label(name: &str) -> String {
    let n = name.to_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
    let kind = if any(&["macbook", "mac mini", "imac", "mac studio", "mac pro", "built-in", "вбудован", "studio display"]) {
        "вбудований"
    } else if any(&["blackhole", "teams", "zoom", "soundflower", "loopback", "aggregate", "агрегат", "multi-output", "steam", "virtual", "protokol", "протокол"]) {
        "віртуальний"
    } else if any(&["airpods", "buds", "beats", "bluetooth", "jabra", "bose", "jbl", "wh-", "wf-", "headphones", "headset", "навушник"]) {
        "Bluetooth"
    } else if any(&["iphone", "ipad"]) {
        "iPhone"
    } else if n.contains("usb") {
        "USB"
    } else {
        "інший"
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    format!("[пристрій {kind} {:03x}]", h.finish() & 0xfff)
}

/// Device names to take out of the lines (the devices on this Mac, listed when the app starts)
pub fn hide_device_names<I: IntoIterator<Item = String>>(names: I) {
    let mut known = lock(&DEVICES);
    for name in names {
        let name = name.trim().to_string();
        let generic = ["default", "none", "system", "unknown", "microphone", "speaker"].contains(&name.to_lowercase().as_str());
        if name.chars().count() < 3 || generic || name.contains('…') || name.starts_with("[пристрій") || known.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let label = device_label(&name);
        known.push((name, label));
        // Longer names first: «AirPods Олени - Find My» before «AirPods Олени»
        known.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }
}

fn hide_devices(line: &str) -> Option<String> {
    if line.contains('\'') || line.contains("device: ") {
        // «[time LEVEL module] message»: the message only
        let message = line.find("] ").map(|i| &line[i + 2..]).unwrap_or(line);
        let found: Vec<String> = DEVICE_NAME.iter().flat_map(|re| re.captures_iter(message).filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string())).collect::<Vec<_>>()).collect();
        if !found.is_empty() {
            hide_device_names(found);
        }
    }
    let known = lock(&DEVICES);
    if !known.iter().any(|(n, _)| line.contains(n.as_str())) {
        return None;
    }
    let mut s = line.to_string();
    for (name, label) in known.iter() {
        if s.contains(name.as_str()) {
            s = s.replace(name.as_str(), label);
        }
    }
    Some(s)
}

/// A line as written to the file: no meeting titles, folders or user name.
/// «for meeting: …» keeps a meeting id («meeting-…») and hides a title.
pub(crate) fn scrub(line: &str) -> String {
    static FOR_MEETING: once_cell::sync::Lazy<Option<regex::Regex>> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)(for meeting: )([^,\n]*)").ok());
    const MARKS: &[&str] = &["/Users/", "Movies/Protokol", "eeting", "otification", "«"];
    let devices_hidden = hide_devices(line);
    let line = devices_hidden.as_deref().unwrap_or(line);
    let chosen = lock(&RECORDINGS).as_ref().filter(|(mark, _)| line.contains(mark.as_str())).map(|(_, re)| re.clone());
    if chosen.is_none() && !MARKS.iter().any(|m| line.contains(m)) {
        return line.to_string();
    }
    let mut s = line.to_string();
    for (re, to) in SCRUB.iter() {
        s = re.replace_all(&s, *to).into_owned();
    }
    if let Some(re) = chosen {
        s = re.replace_all(&s, "${1}…").into_owned();
    }
    if let Some(re) = FOR_MEETING.as_ref() {
        s = re
            .replace_all(&s, |c: &regex::Captures| {
                if c[2].starts_with("meeting-") {
                    c[0].to_string()
                } else {
                    format!("{}…", &c[1])
                }
            })
            .into_owned();
    }
    s
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `~/Library/Logs/ua.protokol.app`
pub fn log_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join("Library/Logs").join(crate::identity::IDENTIFIER))
}

fn day_name(date: &str) -> String {
    format!("protokol-{date}.log")
}

/// «2026-11-21» from «protokol-2026-11-21.log»
fn day_of(name: &str) -> Option<&str> {
    let d = name.strip_prefix("protokol-")?.strip_suffix(".log")?;
    (d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-').then_some(d)
}

/// The day files to delete: older than `keep_days` before `today`, or past
/// `total` bytes counting from the newest (today's file always stays)
fn to_delete(files: &[(String, u64)], today: &str, keep_days: i64, total: u64) -> Vec<String> {
    let oldest = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .map(|d| (d - chrono::Duration::days(keep_days - 1)).format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let mut sorted: Vec<&(String, u64)> = files.iter().collect();
    sorted.sort_by(|a, b| b.0.cmp(&a.0));
    let mut sum = 0u64;
    let mut out = vec![];
    for (date, size) in sorted {
        sum += size;
        if date.as_str() != today && (date.as_str() < oldest.as_str() || sum > total) {
            out.push(date.clone());
        }
    }
    out
}

/// The journal's files of the last `days` days, oldest first (the files of
/// 0.4.3 and earlier too, while they are that recent)
pub fn recent_files(days: i64) -> Vec<PathBuf> {
    let Some(dir) = log_dir() else { return vec![] };
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%d").to_string();
    let mut days_found: Vec<(String, PathBuf)> = vec![];
    let mut legacy: Vec<PathBuf> = vec![];
    let limit = std::time::SystemTime::now() - Duration::from_secs(days.max(0) as u64 * 24 * 3600);
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(d) = day_of(&name) {
            if d >= since.as_str() {
                days_found.push((d.to_string(), e.path()));
            }
        } else if LEGACY_FILES.contains(&name.as_str()) {
            let fresh = e.metadata().and_then(|m| m.modified()).map(|m| m >= limit).unwrap_or(false);
            if fresh && e.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                legacy.push(e.path());
            }
        }
    }
    days_found.sort();
    // protokol.1.log is older than protokol.log
    legacy.sort_by(|a, b| b.cmp(a));
    legacy.into_iter().chain(days_found.into_iter().map(|(_, p)| p)).collect()
}

// ===== The same line again and again =====

/// What the repeat counter says about a line
#[derive(Debug, PartialEq)]
enum Gate {
    Write,
    /// Write it, after a line saying how many were left out before
    WriteAfter { skipped: u32, secs: u64, sample: String },
    Skip,
}

struct Repeat {
    since: Instant,
    written: u32,
    skipped: u32,
    sample: String,
}

#[derive(Default)]
struct Repeats {
    by_key: HashMap<u64, Repeat>,
    swept: Option<Instant>,
}

/// The line's words without its numbers: «buffer is large: 1000320 samples
/// (62.5s)» and «… 1000800 samples (62.5s)» are the same line said again
fn repeat_key(target: &str, message: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    target.hash(&mut h);
    let mut digits = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !digits {
                '#'.hash(&mut h);
            }
            digits = true;
        } else {
            digits = false;
            c.hash(&mut h);
        }
    }
    h.finish()
}

impl Repeats {
    fn check(&mut self, key: u64, message: &str, now: Instant) -> Gate {
        if self.by_key.len() >= REPEAT_KEYS && !self.by_key.contains_key(&key) {
            // Many different lines: forget the windows that are over
            self.by_key.retain(|_, r| now.duration_since(r.since) < REPEAT_WINDOW && r.skipped > 0);
            if self.by_key.len() >= REPEAT_KEYS {
                return Gate::Write;
            }
        }
        match self.by_key.get_mut(&key) {
            Some(r) if now.duration_since(r.since) < REPEAT_WINDOW => {
                if r.written < REPEAT_KEEP {
                    r.written += 1;
                    Gate::Write
                } else {
                    if r.skipped == 0 {
                        r.sample = message.chars().take(200).collect();
                    }
                    r.skipped += 1;
                    Gate::Skip
                }
            }
            Some(r) => {
                let gate = if r.skipped > 0 {
                    Gate::WriteAfter { skipped: r.skipped, secs: now.duration_since(r.since).as_secs(), sample: std::mem::take(&mut r.sample) }
                } else {
                    Gate::Write
                };
                *r = Repeat { since: now, written: 1, skipped: 0, sample: String::new() };
                gate
            }
            None => {
                self.by_key.insert(key, Repeat { since: now, written: 1, skipped: 0, sample: String::new() });
                Gate::Write
            }
        }
    }

    /// Lines that stopped coming: how many of them were left out. At most once a second.
    fn sweep(&mut self, now: Instant) -> Vec<(u32, u64, String)> {
        if self.swept.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return vec![];
        }
        self.swept = Some(now);
        let mut out = vec![];
        self.by_key.retain(|_, r| {
            if now.duration_since(r.since) < REPEAT_WINDOW {
                return true;
            }
            if r.skipped > 0 {
                out.push((r.skipped, now.duration_since(r.since).as_secs(), std::mem::take(&mut r.sample)));
            }
            false
        });
        out
    }
}

fn repeat_line(stamp: &str, skipped: u32, secs: u64, sample: &str) -> String {
    format!("[{stamp} INFO  app_lib::logfile] REPEAT ×{skipped} more in {secs}s, left out: {sample}\n")
}

// ===== The files =====

struct MeetingJournal {
    folder: PathBuf,
    file: Option<File>,
    bytes: u64,
    /// Work on this recording now running (speakers, the document…)
    holders: u32,
    /// The recording itself runs
    recording: bool,
    /// After the recording stopped: until when its lines still come here
    until: Option<Instant>,
}

impl MeetingJournal {
    fn active(&self, now: Instant) -> bool {
        self.holders > 0 || self.recording || self.until.is_some_and(|u| now < u)
    }
}

#[derive(Default)]
struct State {
    dir: Option<PathBuf>,
    date: String,
    file: Option<File>,
    bytes: u64,
    full_noted: bool,
    repeats: Repeats,
    /// The same for debug lines: a busy loop must not fill the black box alone
    debug_repeats: Repeats,
    black_box: VecDeque<String>,
    box_written: Option<Instant>,
    meetings: Vec<MeetingJournal>,
}

static STATE: once_cell::sync::Lazy<Mutex<State>> = once_cell::sync::Lazy::new(|| Mutex::new(State::default()));

thread_local! {
    /// A line being written on this thread: a line logged from inside (or a
    /// panic there) must not wait for the lock it holds
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

impl State {
    fn open_day(&mut self, date: &str) {
        self.date = date.to_string();
        self.file = None;
        self.bytes = 0;
        self.full_noted = false;
        let Some(dir) = self.dir.clone() else { return };
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(dir.join(day_name(date))) {
            self.bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
            self.file = Some(f);
        }
        self.prune();
    }

    /// Old days and the files of 0.4.3 and earlier go after KEEP_DAYS
    fn prune(&mut self) {
        let Some(dir) = self.dir.clone() else { return };
        let mut files = vec![];
        let limit = std::time::SystemTime::now() - Duration::from_secs(KEEP_DAYS as u64 * 24 * 3600);
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(d) = day_of(&name) {
                files.push((d.to_string(), e.metadata().map(|m| m.len()).unwrap_or(0)));
            } else if LEGACY_FILES.contains(&name.as_str()) {
                let old = e.metadata().and_then(|m| m.modified()).map(|m| m < limit).unwrap_or(false);
                if old {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        for date in to_delete(&files, &self.date, KEEP_DAYS, TOTAL_LIMIT) {
            let _ = fs::remove_file(dir.join(day_name(&date)));
        }
        prune_marks(&dir);
    }

    fn to_file(&mut self, line: &str, level: Level) {
        if self.bytes >= DAY_LIMIT && level > Level::Warn {
            if !self.full_noted {
                self.full_noted = true;
                let note = format!("[{} WARN  app_lib::logfile] LOG-FULL today's file passed 20 MB: info lines left out until tomorrow\n", &line[1..21.min(line.len())]);
                self.append(&note);
            }
            return;
        }
        self.append(line);
    }

    fn append(&mut self, line: &str) {
        if let Some(f) = self.file.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                self.bytes += line.len() as u64;
            }
        }
        self.tee(line);
    }

    /// The line also goes to the journal of each recording being worked on
    fn tee(&mut self, line: &str) {
        if self.meetings.is_empty() {
            return;
        }
        let now = Instant::now();
        self.meetings.retain(|m| m.active(now));
        for m in self.meetings.iter_mut() {
            if m.bytes >= MEETING_LIMIT {
                continue;
            }
            if let Some(f) = m.file.as_mut() {
                if f.write_all(line.as_bytes()).is_ok() {
                    m.bytes += line.len() as u64;
                }
            }
        }
    }

    fn keep_in_box(&mut self, line: String) {
        if self.black_box.len() >= BLACK_BOX_LINES {
            self.black_box.pop_front();
        }
        self.black_box.push_back(line);
    }

    fn write_box(&mut self, reason: &str, stamp: &str) {
        if self.black_box.is_empty() {
            return;
        }
        let lines: Vec<String> = self.black_box.drain(..).collect();
        let mut text = format!("[{stamp} INFO  app_lib::logfile] BLACK-BOX {} line(s) kept in memory before this {reason}:\n", lines.len());
        for l in lines {
            text.push_str("    ");
            text.push_str(&l);
        }
        text.push_str(&format!("[{stamp} INFO  app_lib::logfile] BLACK-BOX end\n"));
        self.append(&text);
        self.box_written = Some(Instant::now());
    }

    fn write(&mut self, level: Level, target: &str, line: String) {
        let date = line.get(1..11).unwrap_or("").to_string();
        if date != self.date && date.len() == 10 {
            self.open_day(&date);
        }
        let stamp = line.get(1..21).unwrap_or("").to_string();
        let now = Instant::now();
        if level > Level::Info {
            let message = line.split_once("] ").map(|(_, m)| m).unwrap_or(&line);
            if self.debug_repeats.check(repeat_key(target, message), "", now) != Gate::Skip {
                self.keep_in_box(line);
            }
            return;
        }
        for (skipped, secs, sample) in self.repeats.sweep(now) {
            self.append(&repeat_line(&stamp, skipped, secs, &sample));
        }
        let message = line.split_once("] ").map(|(_, m)| m).unwrap_or(&line);
        match self.repeats.check(repeat_key(target, message), message.trim_end(), now) {
            Gate::Skip => {
                self.keep_in_box(line);
                return;
            }
            Gate::WriteAfter { skipped, secs, sample } => self.append(&repeat_line(&stamp, skipped, secs, &sample)),
            Gate::Write => {}
        }
        if level == Level::Error && self.box_written.map_or(true, |t| now.duration_since(t) >= ERROR_BOX_EVERY) {
            self.write_box("error", &stamp);
        }
        self.to_file(&line, level);
    }
}

struct Logger;

fn ours(target: &str) -> bool {
    target.starts_with("app_lib") || target.starts_with("protokol")
}

impl Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= Level::Info || (m.level() == Level::Debug && ours(m.target()))
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if BUSY.with(|b| b.replace(true)) {
            return;
        }
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let line = format!("[{stamp} {:<5} {}] {}\n", record.level(), record.target(), record.args());
        if record.level() <= Level::Info {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
        if !private(&line) {
            let line = scrub(&line);
            lock(&STATE).write(record.level(), record.target(), line);
        }
        BUSY.with(|b| b.set(false));
    }

    fn flush(&self) {
        if let Some(f) = lock(&STATE).file.as_mut() {
            let _ = f.flush();
        }
        let _ = std::io::stderr().flush();
    }
}

static LOGGER: Logger = Logger;

/// The black box out to the file now (a mark, a report, a panic)
pub fn write_black_box(reason: &str) {
    if BUSY.with(|b| b.get()) {
        return;
    }
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    lock(&STATE).write_box(reason, &stamp);
}

// ===== A recording's own journal =====

fn journal_mut<'a>(st: &'a mut State, folder: &Path) -> Option<&'a mut MeetingJournal> {
    let now = Instant::now();
    st.meetings.retain(|m| m.active(now) || m.folder == folder);
    if let Some(i) = st.meetings.iter().position(|m| m.folder == folder) {
        return st.meetings.get_mut(i);
    }
    let path = folder.join(MEETING_FILE);
    let file = OpenOptions::new().create(true).append(true).open(&path).ok();
    let bytes = file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
    let mut journal = MeetingJournal { folder: folder.to_path_buf(), file, bytes, holders: 0, recording: false, until: None };
    if let Some(f) = journal.file.as_mut() {
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let head = format!("[{stamp} INFO  app_lib::logfile] JOURNAL Протокол {} · this recording's lines from here\n", env!("CARGO_PKG_VERSION"));
        if f.write_all(head.as_bytes()).is_ok() {
            journal.bytes += head.len() as u64;
        }
    }
    st.meetings.push(journal);
    st.meetings.last_mut()
}

/// A recording starts: its lines go to its own journal until an hour after it stops
pub fn recording_started(folder: &Path) {
    set_recordings_folder(&crate::audio::recording_preferences::recordings_folder());
    {
        let mut st = lock(&STATE);
        // An earlier recording's hour after its stop ends with a new recording
        for m in st.meetings.iter_mut() {
            m.recording = false;
            if m.holders == 0 {
                m.until = None;
            }
        }
        if let Some(m) = journal_mut(&mut st, folder) {
            m.recording = true;
            m.until = None;
        }
    }
    log::info!("REC-START recording folder ready");
}

/// The recording stopped: its journal still takes the lines of the next hour
pub fn recording_stopped(folder: &Path) {
    log::info!("REC-STOP saving the recording");
    let mut st = lock(&STATE);
    if let Some(m) = st.meetings.iter_mut().find(|m| m.folder == folder) {
        m.recording = false;
        m.until = Some(Instant::now() + AFTER_RECORDING);
    }
}

/// While it lives, the app's lines also go to this recording's journal
pub struct MeetingScope(PathBuf);

impl Drop for MeetingScope {
    fn drop(&mut self) {
        let mut st = lock(&STATE);
        if let Some(m) = st.meetings.iter_mut().find(|m| m.folder == self.0) {
            m.holders = m.holders.saturating_sub(1);
        }
    }
}

pub fn meeting_scope(folder: &Path) -> MeetingScope {
    if folder.is_dir() {
        let mut st = lock(&STATE);
        if let Some(m) = journal_mut(&mut st, folder) {
            m.holders += 1;
        }
    }
    MeetingScope(folder.to_path_buf())
}

/// The scope of a meeting known by its id (its folder from the database)
pub async fn meeting_scope_by_id(pool: &sqlx::SqlitePool, meeting_id: &str) -> Option<MeetingScope> {
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?").bind(meeting_id).fetch_optional(pool).await.ok().flatten().flatten();
    let folder = PathBuf::from(folder.filter(|f| !f.is_empty())?);
    Some(meeting_scope(&folder))
}

// ===== «Позначити проблему» =====

/// A moment the person marked as «something went wrong here»
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mark {
    /// The mark's number in the journal (USER-MARK #n)
    pub n: u32,
    /// When, UTC (RFC 3339)
    pub t: String,
    /// What the person wrote, if anything
    #[serde(default)]
    pub note: Option<String>,
    /// Where it was made: menu, tray, island, meet
    #[serde(default)]
    pub source: String,
}

/// «2026-11-21T10:42:00Z», as the marks are written and compared
fn stamp_of(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn read_marks(dir: &Path) -> Vec<Mark> {
    fs::read_to_string(dir.join(MARKS_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Mark>(l).ok())
        .collect()
}

fn prune_marks(dir: &Path) {
    let all = read_marks(dir);
    let since = stamp_of(chrono::Utc::now() - chrono::Duration::days(KEEP_DAYS));
    let kept: Vec<&Mark> = all.iter().filter(|m| m.t >= since).collect();
    if kept.len() == all.len() {
        return;
    }
    let text: String = kept.iter().filter_map(|m| serde_json::to_string(m).ok()).map(|l| l + "\n").collect();
    let _ = fs::write(dir.join(MARKS_FILE), text);
}

/// The marks of the last `days` days, oldest first
pub fn marks_since(days: i64) -> Vec<Mark> {
    let Some(dir) = log_dir() else { return vec![] };
    let since = stamp_of(chrono::Utc::now() - chrono::Duration::days(days));
    read_marks(&dir).into_iter().filter(|m| m.t >= since).collect()
}

/// «Позначити проблему»: the moment goes to the journal with the black box
/// before it. The note (the person's own words) stays in `marks.jsonl` and
/// shows only in a problem report they look through first.
pub fn mark(note: Option<&str>, source: &str) -> Mark {
    let note = note.map(|n| n.trim().chars().take(300).collect::<String>()).filter(|n| !n.is_empty());
    let n = log_dir().map(|d| read_marks(&d).iter().map(|m| m.n).max().unwrap_or(0) + 1).unwrap_or(1);
    let mark = Mark { n, t: stamp_of(chrono::Utc::now()), note, source: source.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(16).collect() };
    if let Some(dir) = log_dir() {
        let _ = fs::create_dir_all(&dir);
        if let (Ok(mut f), Ok(line)) = (OpenOptions::new().create(true).append(true).open(dir.join(MARKS_FILE)), serde_json::to_string(&mark)) {
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    }
    write_black_box(&format!("mark #{n}"));
    log::warn!("USER-MARK #{n} from {}{}", mark.source, if mark.note.is_some() { " (with a note)" } else { "" });
    mark
}

// ===== Start, end, panics =====

static LAST_CRASH: Mutex<Option<String>> = Mutex::new(None);

/// The last run ended with an error (once: the start page asks and offers a report)
pub fn last_crash() -> Option<String> {
    lock(&LAST_CRASH).take()
}

/// A report of macOS about a crash of this app (not of a helper), written after `since`
fn crash_report_since(since: std::time::SystemTime) -> bool {
    let Some(dir) = dirs::home_dir().map(|h| h.join("Library/Logs/DiagnosticReports")) else { return false };
    fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".ips") || !e.metadata().and_then(|m| m.modified()).map(|m| m > since).unwrap_or(false) {
            return false;
        }
        let head = fs::read_to_string(e.path()).ok().and_then(|t| t.lines().next().map(str::to_string)).unwrap_or_default();
        head.contains(&format!("\"bundleID\":\"{}\"", crate::identity::IDENTIFIER)) || head.contains(&format!("\"bundleID\" : \"{}\"", crate::identity::IDENTIFIER))
    })
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "?".to_string());
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let trace = trace.lines().take(80).collect::<Vec<_>>().join("\n");
        if let Some(dir) = log_dir() {
            let _ = fs::write(dir.join(PANICKED_FILE), chrono::Utc::now().to_rfc3339());
        }
        write_black_box("panic");
        log::error!(target: "app_lib::panic", "PANIC in thread '{thread}' at {place}: {message}\n{trace}");
        previous(info);
    }));
}

/// The journal, before anything else runs
pub fn init() {
    let dir = log_dir();
    let mut crashed = false;
    if let Some(dir) = dir.as_ref() {
        let _ = fs::create_dir_all(dir);
        let running = dir.join(RUNNING_FILE);
        let panicked = dir.join(PANICKED_FILE);
        if let Ok(meta) = fs::metadata(&running) {
            // The last run did not close normally; an error, not a forced quit, if it left a trace
            let since = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            crashed = panicked.exists() || crash_report_since(since);
            if crashed {
                *lock(&LAST_CRASH) = Some(fs::read_to_string(&running).unwrap_or_default().trim().to_string());
            }
        }
        let _ = fs::remove_file(&panicked);
        let _ = fs::write(&running, chrono::Utc::now().to_rfc3339());
    }
    {
        let mut st = lock(&STATE);
        st.dir = dir;
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        st.open_day(&today);
    }
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Debug);
    }
    install_panic_hook();
    log::info!("START Протокол {}", env!("CARGO_PKG_VERSION"));
    if crashed {
        log::warn!("CRASH the last run ended with an error");
    }
}

/// The app closes normally (not a crash next time)
pub fn clean_exit() {
    log::info!("EXIT");
    if let Some(dir) = log_dir() {
        let _ = fs::remove_file(dir.join(RUNNING_FILE));
        let _ = fs::remove_file(dir.join(PANICKED_FILE));
    }
}

/// Settings → Конфіденційність → «Очистити…»: the day files emptied (today's
/// is kept open and empty), older ones, marks and the black box gone. The
/// recordings' own journals are deleted by the caller.
pub fn clear() {
    let mut st = lock(&STATE);
    st.black_box.clear();
    let Some(dir) = st.dir.clone() else { return };
    let today = day_name(&st.date);
    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == today {
            if let Ok(f) = OpenOptions::new().write(true).open(e.path()) {
                let _ = f.set_len(0);
            }
        } else if day_of(&name).is_some() || LEGACY_FILES.contains(&name.as_str()) || name == MARKS_FILE {
            let _ = fs::remove_file(e.path());
        }
    }
    st.bytes = 0;
    st.full_noted = false;
}

/// The recordings' own journals, deleted (with «Очистити…»)
pub async fn clear_meeting_journals(pool: &sqlx::SqlitePool) -> usize {
    let folders: Vec<Option<String>> = sqlx::query_scalar("SELECT folder_path FROM meetings").fetch_all(pool).await.unwrap_or_default();
    let mut n = 0;
    for folder in folders.into_iter().flatten() {
        if fs::remove_file(Path::new(&folder).join(MEETING_FILE)).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_said_stays_out_of_the_file() {
        assert!(super::private("[t INFO  app_lib::audio::transcription::worker] 🔍 Worker 1 transcription result: text='Доброго ранку'"));
        assert!(super::private("[t INFO  app_lib::other] ✅ Worker 1 transcribed: Доброго ранку"));
        assert!(!super::private("[t INFO  app_lib::speakers::commands] Speakers recognised: 7 voice(s), 25 piece(s), 118.1s"));
        assert!(!super::private("[t INFO  app_lib::api::api] api_save_transcript called for meeting: Нарада 01.02.2026 10:00, transcripts: 292"));
    }

    #[test]
    fn titles_folders_and_user_name_stay_out_of_the_file() {
        use super::scrub;
        assert_eq!(
            scrub("[t INFO  app_lib::api::api] api_save_transcript called for meeting: Нарада 01.02.2026 10:00, transcripts: 292, folder_path: Some(\"/Users/olena/Movies/Protokol/Нарада 01.02.2026 10:00 (3)\"), auth_token: false"),
            "[t INFO  app_lib::api::api] api_save_transcript called for meeting: …, transcripts: 292, folder_path: Some(\"~/Movies/Protokol/…\"), auth_token: false"
        );
        assert_eq!(scrub("Fetched meeting title: Планування продажів"), "Fetched meeting title: …");
        assert_eq!(scrub("Summary status for meeting-1a2b: pending, has_data: true, meeting_name: Some(\"Планування\")"), "Summary status for meeting-1a2b: pending, has_data: true, meeting_name: Some(\"…\")");
        assert_eq!(scrub("🚀 CALLED start_recording - Mic: None, System: None, Meeting: Some(\"Планування\")"), "🚀 CALLED start_recording - Mic: None, System: None, Meeting: Some(\"…\")");
        assert_eq!(scrub("✅ Incremental audio saver initialized for meeting: Планування"), "✅ Incremental audio saver initialized for meeting: …");
        assert_eq!(scrub("Decoding audio file: /Users/olena/Movies/Protokol/Планування (14)/system.m4a"), "Decoding audio file: ~/Movies/Protokol/…/system.m4a");
        assert_eq!(scrub("Showing notification: Документ готовий: Планування"), "Showing notification: …");
        assert_eq!(scrub("3 learned example(s) for «Планування (щотижня)»"), "3 learned example(s) for «…»");
        assert_eq!(scrub("Recording saved to: /Users/olena/Movies/Protokol (перевірка Meet)/Планування (2)/audio.mp4\n"), "Recording saved to: ~/Movies/Protokol (перевірка Meet)/…/audio.mp4\n");
        // Ids, numbers and errors stay
        assert_eq!(scrub("Speakers recognised: 7 voice(s), 25 piece(s), 118.1s"), "Speakers recognised: 7 voice(s), 25 piece(s), 118.1s");
        assert_eq!(scrub("api_get_summary called for meeting: meeting-92df, x"), "api_get_summary called for meeting: meeting-92df, x");
        assert_eq!(scrub("Не вдалося відкрити папку у Finder"), "Не вдалося відкрити папку у Finder");
    }

    #[test]
    fn device_names_become_their_kind() {
        let a = scrub("[t INFO  app_lib::audio::device_detection] 🔍 Detecting device type for: 'AirPods Тест - Find My'");
        assert!(!a.contains("Тест"), "{a}");
        assert!(a.contains("[пристрій Bluetooth "), "{a}");
        // Known from then on, quoted or not
        let b = scrub("[t INFO  app_lib::audio::stream] Stopping audio stream for device: AirPods Тест - Find My");
        assert!(!b.contains("Тест"), "{b}");
        assert_eq!(a.split("for: ").nth(1).map(|x| x.trim_matches('\'')), b.split("device: ").nth(1));
        let c = scrub("[t INFO  x] ✅ macOS Core Audio: Bluetooth detected for 'Buds2 Pro користувача Тест'");
        assert!(!c.contains("Тест"), "{c}");
        hide_device_names(["MacBook Pro Microphone".to_string()]);
        assert!(scrub("using MacBook Pro Microphone now").contains("[пристрій вбудований "));
        assert!(device_label("BlackHole 2ch").contains("віртуальний"));
        // Generic words are not taken for names
        hide_device_names(["default".to_string()]);
        assert_eq!(scrub("the default one"), "the default one");
        // A quoted value of an audio module's line that is not a device stays
        let d = scrub("[t INFO  app_lib::audio::recording_saver] Saving format 'mp4-final' to disk");
        assert!(d.contains("'mp4-final'"), "{d}");
        assert_eq!(scrub("[t INFO  x] mp4-final again"), "[t INFO  x] mp4-final again");
    }

    #[test]
    fn a_chosen_recordings_folder_hides_its_meeting_folders_too() {
        set_recordings_folder(Path::new("/Volumes/Data/Наради"));
        assert_eq!(scrub("Recording saved to: /Volumes/Data/Наради/Склад 03.11 (2)/audio.mp4"), "Recording saved to: /Volumes/Data/Наради/…/audio.mp4");
        set_recordings_folder(Path::new("/Users/olena/Movies/Protokol"));
        assert!(lock(&RECORDINGS).is_none());
    }

    #[test]
    fn the_same_line_with_other_numbers_is_counted_not_written() {
        let a = repeat_key("app_lib::audio::vad", "VAD: Accumulated speech buffer is large: 1000320 samples (62.5s) - possible memory issue");
        let b = repeat_key("app_lib::audio::vad", "VAD: Accumulated speech buffer is large: 1001280 samples (62.6s) - possible memory issue");
        let c = repeat_key("app_lib::audio::vad", "VAD: speech started");
        let d = repeat_key("app_lib::other", "VAD: Accumulated speech buffer is large: 1000320 samples (62.5s) - possible memory issue");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);

        let mut r = Repeats::default();
        let t0 = Instant::now();
        for i in 0..REPEAT_KEEP {
            assert_eq!(r.check(a, "x", t0 + Duration::from_millis(i as u64)), Gate::Write);
        }
        for i in 0..1200u64 {
            assert_eq!(r.check(a, "x 1", t0 + Duration::from_millis(10 + i)), Gate::Skip);
        }
        // Another line is not held back by it
        assert_eq!(r.check(c, "y", t0 + Duration::from_secs(2)), Gate::Write);
        // A minute later: written again, after the count of the ones left out
        match r.check(a, "x", t0 + Duration::from_secs(61)) {
            Gate::WriteAfter { skipped, secs, sample } => {
                assert_eq!(skipped, 1200);
                assert_eq!(secs, 61);
                assert_eq!(sample, "x 1");
            }
            g => panic!("{g:?}"),
        }
        // A line that stopped coming is counted by the sweep
        for i in 0..20u64 {
            r.check(c, "y", t0 + Duration::from_secs(62) + Duration::from_millis(i));
        }
        let swept = r.sweep(t0 + Duration::from_secs(200));
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].0, 20 - REPEAT_KEEP);
    }

    #[test]
    fn days_past_a_week_or_the_size_limit_are_deleted() {
        let mb = 1024 * 1024;
        let files: Vec<(String, u64)> = [
            ("2026-11-21", 3 * mb),
            ("2026-11-20", 3 * mb),
            ("2026-11-15", mb),
            ("2026-11-14", mb),
            ("2026-11-13", mb),
        ]
        .iter()
        .map(|(d, s)| (d.to_string(), *s))
        .collect();
        assert_eq!(to_delete(&files, "2026-11-21", 7, 50 * mb), vec!["2026-11-14".to_string(), "2026-11-13".to_string()]);
        // Too much in all: the oldest go first, today stays whatever its size
        let big: Vec<(String, u64)> = vec![("2026-11-21".into(), 60 * mb), ("2026-11-20".into(), mb)];
        assert_eq!(to_delete(&big, "2026-11-21", 7, 50 * mb), vec!["2026-11-20".to_string()]);
        assert_eq!(day_of("protokol-2026-11-21.log"), Some("2026-11-21"));
        assert_eq!(day_of("protokol.log"), None);
        assert_eq!(day_of("protokol-x.log"), None);
    }

    #[test]
    fn a_recording_journal_takes_lines_while_its_work_runs() {
        let dir = std::env::temp_dir().join(format!("protokol-journal-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut st = State::default();
        {
            let m = journal_mut(&mut st, &dir).unwrap();
            m.holders = 1;
        }
        st.append("[2026-11-21T10:00:00Z INFO  app_lib::x] one\n");
        st.meetings[0].holders = 0;
        st.append("[2026-11-21T10:00:01Z INFO  app_lib::x] two\n");
        let text = fs::read_to_string(dir.join(MEETING_FILE)).unwrap();
        assert!(text.contains("JOURNAL"), "{text}");
        assert!(text.contains("one"), "{text}");
        assert!(!text.contains("two"), "{text}");
        // An hour after a stop
        st.meetings.clear();
        {
            let m = journal_mut(&mut st, &dir).unwrap();
            m.until = Some(Instant::now() + Duration::from_secs(5));
        }
        st.append("[2026-11-21T10:00:02Z INFO  app_lib::x] three\n");
        assert!(fs::read_to_string(dir.join(MEETING_FILE)).unwrap().contains("three"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_black_box_comes_out_once_with_its_lines_indented() {
        let dir = std::env::temp_dir().join(format!("protokol-box-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut st = State { dir: Some(dir.clone()), ..Default::default() };
        st.open_day("2026-11-21");
        st.write(Level::Debug, "app_lib::x", "[2026-11-21T10:00:00Z DEBUG app_lib::x] detail\n".into());
        st.write(Level::Info, "app_lib::x", "[2026-11-21T10:00:01Z INFO  app_lib::x] fine\n".into());
        let text = fs::read_to_string(dir.join(day_name("2026-11-21"))).unwrap();
        assert!(!text.contains("detail"), "{text}");
        st.write(Level::Error, "app_lib::x", "[2026-11-21T10:00:02Z ERROR app_lib::x] broke\n".into());
        let text = fs::read_to_string(dir.join(day_name("2026-11-21"))).unwrap();
        let box_at = text.find("BLACK-BOX 1 line(s)").unwrap();
        assert!(text.contains("    [2026-11-21T10:00:00Z DEBUG app_lib::x] detail"), "{text}");
        assert!(box_at < text.find("broke").unwrap(), "{text}");
        assert!(st.black_box.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_busy_debug_line_does_not_fill_the_black_box() {
        let mut st = State::default();
        for i in 0..50 {
            st.write(Level::Debug, "app_lib::audio::x", format!("[2026-11-21T10:00:00Z DEBUG app_lib::audio::x] chunk {i} done\n"));
        }
        st.write(Level::Debug, "app_lib::x", "[2026-11-21T10:00:01Z DEBUG app_lib::x] something else\n".into());
        assert_eq!(st.black_box.len(), REPEAT_KEEP as usize + 1);
    }
}
