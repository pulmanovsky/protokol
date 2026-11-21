//! The app's log, also in a file: ~/Library/Logs/ua.protokol.app/protokol.log
//! (info level; the previous run's file kept as protokol.1.log once it passes
//! 5 MB; writing stops at 20 MB). Lines that could hold what was said in a
//! meeting (recognised text from the speech modules) go to the terminal only.
//! Meeting titles, recording folders, notification texts and the Mac's user
//! name are taken out of the lines written to the file (see [`scrub`]).

use std::io::Write;
use std::sync::Mutex;

const ROTATE_AT: u64 = 5 * 1024 * 1024;
const STOP_AT: u64 = 20 * 1024 * 1024;

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

/// A line as written to the file: no meeting titles, folders or user name.
/// «for meeting: …» keeps a meeting id («meeting-…») and hides a title.
pub(crate) fn scrub(line: &str) -> String {
    static FOR_MEETING: once_cell::sync::Lazy<Option<regex::Regex>> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)(for meeting: )([^,\n]*)").ok());
    const MARKS: &[&str] = &["/Users/", "Movies/Protokol", "eeting", "otification", "«"];
    if !MARKS.iter().any(|m| line.contains(m)) {
        return line.to_string();
    }
    let mut s = line.to_string();
    for (re, to) in SCRUB.iter() {
        s = re.replace_all(&s, *to).into_owned();
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

struct Tee {
    file: Option<Mutex<(std::fs::File, u64)>>,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(file) = &self.file {
            let line = String::from_utf8_lossy(buf);
            if !private(&line) {
                let line = scrub(&line);
                if let Ok(mut f) = file.lock() {
                    if f.1 < STOP_AT {
                        let _ = f.0.write_all(line.as_bytes());
                        f.1 += line.len() as u64;
                    }
                }
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.0.flush();
            }
        }
        std::io::stderr().flush()
    }
}

fn open_file() -> Option<(std::fs::File, u64)> {
    let dir = dirs::home_dir()?.join("Library/Logs").join(crate::identity::IDENTIFIER);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("protokol.log");
    if std::fs::metadata(&path).map(|m| m.len() > ROTATE_AT).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("protokol.1.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    Some((file, len))
}

/// Logging to the terminal and the log file, info level
pub fn init() {
    let tee = Tee { file: open_file().map(Mutex::new) };
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(tee)))
        .try_init();
}

#[cfg(test)]
mod tests {
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
}
