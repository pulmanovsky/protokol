//! Live input levels, microphone mute and source choice for the recording screen.
//!
//! Levels are measured inside the capture pipeline (`AudioCapture::process_audio_data`)
//! from the real microphone and system samples, after resampling and before
//! mixing, so no device is opened a second time. They are sent to the UI as a
//! throttled `live-audio-levels` event.
//!
//! Mute is in-app only: muted microphone samples are replaced with silence for
//! both the recording and the transcription, while the device stream stays
//! open so unmuting is instant. The macOS input volume is not touched.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use once_cell::sync::{Lazy, OnceCell};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Wry};

use super::recording_state::DeviceType;

/// About 20 updates per second
const EMIT_INTERVAL: Duration = Duration::from_millis(50);
pub const LIVE_LEVELS_EVENT: &str = "live-audio-levels";

static APP: OnceCell<AppHandle<Wry>> = OnceCell::new();
static MIC_MUTED: AtomicBool = AtomicBool::new(false);
static CALL_MUTED: AtomicBool = AtomicBool::new(false);
static MIC_ENABLED: AtomicBool = AtomicBool::new(true);
static SYSTEM_ENABLED: AtomicBool = AtomicBool::new(true);

#[derive(Default)]
struct Accumulator {
    sum_squares: f64,
    samples: u64,
    peak: f32,
}

impl Accumulator {
    fn add(&mut self, samples: &[f32]) {
        for &s in samples {
            self.sum_squares += (s as f64) * (s as f64);
            self.peak = self.peak.max(s.abs());
        }
        self.samples += samples.len() as u64;
    }

    /// (rms, peak, received any samples); resets the accumulator
    fn take(&mut self) -> (f32, f32, bool) {
        let active = self.samples > 0;
        let rms = if active { (self.sum_squares / self.samples as f64).sqrt() as f32 } else { 0.0 };
        let peak = self.peak;
        *self = Self::default();
        (rms, peak, active)
    }
}

struct LevelState {
    mic: Accumulator,
    system: Accumulator,
    last_emit: Instant,
}

static LEVELS: Lazy<Mutex<LevelState>> = Lazy::new(|| {
    Mutex::new(LevelState { mic: Accumulator::default(), system: Accumulator::default(), last_emit: Instant::now() })
});

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LiveAudioLevels {
    pub mic_rms: f32,
    pub mic_peak: f32,
    pub system_rms: f32,
    pub system_peak: f32,
    pub mic_muted: bool,
    /// The stream delivered samples since the last update
    pub mic_active: bool,
    pub system_active: bool,
}

/// Called once at startup so the capture threads can emit events
pub fn init(app: AppHandle<Wry>) {
    let _ = APP.set(app);
}

/// Measures one block of samples (mono, after resampling) and emits an
/// update when the interval has passed.
pub fn record(device_type: &DeviceType, samples: &[f32]) {
    if samples.is_empty() {
        return;
    }
    let update = {
        let Ok(mut state) = LEVELS.lock() else { return };
        match device_type {
            DeviceType::Microphone => state.mic.add(samples),
            DeviceType::System => state.system.add(samples),
        }
        if state.last_emit.elapsed() < EMIT_INTERVAL {
            return;
        }
        state.last_emit = Instant::now();
        let (mic_rms, mic_peak, mic_active) = state.mic.take();
        let (system_rms, system_peak, system_active) = state.system.take();
        LiveAudioLevels {
            mic_rms,
            mic_peak,
            system_rms,
            system_peak,
            mic_muted: is_mic_muted(),
            mic_active,
            system_active,
        }
    };
    if let Some(app) = APP.get() {
        let _ = app.emit(LIVE_LEVELS_EVENT, &update);
    }
}

pub fn is_mic_muted() -> bool {
    MIC_MUTED.load(Ordering::SeqCst)
}

/// Returns the previous state
pub fn set_mic_muted(muted: bool) -> bool {
    MIC_MUTED.swap(muted, Ordering::SeqCst)
}

pub fn is_call_muted() -> bool {
    CALL_MUTED.load(Ordering::SeqCst)
}

/// Returns the previous state
pub fn set_call_muted(muted: bool) -> bool {
    CALL_MUTED.swap(muted, Ordering::SeqCst)
}

/// Replaces the samples of a muted source with silence (in place)
pub fn apply_mute(device_type: &DeviceType, samples: &mut [f32]) {
    let muted = match device_type {
        DeviceType::Microphone => is_mic_muted(),
        DeviceType::System => is_call_muted(),
    };
    if muted {
        samples.fill(0.0);
    }
}

