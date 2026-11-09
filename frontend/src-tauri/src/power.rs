//! macOS Low Power Mode: it throttles the GPU, so local protocols take
//! two to three times longer. The UI warns about it.

/// Whether macOS Low Power Mode is on right now.
#[tauri::command]
pub fn api_is_low_power_mode() -> bool {
    #[cfg(target_os = "macos")]
    {
        // `pmset -g` prints "lowpowermode 1" while the mode is active
        std::process::Command::new("/usr/bin/pmset")
            .arg("-g")
            .output()
            .map(|out| parse_low_power(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Opens System Settings → Battery, where Low Power Mode is switched off.
#[tauri::command]
pub fn api_open_battery_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.Battery-Settings.extension")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Not supported on this platform".to_string())
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_low_power(pmset: &str) -> bool {
    pmset.lines().any(|line| {
        let mut parts = line.split_whitespace();
        parts.next() == Some("lowpowermode") && parts.next() == Some("1")
    })
}

#[cfg(test)]
mod tests {
    use super::parse_low_power;

    #[test]
    fn reads_pmset_output() {
        assert!(parse_low_power(" sleep 1\n lowpowermode         1\n"));
        assert!(!parse_low_power(" lowpowermode         0\n"));
        assert!(!parse_low_power(" sleep 1\n"));
    }
}
