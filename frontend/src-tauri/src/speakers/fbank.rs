//! Log-mel filterbank features (Kaldi style) for speaker embedding models.
//!
//! 25 ms frames every 10 ms, 512-point FFT, 80 triangular filters on the mel
//! scale, natural log of the power. Matches what WeSpeaker computes with
//! `torchaudio.compliance.kaldi.fbank` (dither 0) closely enough for
//! embeddings; NeMo models get the Hann window and per-bin normalisation.

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Window {
    Hamming,
    Hann,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FbankOptions {
    pub sample_rate: u32,
    pub num_bins: usize,
    pub window: Window,
    /// Multiply samples by 32768 first (models trained on 16-bit integers)
    pub scale_to_int16: bool,
    pub remove_dc_offset: bool,
    pub preemphasis: f32,
    pub low_freq: f32,
}

impl FbankOptions {
    pub fn wespeaker() -> Self {
        Self { sample_rate: 16_000, num_bins: 80, window: Window::Hamming, scale_to_int16: true, remove_dc_offset: true, preemphasis: 0.97, low_freq: 20.0 }
    }
    pub fn nemo() -> Self {
        Self { sample_rate: 16_000, num_bins: 80, window: Window::Hann, scale_to_int16: false, remove_dc_offset: false, preemphasis: 0.97, low_freq: 0.0 }
    }
}

const FRAME_MS: usize = 25;
const SHIFT_MS: usize = 10;
const FFT: usize = 512;

fn mel(hz: f32) -> f32 {
    1127.0 * (1.0 + hz / 700.0).ln()
}

pub(crate) struct Fbank {
    opts: FbankOptions,
    window: Vec<f32>,
    filters: Vec<(usize, Vec<f32>)>,
    fft: Arc<dyn RealToComplex<f32>>,
}

impl Fbank {
    pub fn new(opts: FbankOptions) -> Self {
        let frame = opts.sample_rate as usize * FRAME_MS / 1000;
        let window = (0..frame)
            .map(|i| {
                let a = 2.0 * PI * i as f32 / (frame - 1) as f32;
                match opts.window {
                    Window::Hamming => 0.54 - 0.46 * a.cos(),
                    Window::Hann => 0.5 - 0.5 * a.cos(),
                }
            })
            .collect();
        // Triangular filters equally spaced on the mel scale (Kaldi)
        let nyquist = opts.sample_rate as f32 / 2.0;
        let (lo, hi) = (mel(opts.low_freq), mel(nyquist));
        let delta = (hi - lo) / (opts.num_bins + 1) as f32;
        let bin_hz = opts.sample_rate as f32 / FFT as f32;
        let filters = (0..opts.num_bins)
            .map(|b| {
                let (left, center, right) = (lo + b as f32 * delta, lo + (b + 1) as f32 * delta, lo + (b + 2) as f32 * delta);
                let mut first = None;
                let mut weights = Vec::new();
                for k in 0..FFT / 2 {
                    let m = mel(k as f32 * bin_hz);
                    let w = if m > left && m < right {
                        if m <= center { (m - left) / (center - left) } else { (right - m) / (right - center) }
                    } else {
                        0.0
                    };
                    if w > 0.0 {
                        first.get_or_insert(k);
                        weights.push(w);
                    } else if first.is_some() {
                        break;
                    }
                }
                (first.unwrap_or(0), weights)
            })
            .collect();
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
        Self { opts, window, filters, fft }
    }

    /// Features, one row of `num_bins` values per 10 ms frame
    pub fn compute(&self, samples: &[f32]) -> Vec<Vec<f32>> {
        let frame = self.window.len();
        let shift = self.opts.sample_rate as usize * SHIFT_MS / 1000;
        if samples.len() < frame {
            return Vec::new();
        }
        let scale = if self.opts.scale_to_int16 { 32768.0 } else { 1.0 };
        let n_frames = 1 + (samples.len() - frame) / shift;
        let mut input = self.fft.make_input_vec();
        let mut spectrum = self.fft.make_output_vec();
        let mut out = Vec::with_capacity(n_frames);
        let mut buf = vec![0.0f32; frame];
        for f in 0..n_frames {
            let chunk = &samples[f * shift..f * shift + frame];
            for (b, s) in buf.iter_mut().zip(chunk) {
                *b = s * scale;
            }
            if self.opts.remove_dc_offset {
                let mean = buf.iter().sum::<f32>() / frame as f32;
                buf.iter_mut().for_each(|b| *b -= mean);
            }
            if self.opts.preemphasis > 0.0 {
                for i in (1..frame).rev() {
                    buf[i] -= self.opts.preemphasis * buf[i - 1];
                }
                buf[0] -= self.opts.preemphasis * buf[0];
            }
            input.iter_mut().for_each(|x| *x = 0.0);
            for i in 0..frame {
                input[i] = buf[i] * self.window[i];
            }
            if self.fft.process(&mut input, &mut spectrum).is_err() {
                continue;
            }
            let power: Vec<f32> = spectrum.iter().map(|c| c.norm_sqr()).collect();
            let row = self
                .filters
                .iter()
                .map(|(first, weights)| {
                    let e: f32 = weights.iter().enumerate().map(|(i, w)| w * power[first + i]).sum();
                    e.max(f32::EPSILON).ln()
                })
                .collect();
            out.push(row);
        }
        out
    }
}

/// Subtracts the mean of each bin over time (WeSpeaker's CMN)
pub(crate) fn subtract_mean(features: &mut [Vec<f32>]) {
    normalize(features, false);
}

/// Zero mean and unit variance per bin (NeMo "per_feature")
pub(crate) fn normalize_per_feature(features: &mut [Vec<f32>]) {
    normalize(features, true);
}

fn normalize(features: &mut [Vec<f32>], variance: bool) {
    let Some(dims) = features.first().map(Vec::len) else { return };
    let n = features.len() as f32;
    for d in 0..dims {
        let mean = features.iter().map(|r| r[d]).sum::<f32>() / n;
        let std = if variance {
            (features.iter().map(|r| (r[d] - mean).powi(2)).sum::<f32>() / (n - 1.0).max(1.0)).sqrt() + 1e-5
        } else {
            1.0
        };
        for r in features.iter_mut() {
            r[d] = (r[d] - mean) / std;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_bins() {
        let fb = Fbank::new(FbankOptions::wespeaker());
        let tone: Vec<f32> = (0..16_000).map(|i| (2.0 * PI * 440.0 * i as f32 / 16_000.0).sin() * 0.3).collect();
        let feats = fb.compute(&tone);
        assert_eq!(feats.len(), 98); // 1 s: 1 + (16000 - 400) / 160
        assert_eq!(feats[0].len(), 80);
        // A 440 Hz tone peaks in a low bin, far above the top bins
        let row = &feats[50];
        let peak = row.iter().cloned().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
        assert!(peak < 20, "peak bin {peak}");
        assert!(row[peak] > row[79] + 5.0);
    }

    #[test]
    fn mean_normalisation() {
        let mut f = vec![vec![1.0, 2.0], vec![3.0, 6.0]];
        subtract_mean(&mut f);
        assert_eq!(f, vec![vec![-1.0, -2.0], vec![1.0, 2.0]]);
    }
}
