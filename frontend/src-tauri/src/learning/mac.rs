//! Facts about this Mac that decide whether a run may start: the chip, its
//! memory, free disk, power and how long nobody has touched it. Each is read
//! from a system tool, the way `feedback` does; the parsing is tested here.

use std::path::Path;
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Apple Silicon: the trainer needs its GPU and shared memory
pub fn apple_silicon() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// Memory of the Mac (GB), 0 when unknown
pub fn memory_gb() -> f64 {
    run("/usr/sbin/sysctl", &["-n", "hw.memsize"])
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|bytes| bytes / (1024.0 * 1024.0 * 1024.0))
        .unwrap_or(0.0)
}

/// Free space (GB) from `df -k <path>`: the fourth column of the second line
pub fn parse_df_free_gb(text: &str) -> Option<f64> {
    let kb: f64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb / (1024.0 * 1024.0))
}

/// Free space (GB) on the disk that holds `path`, 0 when unknown
pub fn free_disk_gb(path: &Path) -> f64 {
    run("/bin/df", &["-k", &path.to_string_lossy()]).and_then(|t| parse_df_free_gb(&t)).unwrap_or(0.0)
}

/// `pmset -g batt` says «Now drawing from 'AC Power'» or «'Battery Power'»;
/// a Mac without a battery (Mac mini) is always on power
pub fn parse_on_power(text: &str) -> bool {
    !text.contains("'Battery Power'")
}

pub fn on_power() -> bool {
    run("/usr/bin/pmset", &["-g", "batt"]).map(|t| parse_on_power(&t)).unwrap_or(true)
}

/// Minutes since the last keyboard or mouse input, from
/// `ioreg -c IOHIDSystem`: `"HIDIdleTime" = <nanoseconds>`
pub fn parse_idle_minutes(text: &str) -> Option<f64> {
    text.lines()
        .filter(|l| l.contains("\"HIDIdleTime\""))
        .filter_map(|l| l.rsplit('=').next()?.trim().parse::<f64>().ok())
        .reduce(f64::min)
        .map(|ns| ns / 1e9 / 60.0)
}

/// 0 when unknown, so an unknown Mac counts as in use
pub fn idle_minutes() -> f64 {
    run("/usr/sbin/ioreg", &["-c", "IOHIDSystem", "-d", "4"]).and_then(|t| parse_idle_minutes(&t)).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_disk_is_read_from_df() {
        let df = "Filesystem     1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n/dev/disk3s5    482797652 301230000 104857600    75% 1 2 0%   /System/Volumes/Data\n";
        assert_eq!(parse_df_free_gb(df), Some(100.0));
        assert_eq!(parse_df_free_gb("nothing"), None);
    }

    #[test]
    fn power_is_read_from_pmset() {
        assert!(parse_on_power("Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t100%; charged;"));
        assert!(!parse_on_power("Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t80%; discharging;"));
        assert!(parse_on_power("Now drawing from 'AC Power'\n"));
    }

    #[test]
    fn idle_time_is_read_from_ioreg() {
        let io = "    | |   \"HIDIdleTime\" = 120000000000\n    | |   \"HIDIdleTime\" = 900000000000\n";
        assert_eq!(parse_idle_minutes(io), Some(2.0));
        assert_eq!(parse_idle_minutes("no such key"), None);
    }
}
