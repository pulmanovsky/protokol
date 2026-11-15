//! When a training run may start.
//!
//! By default the app trains at night, after enough new speech has been
//! checked, when the Mac is on power and nobody is using it. A run started
//! by hand («Покращити розпізнавання зараз») skips the night and idle rules.
//! A Mac that cannot train at all (Intel, too little memory) is told so once
//! and never tries.

use serde::{Deserialize, Serialize};

/// New checked speech (hours) since the last run before the next one
pub const MIN_NEW_HOURS: f64 = 2.0;
/// Checked speech (hours) needed for a first run, also by hand
pub const MIN_TOTAL_HOURS: f64 = 2.0;
/// Night: a run starts between these hours, local time
pub const NIGHT_FROM: u32 = 1;
pub const NIGHT_TO: u32 = 6;
/// Minutes without keyboard or mouse before a night run starts
pub const IDLE_MINUTES: f64 = 15.0;
/// Memory of the Mac (GB) and free disk (GB) needed
pub const MIN_MEMORY_GB: f64 = 16.0;
pub const MIN_FREE_DISK_GB: f64 = 10.0;
/// After a run that was stopped or failed, wait this long before trying again (hours)
pub const RETRY_AFTER_HOURS: f64 = 20.0;
/// After «Не зараз» the offer comes back this many days later
pub const OFFER_AGAIN_DAYS: f64 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// At night by itself (asked once first)
    #[default]
    Nightly,
    /// Only with the button
    Manual,
    Off,
}

/// What the app knows right now
#[derive(Debug, Clone, PartialEq)]
pub struct Now {
    pub mode: Mode,
    /// The user pressed «Покращити розпізнавання зараз»
    pub by_hand: bool,
    pub apple_silicon: bool,
    pub memory_gb: f64,
    /// The trainer is installed
    pub engine_ready: bool,
    /// Recognition uses the model that is trained (`large-v3`)
    pub model_fits: bool,
    pub free_disk_gb: f64,
    /// All checked speech, and how much of it is new since the last run
    pub checked_hours: f64,
    pub new_hours: f64,
    pub local_hour: u32,
    pub on_power: bool,
    pub idle_minutes: f64,
    pub recording: bool,
    /// Hours since a run that was stopped or failed, if the last run was one
    pub since_unfinished_hours: Option<f64>,
}

/// Why a run does not start now
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wait {
    /// This Mac cannot train: not Apple Silicon, or too little memory
    NotThisMac,
    Off,
    /// The trainer is not installed yet
    NoEngine,
    /// Recognition uses another model (the fast one), which is not trained
    OtherModel,
    LowDisk,
    /// Mode «only by hand» and no button pressed
    ByHandOnly,
    NotEnoughSpeech,
    Recording,
    NotNight,
    OnBattery,
    InUse,
    /// A run was stopped or failed recently
    TriedRecently,
}

pub fn can_train_here(apple_silicon: bool, memory_gb: f64) -> bool {
    // 16 GB Macs report a little under 16 in some tools
    apple_silicon && memory_gb >= MIN_MEMORY_GB - 0.5
}

/// Whether to offer learning («Донавчити розпізнавання на ваших нарадах?»):
/// not answered yet, this Mac can train the model in use, enough speech is
/// checked, and «Не зараз» (if pressed) was long enough ago
pub fn offer(mode: Option<Mode>, can_train: bool, model_fits: bool, checked_hours: f64, later_days_ago: Option<f64>) -> bool {
    mode.is_none() && can_train && model_fits && checked_hours >= MIN_TOTAL_HOURS && later_days_ago.map_or(true, |d| d >= OFFER_AGAIN_DAYS)
}

