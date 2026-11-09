//! Per-channel record of a recording, kept next to the mixed audio.
//!
//! The recording itself is one mixed track (microphone + system sound), so
//! afterwards nothing tells who was speaking locally and who came through the
//! call. While mixing, this module also keeps:
//! - `channels.json`: speech level of each channel every 100 ms, on the same
//!   timeline as the transcript (it is used to label the local speaker «Я»);
//! - `system.m4a`: the system channel alone at 16 kHz mono, for telling the
//!   remote participants apart by voice.
//! Both stay in the meeting folder on this Mac.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{info, warn};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

pub const CHANNELS_FILE: &str = "channels.json";
pub const SYSTEM_AUDIO_FILE: &str = "system.m4a";
/// Level resolution in the activity track
pub const STEP_MS: u32 = 100;
const OUT_RATE: u32 = 16_000;

/// Speech level per channel, one value per STEP_MS (RMS × 10 000)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChannelLevels {
    pub step_ms: u32,
    pub mic: Vec<u16>,
    pub system: Vec<u16>,
}

struct Track {
    sample_rate: u32,
    levels: ChannelLevels,
    pcm_path: PathBuf,
    pcm: Option<BufWriter<File>>,
    /// Samples of the current, not yet complete level step
    pending_mic: Vec<f32>,
    pending_sys: Vec<f32>,
    /// Resampler state for the system channel (fractional position)
    resample_pos: f64,
}

static TRACK: Lazy<Mutex<Option<Track>>> = Lazy::new(|| Mutex::new(None));

fn rms(samples: &[f32]) -> u16 {
    if samples.is_empty() {
        return 0;
    }
    let mean = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    (mean.sqrt() * 10_000.0).min(u16::MAX as f32) as u16
}

/// Starts a new track for a recording that is about to be mixed
pub fn start(sample_rate: u32) {
    let pcm_path = std::env::temp_dir().join(format!("protokol-system-{}.pcm", std::process::id()));
    let pcm = File::create(&pcm_path).map(BufWriter::new).map_err(|e| warn!("System channel copy disabled: {e}")).ok();
    let track = Track {
        sample_rate,
        levels: ChannelLevels { step_ms: STEP_MS, ..Default::default() },
        pcm_path,
        pcm,
        pending_mic: Vec::new(),
        pending_sys: Vec::new(),
        resample_pos: 0.0,
    };
    if let Ok(mut slot) = TRACK.lock() {
        *slot = Some(track);
    }
}

