//! Runs the trainer for one job and keeps an eye on it.
//!
//! The trainer gets the path of `job.json` as its only argument. While a
//! recording is on it is paused (SIGSTOP) and resumed after (SIGCONT), so
//! recording always has the Mac to itself. [`super::watch`] stops it when
//! the Mac struggles; the user can stop it too.
//!
//! Its stdin stays open while the app is there: the trainer stops by itself
//! when stdin closes. Its process id is written next to `job.json`, so a
//! trainer left over after a crash is found and stopped at the next start.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::job::{parse_line, Progress};
use super::watch::{Stop, Watch};

/// How a trainer run ended
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "end", rename_all = "snake_case")]
pub enum End {
    Done { minutes: f64, peak_memory_gb: f64 },
    Watched { why: Stop },
    StoppedByUser,
    Failed { reason: String },
}

/// Swap in use (GB) from `sysctl vm.swapusage`: `total = 2048.00M  used = 1234.50M  free = ...`
pub fn parse_swap_used_gb(text: &str) -> Option<f64> {
    let after = text.split("used =").nth(1)?.trim_start();
    let num: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let value: f64 = num.parse().ok()?;
    let unit = after[num.len()..].chars().next()?;
    Some(match unit {
        'G' => value,
        'M' => value / 1024.0,
        'K' => value / (1024.0 * 1024.0),
        _ => return None,
    })
}

