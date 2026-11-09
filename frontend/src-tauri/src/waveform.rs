//! The recording's shape for the simple timeline: the speech-band loudness
//! of the mix the player plays, 50 slices a second, cached in the meeting's
//! folder (waveform-v2.bin).
//! Built once (after processing, or the first time a meeting is opened), at
//! a low priority, never while recording.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Slices per second: enough to zoom down to a few seconds
pub const RATE: usize = 50;
/// A new format or formula means a new file: old caches are simply not read
const FILE: &str = "waveform-v2.bin";

/// The picture of the sound the player plays: one height per slice, 0 for
/// silence and 1–255 for sound
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Waveform {
    /// Seconds of sound
    pub duration: f64,
    /// Slices per second
    pub rate: usize,
    pub data: Vec<u8>,
}

/// A second-order filter (RBJ), run forward over the samples
fn biquad(x: &mut [f32], b: [f32; 3], a: [f32; 2]) {
    let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
    for s in x.iter_mut() {
        let v = *s;
        let y = b[0] * v + b[1] * x1 + b[2] * x2 - a[0] * y1 - a[1] * y2;
        (x2, x1, y2, y1) = (x1, v, y1, y);
        *s = y;
    }
}

/// What the ear takes as speech: 300–3400 Hz. Room rumble (often the loudest
/// thing in a quiet stretch once the level is normalised) stays out.
fn speech_band(samples: &[i16], rate: f64) -> Vec<f32> {
    use std::f32::consts::PI;
    let mut x: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w = 2.0 * PI * 300.0 / rate as f32;
    let (c, al) = (w.cos(), w.sin() / (2.0 * q));
    let a0 = 1.0 + al;
    biquad(&mut x, [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0], [-2.0 * c / a0, (1.0 - al) / a0]);
    let w = 2.0 * PI * 3400.0 / rate as f32;
    let (c, al) = (w.cos(), w.sin() / (2.0 * q));
    let a0 = 1.0 + al;
    biquad(&mut x, [(1.0 - c) / 2.0 / a0, (1.0 - c) / a0, (1.0 - c) / 2.0 / a0], [-2.0 * c / a0, (1.0 - al) / a0]);
    x
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

/// Heights for `samples` (16-bit, `rate` Hz). Linear loudness scaled to the
/// meeting's 99th percentile, gently compressed (√). Silence (0): below
/// −50 dBFS or the noise floor +6 dB, and steady sound that does not move
/// like speech (hum, fans) over a second.
pub fn compute(samples: &[i16], rate: f64) -> Waveform {
    let hop = (rate as usize / RATE).max(1);
    let band = speech_band(samples, rate);
    let rms: Vec<f32> = band.chunks(hop).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let mut audible: Vec<f32> = rms.iter().copied().filter(|&r| r > 1e-5).collect();
    audible.sort_by(|a, b| a.total_cmp(b));
    let floor = percentile(&audible, 0.10);
    let gate = (10f32.powf(-50.0 / 20.0)).max(floor * 2.0);
    let mut quiet: Vec<bool> = rms.iter().map(|&r| r < gate).collect();
    // A second that barely changes is not speech (std/mean: speech 0.7–1.1, hum about 0.2)
    let win = RATE;
    let mut i = 0;
    while i + win <= rms.len() {
        let w = &rms[i..i + win];
        let mean = w.iter().sum::<f32>() / win as f32;
        let var = w.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / win as f32;
        if mean > 0.0 && var.sqrt() / mean < STEADY {
            quiet[i..i + win].iter_mut().for_each(|q| *q = true);
        }
        i += win / 10;
    }
    // A click alone in the quiet (shorter than a syllable) is not someone speaking
    let min_run = RATE / 5;
    let mut i = 0;
    while i < quiet.len() {
        if quiet[i] {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < quiet.len() && !quiet[j] {
            j += 1;
        }
        let alone = |range: std::ops::Range<usize>| quiet[range].iter().all(|&q| q);
        if j - i < min_run && alone(i.saturating_sub(RATE / 3)..i) && alone(j..(j + RATE / 3).min(quiet.len())) {
            quiet[i..j].iter_mut().for_each(|q| *q = true);
        }
        i = j;
    }
    let mut loud: Vec<f32> = rms.iter().zip(&quiet).filter(|(_, q)| !**q).map(|(r, _)| *r).collect();
    loud.sort_by(|a, b| a.total_cmp(b));
    let top = percentile(&loud, 0.99).max(1e-5);
    let data = rms.iter().zip(&quiet).map(|(r, q)| if *q { 0 } else { (((r / top).min(1.0).sqrt() * 255.0).round() as u8).max(1) }).collect();
    Waveform { duration: samples.len() as f64 / rate, rate: RATE, data }
}

const STEADY: f32 = 0.35;

fn read_cache(path: &Path) -> Option<Waveform> {
    let b = std::fs::read(path).ok()?;
    let d = b.get(..8)?;
    Some(Waveform { duration: f64::from_le_bytes(d.try_into().ok()?), rate: RATE, data: b[8..].to_vec() })
}

/// The meeting's waveform, from the same file the player plays: from the
/// cache when it is newer than the sound, else built and kept
pub fn load_or_build(folder: &Path) -> anyhow::Result<Waveform> {
    let audio = crate::audio::retranscription::find_audio_file(folder).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cache = folder.join(FILE);
    let fresh = |c: &Path| -> bool {
        let (Ok(a), Ok(w)) = (std::fs::metadata(&audio).and_then(|m| m.modified()), std::fs::metadata(c).and_then(|m| m.modified())) else { return false };
        w >= a
    };
    if fresh(&cache) {
        if let Some(w) = read_cache(&cache) {
            return Ok(w);
        }
    }
    let started = std::time::Instant::now();
    let mix = crate::speakers::aligner::player_mix(&audio)?;
    let w = compute(&mix, 16_000.0);
    let mut bytes = w.duration.to_le_bytes().to_vec();
    bytes.extend_from_slice(&w.data);
    std::fs::write(&cache, bytes)?;
    log::info!("Waveform built: {:.0} s of sound in {:.1} s", w.duration, started.elapsed().as_secs_f64());
    Ok(w)
}

/// Builds it in the background (after processing); nothing while recording
pub fn build_later(folder: std::path::PathBuf) {
    if crate::audio::recording_commands::recording_now() {
        return;
    }
    std::thread::spawn(move || {
        crate::speakers::aligner::lower_priority();
        if let Err(e) = load_or_build(&folder) {
            log::warn!("Waveform not built: {e}");
        }
    });
}

#[tauri::command]
pub async fn api_waveform(state: tauri::State<'_, crate::state::AppState>, meeting_id: String) -> Result<Option<Waveform>, String> {
    if crate::audio::recording_commands::recording_now() {
        return Ok(None);
    }
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
        .bind(&meeting_id)
        .fetch_optional(state.db_manager.pool())
        .await
        .map_err(|e| e.to_string())?
        .flatten();
    let Some(folder) = folder.filter(|f| !f.trim().is_empty()) else { return Ok(None) };
    tokio::task::spawn_blocking(move || {
        crate::speakers::aligner::lower_priority();
        load_or_build(Path::new(&folder)).ok()
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A burst pattern like speech: loud and quiet within each second
    fn speech_like(n: usize, amp: f64) -> Vec<i16> {
        (0..n).map(|i| {
            let env = if (i / 2400) % 2 == 0 { 1.0 } else { 0.15 };
            ((i as f64 * 2.0 * std::f64::consts::PI * 800.0 / 16_000.0).sin() * amp * env * 32767.0) as i16
        }).collect()
    }

    #[test]
    fn digital_silence_and_hum_are_flat_speech_varies() {
        let rate = 16_000.0;
        let mut s = vec![0i16; 48_000]; // 3 s of nothing
        // 3 s of loud steady hum at 120 Hz: the ear calls it quiet
        s.extend((0..48_000).map(|i| ((i as f64 * 2.0 * std::f64::consts::PI * 120.0 / 16_000.0).sin() * 8000.0) as i16));
        s.extend(speech_like(96_000, 0.3)); // 6 s of speech
        s.extend(speech_like(48_000, 0.08)); // 3 s of quiet speech
        let w = compute(&s, rate);
        assert_eq!(w.rate, 50);
        assert_eq!(w.data.len(), s.len() / 320);
        assert!((w.duration - 15.0).abs() < 1e-9);
        let part = |a: f64, b: f64| &w.data[(a * 50.0) as usize..(b * 50.0) as usize];
        assert!(part(0.0, 3.0).iter().all(|&v| v == 0), "digital silence");
        assert!(part(3.2, 5.8).iter().all(|&v| v == 0), "hum");
        let loud = *part(6.5, 11.5).iter().max().unwrap();
        let quiet = *part(12.5, 14.5).iter().max().unwrap();
        assert!(loud > 200, "{loud}");
        assert!(quiet > 0 && (quiet as f32) < loud as f32 * 0.75, "{quiet} {loud}");
        let m = part(6.5, 11.5);
        assert!(m.iter().filter(|&&v| v > 150).count() > 40 && m.iter().filter(|&&v| v < 140).count() > 40, "speech moves");
    }

    #[test]
    fn cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let w = Waveform { duration: 12.5, rate: RATE, data: vec![0, 5, 255] };
        let mut b = w.duration.to_le_bytes().to_vec();
        b.extend_from_slice(&w.data);
        let p = dir.path().join(FILE);
        std::fs::write(&p, b).unwrap();
        assert_eq!(read_cache(&p), Some(w));
        assert!(compute(&[], 16_000.0).data.is_empty());
    }
}

#[cfg(test)]
mod timing {
    /// PROTOKOL_WAVE_FOLDER: a COPY of a meeting folder; prints the build time
    #[test]
    #[ignore = "needs a meeting folder copy"]
    fn build_time_on_a_copy() {
        let folder = std::path::PathBuf::from(std::env::var("PROTOKOL_WAVE_FOLDER").unwrap());
        let _ = std::fs::remove_file(folder.join(super::FILE));
        let t = std::time::Instant::now();
        let w = super::load_or_build(&folder).unwrap();
        let built = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        super::load_or_build(&folder).unwrap();
        let cached = t.elapsed().as_secs_f64();
        let size = std::fs::metadata(folder.join(super::FILE)).unwrap().len();
        let silent = w.data.iter().filter(|&&d| d == 0).count();
        println!("{:.0} s of sound: built in {built:.2} s, from cache in {cached:.3} s, {size} bytes, {silent}/{} slices silent", w.duration, w.data.len());
        // Per second, the first 25 s and a stretch in the middle: max height (0 = flat)
        let sec = |from: usize, n: usize| (from..from + n).map(|s| w.data[s * 50..(s + 1) * 50].iter().copied().max().unwrap()).collect::<Vec<u8>>();
        println!("first 25 s: {:?}", sec(0, 25));
        println!("from 300 s: {:?}", sec(300, 25));
    }
}