/// Which sources the next recording uses
pub fn set_sources(mic_enabled: bool, system_enabled: bool) {
    MIC_ENABLED.store(mic_enabled, Ordering::SeqCst);
    SYSTEM_ENABLED.store(system_enabled, Ordering::SeqCst);
}

pub fn sources() -> (bool, bool) {
    (MIC_ENABLED.load(Ordering::SeqCst), SYSTEM_ENABLED.load(Ordering::SeqCst))
}


// ---------------------------------------------------------------------------
// Microphone preview before recording (only while the source picker is open)
// ---------------------------------------------------------------------------

pub const MIC_PREVIEW_EVENT: &str = "mic-preview-level";
static PREVIEW_RUNNING: AtomicBool = AtomicBool::new(false);
static PREVIEW_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct MicPreviewLevel {
    pub rms: f32,
    pub peak: f32,
}

/// Opens the chosen microphone (or the default one) and emits its level until
/// `stop_mic_preview` is called. Not used while recording: then the levels
/// come from the recording pipeline.
pub fn start_mic_preview(device_name: Option<String>) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let generation = PREVIEW_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    PREVIEW_RUNNING.store(true, Ordering::SeqCst);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    std::thread::Builder::new()
        .name("mic-preview".into())
        .spawn(move || {
            let host = cpal::default_host();
            let wanted = device_name
                .as_deref()
                .map(|n| n.trim_end_matches("(input)").trim().to_string())
                .filter(|n| !n.is_empty());
            let device = match &wanted {
                Some(name) => crate::audio::devices::lookup::device_by_name(&host, name, true)
                    .or_else(|| host.default_input_device()),
                None => host.default_input_device(),
            };
            let Some(device) = device else {
                let _ = ready_tx.send(Err("Мікрофон не знайдено".into()));
                return;
            };
            let config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let channels = config.channels().max(1) as usize;
            let acc = std::sync::Arc::new(Mutex::new((Accumulator::default(), Instant::now())));

            let on_samples = {
                let acc = acc.clone();
                move |mono: &[f32]| {
                    let Ok(mut guard) = acc.lock() else { return };
                    guard.0.add(mono);
                    if guard.1.elapsed() >= EMIT_INTERVAL {
                        guard.1 = Instant::now();
                        let (rms, peak, _) = guard.0.take();
                        if let Some(app) = APP.get() {
                            let _ = app.emit(MIC_PREVIEW_EVENT, MicPreviewLevel { rms, peak });
                        }
                    }
                }
            };
            let to_mono = move |data: &[f32]| -> Vec<f32> {
                data.chunks(channels).map(|frame| frame.iter().sum::<f32>() / frame.len() as f32).collect()
            };
            let stream_config: cpal::StreamConfig = config.clone().into();
            let err_fn = |e| log::warn!("Mic preview stream error: {e}");
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| on_samples(&to_mono(data)),
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let f: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        on_samples(&to_mono(&f))
                    },
                    err_fn,
                    None,
                ),
                other => {
                    let _ = ready_tx.send(Err(format!("Unsupported sample format {other:?}")));
                    return;
                }
            };
            let stream = match stream.and_then(|s| s.play().map(|_| s).map_err(|e| cpal::BuildStreamError::BackendSpecific {
                err: cpal::BackendSpecificError { description: e.to_string() },
            })) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            // Keep the stream open until stopped or replaced by a newer preview
            while PREVIEW_RUNNING.load(Ordering::SeqCst) && PREVIEW_GENERATION.load(Ordering::SeqCst) == generation {
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(stream);
        })
        .map_err(|e| e.to_string())?;

    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "Мікрофон не відповідає".to_string())?
}

pub fn stop_mic_preview() {
    PREVIEW_RUNNING.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_reports_rms_and_peak() {
        let mut acc = Accumulator::default();
        acc.add(&[0.5, -0.5, 0.5, -0.5]);
        let (rms, peak, active) = acc.take();
        assert!((rms - 0.5).abs() < 1e-6);
        assert_eq!(peak, 0.5);
        assert!(active);
        assert_eq!(acc.take(), (0.0, 0.0, false));
    }

    /// One test for both flags: they are process-wide, and tests run in parallel
    #[test]
    fn mute_silences_only_the_chosen_source() {
        let silenced = |device: DeviceType| {
            let mut samples = vec![0.3_f32; 4];
            apply_mute(&device, &mut samples);
            samples.iter().all(|&s| s == 0.0)
        };

        set_mic_muted(true);
        assert!(silenced(DeviceType::Microphone));
        assert!(!silenced(DeviceType::System));
        set_mic_muted(false);

        set_call_muted(true);
        assert!(!silenced(DeviceType::Microphone));
        assert!(silenced(DeviceType::System));
        set_call_muted(false);
    }
}
