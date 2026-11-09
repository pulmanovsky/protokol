//! Experiment (ignored): how close Whisper's word times are to where the
//! words really start, with the current token times and with DTW (cross-
//! attention alignment). Reference: speech onsets after a pause, from the
//! signal's energy — the first word after silence should start there.
//!
//! PROTOKOL_DTW_PCM=excerpt (16 kHz mono s16le) PROTOKOL_DTW_MODEL=ggml-large-v3.bin

#[cfg(test)]
mod tests {
    use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    /// (word, t0, t1, t_dtw) in seconds, word-initial tokens only
    fn words(samples: &[f32], model: &str, dtw: bool, flash: bool) -> (Vec<(String, f64, f64, f64)>, f64) {
        let mut cp = WhisperContextParameters::default();
        cp.use_gpu = true;
        cp.flash_attn = flash;
        if dtw {
            cp.dtw_parameters = DtwParameters { mode: DtwMode::ModelPreset { model_preset: DtwModelPreset::LargeV3 }, ..Default::default() };
        }
        let ctx = WhisperContext::new_with_params(model, cp).unwrap();
        let mut state = ctx.create_state().unwrap();
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some("uk"));
        p.set_token_timestamps(true);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        let started = std::time::Instant::now();
        state.full(p, samples).unwrap();
        let secs = started.elapsed().as_secs_f64();
        let mut out = Vec::new();
        for i in 0..state.full_n_segments().unwrap() {
            for j in 0..state.full_n_tokens(i).unwrap() {
                let Ok(text) = state.full_get_token_text(i, j) else { continue };
                if text.starts_with("[_") || text.starts_with("<|") {
                    continue;
                }
                let d = state.full_get_token_data(i, j).unwrap();
                if text.starts_with(' ') || out.is_empty() {
                    out.push((text.trim().to_string(), d.t0 as f64 / 100.0, d.t1 as f64 / 100.0, d.t_dtw as f64 / 100.0));
                }
            }
        }
        (out, secs)
    }

    /// Speech onsets after at least 0.4 s of quiet (10 ms frames)
    fn onsets(samples: &[f32]) -> Vec<f64> {
        let frame = 160;
        let rms: Vec<f32> = samples.chunks(frame).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let mut sorted = rms.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let floor = sorted[sorted.len() / 10];
        let speech = sorted[sorted.len() * 7 / 10];
        let thr = floor + (speech - floor) * 0.3;
        let mut out = Vec::new();
        let mut quiet = 0;
        for (i, r) in rms.iter().enumerate() {
            if *r < thr {
                quiet += 1;
            } else {
                if quiet >= 40 {
                    out.push(i as f64 / 100.0);
                }
                quiet = 0;
            }
        }
        out
    }

    #[test]
    #[ignore = "needs an excerpt and the model"]
    fn word_times_against_speech_onsets() {
        let raw = std::fs::read(std::env::var("PROTOKOL_DTW_PCM").unwrap()).unwrap();
        let samples: Vec<f32> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let model = std::env::var("PROTOKOL_DTW_MODEL").unwrap();
        let on = onsets(&samples);
        println!("{} onsets after a pause in {:.0} s", on.len(), samples.len() as f64 / 16000.0);
        for (name, dtw, flash) in [("current (flash attention)", false, true), ("DTW large-v3 (no flash attention)", true, false)] {
            let (w, secs) = words(&samples, &model, dtw, flash);
            // For each onset: the word whose start is nearest (within 1.5 s)
            let report = |label: &str, f: &dyn Fn(&(String, f64, f64, f64)) -> f64| {
                let mut diffs = Vec::new();
                for o in &on {
                    if let Some(d) = w.iter().map(|x| f(x) - o).filter(|d| d.abs() < 1.5).min_by(|a, b| a.abs().total_cmp(&b.abs())) {
                        diffs.push(d);
                    }
                }
                let n = diffs.len().max(1) as f64;
                let mean = diffs.iter().sum::<f64>() / n;
                let abs = diffs.iter().map(|d| d.abs()).sum::<f64>() / n;
                diffs.sort_by(|a, b| a.abs().total_cmp(&b.abs()));
                let p80 = diffs.get(diffs.len() * 8 / 10).map(|d| d.abs()).unwrap_or(0.0);
                println!("  {label:<22} matched {:>3}  mean {:+.3} s  mean |Δ| {:.3} s  80% within {:.3} s", diffs.len(), mean, abs, p80);
            };
            println!("{name}: {} words, {:.1} s", w.len(), secs);
            report("t0 (token start)", &|x| x.1);
            report("(t0+t1)/2 (now used)", &|x| (x.1 + x.2) / 2.0);
            if dtw {
                report("t_dtw", &|x| x.3);
            }
        }
    }
}