/// `Ok` when a run may start now
pub fn check(n: &Now) -> Result<(), Wait> {
    if !can_train_here(n.apple_silicon, n.memory_gb) {
        return Err(Wait::NotThisMac);
    }
    if n.mode == Mode::Off {
        return Err(Wait::Off);
    }
    if !n.engine_ready {
        return Err(Wait::NoEngine);
    }
    if !n.model_fits {
        return Err(Wait::OtherModel);
    }
    if n.free_disk_gb < MIN_FREE_DISK_GB {
        return Err(Wait::LowDisk);
    }
    if n.recording {
        return Err(Wait::Recording);
    }
    if n.by_hand {
        return if n.checked_hours >= MIN_TOTAL_HOURS { Ok(()) } else { Err(Wait::NotEnoughSpeech) };
    }
    if n.mode == Mode::Manual {
        return Err(Wait::ByHandOnly);
    }
    if n.checked_hours < MIN_TOTAL_HOURS || n.new_hours < MIN_NEW_HOURS {
        return Err(Wait::NotEnoughSpeech);
    }
    if !(NIGHT_FROM..NIGHT_TO).contains(&n.local_hour) {
        return Err(Wait::NotNight);
    }
    if !n.on_power {
        return Err(Wait::OnBattery);
    }
    if n.idle_minutes < IDLE_MINUTES {
        return Err(Wait::InUse);
    }
    if n.since_unfinished_hours.is_some_and(|h| h < RETRY_AFTER_HOURS) {
        return Err(Wait::TriedRecently);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn night() -> Now {
        Now {
            mode: Mode::Nightly,
            by_hand: false,
            apple_silicon: true,
            memory_gb: 24.0,
            engine_ready: true,
            model_fits: true,
            free_disk_gb: 80.0,
            checked_hours: 5.0,
            new_hours: 2.5,
            local_hour: 2,
            on_power: true,
            idle_minutes: 60.0,
            recording: false,
            since_unfinished_hours: None,
        }
    }

    #[test]
    fn a_quiet_night_with_new_speech_starts_a_run() {
        assert_eq!(check(&night()), Ok(()));
    }

    #[test]
    fn each_rule_holds_a_night_run_back() {
        let cases: Vec<(Box<dyn Fn(&mut Now)>, Wait)> = vec![
            (Box::new(|n| n.apple_silicon = false), Wait::NotThisMac),
            (Box::new(|n| n.memory_gb = 8.0), Wait::NotThisMac),
            (Box::new(|n| n.mode = Mode::Off), Wait::Off),
            (Box::new(|n| n.engine_ready = false), Wait::NoEngine),
            (Box::new(|n| n.model_fits = false), Wait::OtherModel),
            (Box::new(|n| n.free_disk_gb = 4.0), Wait::LowDisk),
            (Box::new(|n| n.recording = true), Wait::Recording),
            (Box::new(|n| n.mode = Mode::Manual), Wait::ByHandOnly),
            (Box::new(|n| n.new_hours = 1.0), Wait::NotEnoughSpeech),
            (Box::new(|n| n.local_hour = 14), Wait::NotNight),
            (Box::new(|n| n.local_hour = 6), Wait::NotNight),
            (Box::new(|n| n.on_power = false), Wait::OnBattery),
            (Box::new(|n| n.idle_minutes = 3.0), Wait::InUse),
            (Box::new(|n| n.since_unfinished_hours = Some(4.0)), Wait::TriedRecently),
        ];
        for (change, want) in cases {
            let mut n = night();
            change(&mut n);
            assert_eq!(check(&n), Err(want));
        }
    }

    #[test]
    fn by_hand_skips_night_and_idle_but_not_recording_or_hardware() {
        let mut n = night();
        n.by_hand = true;
        n.mode = Mode::Manual;
        n.local_hour = 14;
        n.idle_minutes = 0.0;
        n.on_power = false;
        n.new_hours = 0.3;
        assert_eq!(check(&n), Ok(()));
        n.recording = true;
        assert_eq!(check(&n), Err(Wait::Recording));
        n.recording = false;
        n.checked_hours = 1.0;
        assert_eq!(check(&n), Err(Wait::NotEnoughSpeech));
        n.memory_gb = 8.0;
        assert_eq!(check(&n), Err(Wait::NotThisMac));
    }

    #[test]
    fn the_offer_comes_once_enough_is_checked_and_again_a_week_after_not_now() {
        assert!(offer(None, true, true, 2.0, None));
        assert!(!offer(None, true, true, 1.5, None), "too little checked speech");
        assert!(!offer(Some(Mode::Nightly), true, true, 5.0, None), "answered");
        assert!(!offer(Some(Mode::Off), true, true, 5.0, None), "answered");
        assert!(!offer(None, false, true, 5.0, None), "this Mac cannot train");
        assert!(!offer(None, true, false, 5.0, None), "another model in use");
        assert!(!offer(None, true, true, 5.0, Some(3.0)), "«Не зараз» three days ago");
        assert!(offer(None, true, true, 5.0, Some(7.5)));
    }

    #[test]
    fn a_16_gb_mac_counts_even_when_reported_a_little_lower() {
        assert!(can_train_here(true, 15.9));
        assert!(!can_train_here(true, 8.0));
    }
}