pub fn swap_used_gb() -> f64 {
    std::process::Command::new("/usr/sbin/sysctl")
        .args(["vm.swapusage"])
        .output()
        .ok()
        .and_then(|o| parse_swap_used_gb(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(0.0)
}

fn signal(pid: u32, sig: &str) {
    let _ = std::process::Command::new("/bin/kill").args([sig, &pid.to_string()]).status();
}

/// Next to `job.json`: the trainer's process id
pub const PID_FILE: &str = "trainer.pid";

/// Stops trainers left over in `work_root` (the app quit or crashed during a
/// run): a process is stopped only if it still runs this folder's job
pub fn stop_leftovers(work_root: &Path) {
    let Ok(dirs) = std::fs::read_dir(work_root) else { return };
    for dir in dirs.filter_map(|d| d.ok()).map(|d| d.path()) {
        let Some(pid) = std::fs::read_to_string(dir.join(PID_FILE)).ok().and_then(|s| s.trim().parse::<u32>().ok()) else {
            continue;
        };
        let args = std::process::Command::new("/bin/ps")
            .args(["-o", "args=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        if args.contains(&*dir.join("job.json").to_string_lossy()) {
            log::warn!("learning: stopping a trainer left from an earlier run");
            // A paused process only dies once resumed
            signal(pid, "-CONT");
            signal(pid, "-KILL");
        }
    }
}

/// The trainer program and the arguments before the job path
#[derive(Debug, Clone)]
pub struct Trainer {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Runs the trainer to the end. `recording` and `stop_asked` are polled
/// every two seconds; `progress` gets every progress line.
pub async fn run(
    trainer: &Trainer,
    job_path: &Path,
    mac_memory_gb: f64,
    recording: impl Fn() -> bool,
    stop_asked: impl Fn() -> bool,
    mut progress: impl FnMut(&Progress),
) -> Result<End> {
    let mut child = tokio::process::Command::new(&trainer.program)
        .args(&trainer.args)
        .arg(job_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("the trainer did not start: {e}"))?;
    let pid = child.id().ok_or_else(|| anyhow!("the trainer has no process id"))?;
    // Closed when this function returns or the app is gone
    let _stdin = child.stdin.take();
    if let Some(dir) = job_path.parent() {
        let _ = std::fs::write(dir.join(PID_FILE), pid.to_string());
    }
    let mut lines = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no trainer output"))?).lines();
    let mut watch = Watch::new(mac_memory_gb, swap_used_gb());
    let mut paused = false;
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let started = Instant::now();
    let mut end: Option<End> = None;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        let Some(p) = parse_line(&text) else { continue };
                        progress(&p);
                        match &p {
                            Progress::Step { seconds_per_piece, memory_gb, .. } => {
                                // A paused step is slow because of the pause, not the Mac
                                if !paused {
                                    if let Some(why) = watch.step(*seconds_per_piece, *memory_gb, swap_used_gb()) {
                                        end = Some(End::Watched { why });
                                        break;
                                    }
                                }
                            }
                            Progress::Done { minutes, peak_memory_gb } => end = Some(End::Done { minutes: *minutes, peak_memory_gb: *peak_memory_gb }),
                            Progress::Failed { reason } => end = Some(End::Failed { reason: reason.clone() }),
                            _ => {}
                        }
                    }
                    // Output closed: the trainer has finished one way or another
                    _ => break,
                }
            }
            _ = tick.tick() => {
                if stop_asked() {
                    end = Some(End::StoppedByUser);
                    break;
                }
                let rec = recording();
                if rec && !paused {
                    signal(pid, "-STOP");
                    paused = true;
                } else if !rec && paused {
                    signal(pid, "-CONT");
                    paused = false;
                }
            }
        }
    }
    let finished = matches!(end, Some(End::Done { .. }) | Some(End::Failed { .. }));
    if !finished {
        if paused {
            signal(pid, "-CONT");
        }
        let _ = child.kill().await;
    }
    let status = child.wait().await.ok();
    Ok(end.unwrap_or_else(|| End::Failed {
        reason: format!("the trainer ended without a result after {:.0} min ({status:?})", started.elapsed().as_secs_f64() / 60.0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_is_read_from_sysctl() {
        assert_eq!(parse_swap_used_gb("vm.swapusage: total = 2048.00M  used = 1024.00M  free = 1024.00M  (encrypted)"), Some(1.0));
        assert_eq!(parse_swap_used_gb("vm.swapusage: total = 0.00M  used = 0.00M  free = 0.00M"), Some(0.0));
        assert_eq!(parse_swap_used_gb("vm.swapusage: total = 8.00G  used = 5.50G  free = 2.50G"), Some(5.5));
        assert_eq!(parse_swap_used_gb("nothing"), None);
    }

    #[cfg(unix)]
    fn fake_trainer(name: &str, body: &str) -> Trainer {
        // Run by `sh`, not executed directly: a file just written can be
        // «busy» for exec while other tests fork
        let path = std::env::temp_dir().join(format!("protokol-fake-trainer-{name}-{}.sh", std::process::id()));
        std::fs::write(&path, format!("{body}\n")).unwrap();
        Trainer { program: "/bin/sh".into(), args: vec![path.to_string_lossy().into_owned()] }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_trainer_that_finishes_is_done() {
        let t = fake_trainer(
            "done",
            r#"echo 'loading'
echo '{"event":"started","pieces":2,"epochs":1}'
echo '{"event":"step","epoch":1,"step":1,"of":2,"loss":0.5,"seconds_per_piece":1.8,"memory_gb":9.0}'
echo '{"event":"done","minutes":0.1,"peak_memory_gb":9.0}'"#,
        );
        let mut seen = 0;
        let end = run(&t, Path::new("/tmp/job.json"), 24.0, || false, || false, |_| seen += 1).await.unwrap();
        assert_eq!(end, End::Done { minutes: 0.1, peak_memory_gb: 9.0 });
        assert_eq!(seen, 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_trainer_that_slows_down_is_stopped_and_killed() {
        let t = fake_trainer(
            "slow",
            r#"i=0; while [ $i -lt 20 ]; do echo '{"event":"step","epoch":1,"step":1,"of":99,"loss":0.5,"seconds_per_piece":1.8,"memory_gb":9.0}'; i=$((i+1)); done
i=0; while [ $i -lt 12 ]; do echo '{"event":"step","epoch":1,"step":2,"of":99,"loss":0.5,"seconds_per_piece":46.0,"memory_gb":14.0}'; i=$((i+1)); done
sleep 30"#,
        );
        let started = Instant::now();
        let end = run(&t, Path::new("/tmp/job.json"), 24.0, || false, || false, |_| {}).await.unwrap();
        assert_eq!(end, End::Watched { why: Stop::TooSlow });
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_trainer_that_dies_quietly_has_failed() {
        let t = fake_trainer("quiet", "exit 3");
        let end = run(&t, Path::new("/tmp/job.json"), 24.0, || false, || false, |_| {}).await.unwrap();
        assert!(matches!(end, End::Failed { .. }), "{end:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_trainer_left_from_a_crash_is_stopped() {
        let root = std::env::temp_dir().join(format!("protokol-leftover-{}", std::process::id()));
        let dir = root.join("2026-11-15T01-30");
        std::fs::create_dir_all(&dir).unwrap();
        let script = root.join("t.sh");
        std::fs::write(&script, "sleep 30\n").unwrap();
        let started = Instant::now();
        let mut child = std::process::Command::new("/bin/sh").arg(&script).arg(dir.join("job.json")).spawn().unwrap();
        std::fs::write(dir.join(PID_FILE), child.id().to_string()).unwrap();
        // A folder whose process runs something else is left alone
        let other = root.join("2026-11-14T01-30");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(PID_FILE), std::process::id().to_string()).unwrap();
        stop_leftovers(&root);
        assert!(!child.wait().unwrap().success());
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_user_can_stop_it() {
        let t = fake_trainer("stop", "sleep 30");
        let started = Instant::now();
        let end = run(&t, Path::new("/tmp/job.json"), 24.0, || false, || true, |_| {}).await.unwrap();
        assert_eq!(end, End::StoppedByUser);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