/// Adds one mixing window (both channels, same length, at the pipeline rate)
pub fn push(mic: &[f32], sys: &[f32]) {
    let Ok(mut slot) = TRACK.lock() else { return };
    let Some(track) = slot.as_mut() else { return };
    let step = (track.sample_rate * STEP_MS / 1000) as usize;
    track.pending_mic.extend_from_slice(mic);
    track.pending_sys.extend_from_slice(sys);
    while track.pending_mic.len() >= step && track.pending_sys.len() >= step {
        let m: Vec<f32> = track.pending_mic.drain(..step).collect();
        let s: Vec<f32> = track.pending_sys.drain(..step).collect();
        track.levels.mic.push(rms(&m));
        track.levels.system.push(rms(&s));
    }
    // System channel at 16 kHz: average over each output sample's span
    if let Some(pcm) = track.pcm.as_mut() {
        let ratio = track.sample_rate as f64 / OUT_RATE as f64;
        let mut bytes = Vec::with_capacity((sys.len() as f64 / ratio) as usize * 2 + 2);
        let mut pos = track.resample_pos;
        while pos + ratio <= sys.len() as f64 {
            let from = pos as usize;
            let to = ((pos + ratio) as usize).clamp(from + 1, sys.len());
            let avg = sys[from..to].iter().sum::<f32>() / (to - from) as f32;
            bytes.extend_from_slice(&((avg.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            pos += ratio;
        }
        track.resample_pos = pos - sys.len() as f64;
        if pcm.write_all(&bytes).is_err() {
            track.pcm = None;
        }
    }
}

/// Ends the track and writes `channels.json` and `system.m4a` into the meeting folder
pub fn finish(folder: &Path) {
    let Some(mut track) = TRACK.lock().ok().and_then(|mut slot| slot.take()) else { return };
    if track.levels.mic.is_empty() {
        let _ = std::fs::remove_file(&track.pcm_path);
        return;
    }
    match serde_json::to_vec(&track.levels) {
        Ok(json) => {
            if let Err(e) = std::fs::write(folder.join(CHANNELS_FILE), json) {
                warn!("Could not save channel levels: {e}");
            }
        }
        Err(e) => warn!("Could not encode channel levels: {e}"),
    }
    let has_system = track.levels.system.iter().any(|&l| l > 0);
    let pcm_ok = track.pcm.take().map(|mut p| p.flush().is_ok()).unwrap_or(false);
    if has_system && pcm_ok {
        if let Err(e) = encode_system_audio(&track.pcm_path, &folder.join(SYSTEM_AUDIO_FILE)) {
            warn!("Could not save the system channel: {e}");
        }
    }
    let _ = std::fs::remove_file(&track.pcm_path);
    info!("Saved per-channel track: {} steps", track.levels.mic.len());
}

fn encode_system_audio(pcm: &Path, out: &Path) -> Result<(), String> {
    let ffmpeg = super::ffmpeg::find_ffmpeg_path().ok_or("ffmpeg not found")?;
    let status = std::process::Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "s16le", "-ar", "16000", "-ac", "1", "-i"])
        .arg(pcm)
        .args(["-c:a", "aac", "-b:a", "48k"])
        .arg(out)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("ffmpeg exited with {status}"))
    }
}

/// Reads `channels.json` from a meeting folder
pub fn load_levels(folder: &Path) -> Option<ChannelLevels> {
    let text = std::fs::read(folder.join(CHANNELS_FILE)).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Which channel carried the speech between two moments (seconds).
/// Some(true) = microphone, Some(false) = system, None = unclear or no data.
pub fn dominant_channel(levels: &ChannelLevels, start_s: f64, end_s: f64) -> Option<bool> {
    let step = levels.step_ms as f64 / 1000.0;
    if step <= 0.0 {
        return None;
    }
    let from = (start_s / step).floor().max(0.0) as usize;
    let to = ((end_s / step).ceil() as usize).min(levels.mic.len()).min(levels.system.len());
    if from >= to {
        return None;
    }
    let mic: u64 = levels.mic[from..to].iter().map(|&v| v as u64 * v as u64).sum();
    let sys: u64 = levels.system[from..to].iter().map(|&v| v as u64 * v as u64).sum();
    // One channel must carry clearly more energy (×4 ≈ 6 dB)
    if mic > sys.saturating_mul(4) {
        Some(true)
    } else if sys > mic.saturating_mul(4) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_channel_by_energy() {
        let levels = ChannelLevels { step_ms: 100, mic: vec![0, 0, 900, 900, 10], system: vec![500, 600, 20, 30, 10] };
        assert_eq!(dominant_channel(&levels, 0.0, 0.2), Some(false));
        assert_eq!(dominant_channel(&levels, 0.2, 0.4), Some(true));
        assert_eq!(dominant_channel(&levels, 0.4, 0.5), None);
        assert_eq!(dominant_channel(&levels, 3.0, 4.0), None);
    }

    #[test]
    fn levels_and_pcm_are_collected() {
        start(48_000);
        let mic = vec![0.1f32; 28_800]; // 600 ms
        let sys = vec![0.0f32; 28_800];
        push(&mic, &sys);
        let slot = TRACK.lock().unwrap();
        let track = slot.as_ref().unwrap();
        assert_eq!(track.levels.mic.len(), 6);
        assert!(track.levels.mic.iter().all(|&l| (995..=1005).contains(&l)));
        assert!(track.levels.system.iter().all(|&l| l == 0));
        let path = track.pcm_path.clone();
        drop(slot);
        TRACK.lock().unwrap().take();
        let _ = std::fs::remove_file(path);
    }
}
