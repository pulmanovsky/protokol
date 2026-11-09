//! The app's log, also in a file: ~/Library/Logs/ua.protokol.app/protokol.log
//! (info level; the previous run's file kept as protokol.1.log once it passes
//! 5 MB; writing stops at 20 MB). Lines that could hold what was said in a
//! meeting (recognised text from the speech modules) go to the terminal only.

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

struct Tee {
    file: Option<Mutex<(std::fs::File, u64)>>,
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(file) = &self.file {
            if !private(&String::from_utf8_lossy(buf)) {
                if let Ok(mut f) = file.lock() {
                    if f.1 < STOP_AT {
                        let _ = f.0.write_all(buf);
                        f.1 += buf.len() as u64;
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
}
