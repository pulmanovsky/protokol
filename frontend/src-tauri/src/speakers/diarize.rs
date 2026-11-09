//! Who spoke when, from one 16 kHz mono track.
//!
//! Follows the pyannote 3 recipe as implemented in sherpa-onnx:
//! 1. the segmentation model looks at 10 s windows (every `WINDOW_STEP_S`)
//!    and marks, per 17 ms frame, which of up to 3 local speakers talk;
//! 2. the speech of each local speaker in each window gets a voice embedding;
//! 3. embeddings are clustered, so local speakers across windows become
//!    global speakers;
//! 4. per frame, the most likely global speaker(s) are kept and turned into
//!    turns: «speaker k from t1 to t2».

use std::path::Path;

use anyhow::{anyhow, Result};
use log::info;
use ndarray::Array3;
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;

use super::cluster::agglomerative;
use super::embedding::{centroid, Embedder};

/// Error text of a run the user stopped
pub const STOPPED: &str = "Зупинено";

/// Distance between windows of the segmentation model
pub(crate) const WINDOW_STEP_S: f32 = 2.0;
/// Largest average cosine distance at which voices are taken as the same
pub(crate) const DEFAULT_CLUSTER_THRESHOLD: f32 = 0.6;
/// Only pieces at least this long shape the clusters; shorter ones join the
/// nearest cluster afterwards (their embeddings are noisy)
const MIN_CLUSTER_SPEECH_S: f32 = 1.5;
/// Clusters whose voice centroids are at least this similar are one person
/// (e.g. a chair speaking between reports, split by average linkage)
pub(crate) const CENTROID_MERGE_SIMILARITY: f32 = 0.65;
/// Shorter pauses inside one speaker's speech are bridged
const MIN_GAP_S: f64 = 0.5;
/// Shorter turns are absorbed by their neighbours
const MIN_TURN_S: f64 = 0.4;
const BATCH: usize = 16;

/// A stretch of speech by one speaker (seconds from the start of the audio)
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: usize,
}

#[derive(Debug, Clone)]
pub struct Diarization {
    pub turns: Vec<Turn>,
    /// Voice centroid per speaker (index = speaker), for naming and profiles
    pub centroids: Vec<Vec<f32>>,
    /// Seconds of speech per speaker
    pub speech_s: Vec<f64>,
}

struct Segmenter {
    session: Session,
    window: usize,
    rf_shift: usize,
    rf_size: usize,
    num_speakers: usize,
}

impl Segmenter {
    fn open(path: &Path) -> Result<Self> {
        let session = super::onnx::open(path)?;
        let get = |k: &str| super::onnx::meta(&session, k).and_then(|v| v.parse::<usize>().ok()).ok_or_else(|| anyhow!("segmentation model has no {k}"));
        let window = get("window_size")?;
        let rf_shift = get("receptive_field_shift")?;
        let rf_size = get("receptive_field_size")?;
        let num_speakers = get("num_speakers")?;
        if get("powerset_max_classes")? != 2 {
            return Err(anyhow!("unsupported segmentation model"));
        }
        Ok(Self { session, window, rf_shift, rf_size, num_speakers })
    }

    /// Powerset class → local speakers (class 0 = silence, then singles, then pairs)
    fn mapping(&self) -> Vec<Vec<usize>> {
        let mut m = vec![Vec::new()];
        for j in 0..self.num_speakers {
            m.push(vec![j]);
        }
        for j in 0..self.num_speakers {
            for k in (j + 1)..self.num_speakers {
                m.push(vec![j, k]);
            }
        }
        m
    }

    /// Per window: per frame, which local speakers are active
    fn run(&mut self, audio: &[f32], step: usize, progress: &dyn Fn(f32), stop: &dyn Fn() -> bool) -> Result<Vec<Vec<Vec<bool>>>> {
        let n_windows = if audio.len() <= self.window { 1 } else { (audio.len() - self.window).div_ceil(step) + 1 };
        let mapping = self.mapping();
        let mut out = Vec::with_capacity(n_windows);
        let mut w = 0;
        while w < n_windows {
            let b = BATCH.min(n_windows - w);
            let mut batch = vec![0.0f32; b * self.window];
            for i in 0..b {
                let start = (w + i) * step;
                let end = (start + self.window).min(audio.len());
                if start < end {
                    batch[i * self.window..i * self.window + (end - start)].copy_from_slice(&audio[start..end]);
                }
            }
            let input = Array3::from_shape_vec((b, 1, self.window), batch)?;
            let outputs = self.session.run(inputs!["x" => TensorRef::from_array_view(input.view())?])?;
            let (shape, data) = outputs["y"].try_extract_tensor::<f32>()?;
            let (frames, classes) = (shape[1] as usize, shape[2] as usize);
            for i in 0..b {
                let mut window_labels = Vec::with_capacity(frames);
                for f in 0..frames {
                    let row = &data[(i * frames + f) * classes..(i * frames + f + 1) * classes];
                    let class = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(c, _)| c).unwrap_or(0);
                    let mut active = vec![false; self.num_speakers];
                    for &s in mapping.get(class).map(Vec::as_slice).unwrap_or(&[]) {
                        active[s] = true;
                    }
                    window_labels.push(active);
                }
                out.push(window_labels);
            }
            w += b;
            progress(w as f32 / n_windows as f32);
            if stop() {
                return Err(anyhow!(STOPPED));
            }
        }
        Ok(out)
    }
}

/// Diarizes 16 kHz mono audio with the given models.
pub fn diarize(audio: &[f32], segmentation_model: &Path, embedding_model: &Path, threshold: f32) -> Result<Diarization> {
    diarize_with_progress(audio, segmentation_model, embedding_model, threshold, &|_| {})
}

/// [`diarize`] reporting how far it got (0.0–1.0)
pub fn diarize_with_progress(
    audio: &[f32],
    segmentation_model: &Path,
    embedding_model: &Path,
    threshold: f32,
    progress: &dyn Fn(f32),
) -> Result<Diarization> {
    diarize_cancellable(audio, segmentation_model, embedding_model, threshold, progress, &|| false)
}

/// [`diarize_with_progress`] that stops (with an error) when `stop` says so
pub fn diarize_cancellable(
    audio: &[f32],
    segmentation_model: &Path,
    embedding_model: &Path,
    threshold: f32,
    progress: &dyn Fn(f32),
    stop: &dyn Fn() -> bool,
) -> Result<Diarization> {
    let started = std::time::Instant::now();
    let mut seg = Segmenter::open(segmentation_model)?;
    let mut embedder = Embedder::open(embedding_model)?;
    let step = (WINDOW_STEP_S * 16_000.0) as usize;
    let labels = seg.run(audio, step, &|f| progress(f * 0.3), stop)?;
    let frames_per_window = labels.first().map(Vec::len).unwrap_or(0);
    if frames_per_window == 0 {
        return Ok(Diarization { turns: Vec::new(), centroids: Vec::new(), speech_s: Vec::new() });
    }
    let seg_s = started.elapsed().as_secs_f64();
    // Sample position of a frame inside a window
    let frame_sample = |f: usize| f * seg.window / frames_per_window;

    // Embedding per (window, local speaker) with enough speech
    let mut keys: Vec<(usize, usize)> = Vec::new();
    let mut embeddings: Vec<Vec<f32>> = Vec::new();
    let mut long_enough: Vec<bool> = Vec::new();
    let mut buffer: Vec<f32> = Vec::with_capacity(seg.window);
    let n_windows = labels.len().max(1);
    for (w, window_labels) in labels.iter().enumerate() {
        if w % 20 == 0 {
            progress(0.3 + 0.65 * w as f32 / n_windows as f32);
            if stop() {
                return Err(anyhow!(STOPPED));
            }
        }
        let offset = w * step;
        for s in 0..seg.num_speakers {
            // Only frames where this speaker talks alone: overlap blurs the voice
            buffer.clear();
            let mut run_start: Option<usize> = None;
            for f in 0..=frames_per_window {
                let alone = f < frames_per_window && window_labels[f][s] && window_labels[f].iter().filter(|&&a| a).count() == 1;
                match (alone, run_start) {
                    (true, None) => run_start = Some(f),
                    (false, Some(r)) => {
                        let (a, b) = ((offset + frame_sample(r)).min(audio.len()), (offset + frame_sample(f)).min(audio.len()));
                        buffer.extend_from_slice(&audio[a..b]);
                        run_start = None;
                    }
                    _ => {}
                }
            }
            if let Some(e) = embedder.embed(&buffer)? {
                keys.push((w, s));
                embeddings.push(e);
                long_enough.push(buffer.len() as f32 >= MIN_CLUSTER_SPEECH_S * 16_000.0);
            }
        }
    }
    let emb_s = started.elapsed().as_secs_f64() - seg_s;

    // Cluster the reliable pieces, then attach the short ones to the nearest voice
    let reliable: Vec<usize> = (0..embeddings.len()).filter(|&i| long_enough[i]).collect();
    let reliable_vectors: Vec<Vec<f32>> = reliable.iter().map(|&i| embeddings[i].clone()).collect();
    let reliable_clusters = agglomerative(&reliable_vectors, threshold);
    let n_clusters = reliable_clusters.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut centroids_tmp: Vec<Vec<f32>> = Vec::with_capacity(n_clusters);
    for c in 0..n_clusters {
        let members = reliable_vectors.iter().zip(&reliable_clusters).filter(|(_, &k)| k == c).map(|(e, _)| e);
        centroids_tmp.push(centroid(members).unwrap_or_default());
    }
    let mut clusters: Vec<Option<usize>> = vec![None; embeddings.len()];
    for (j, &i) in reliable.iter().enumerate() {
        clusters[i] = Some(reliable_clusters[j]);
    }
    for i in 0..embeddings.len() {
        if clusters[i].is_none() {
            clusters[i] = centroids_tmp
                .iter()
                .enumerate()
                .map(|(c, ce)| (c, super::embedding::cosine(ce, &embeddings[i])))
                .filter(|(_, sim)| 1.0 - sim <= threshold)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(c, _)| c);
        }
    }
    // One person split in two (e.g. a chair between reports): join close voices
    let assigned: Vec<usize> = (0..embeddings.len()).filter(|&i| clusters[i].is_some()).collect();
    let assigned_vectors: Vec<Vec<f32>> = assigned.iter().map(|&i| embeddings[i].clone()).collect();
    let merged = merge_close_clusters(&assigned_vectors, assigned.iter().map(|&i| clusters[i].unwrap()).collect());
    for (j, &i) in assigned.iter().enumerate() {
        clusters[i] = Some(merged[j]);
    }
    let n_clusters = merged.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let cluster_of = |w: usize, s: usize| keys.iter().position(|&k| k == (w, s)).and_then(|i| clusters[i]);

    // Global frame grid: votes per cluster, and how many talk at once
    let total_frames = audio.len() / seg.rf_shift + 1;
    let mut votes = vec![vec![0.0f32; n_clusters]; total_frames];
    let mut talking = vec![0.0f32; total_frames];
    let mut seen = vec![0.0f32; total_frames];
    for (w, window_labels) in labels.iter().enumerate() {
        let local_cluster: Vec<Option<usize>> = (0..seg.num_speakers).map(|s| cluster_of(w, s)).collect();
        for (f, active) in window_labels.iter().enumerate() {
            let g = (w * step + frame_sample(f)) / seg.rf_shift;
            if g >= total_frames {
                break;
            }
            seen[g] += 1.0;
            talking[g] += active.iter().filter(|&&a| a).count() as f32;
            for (s, &a) in active.iter().enumerate() {
                if a {
                    if let Some(c) = local_cluster[s] {
                        votes[g][c] += 1.0;
                    }
                }
            }
        }
    }

    // One speaker per frame: the transcript needs a single label per moment
    let frame_s = seg.rf_shift as f64 / 16_000.0;
    let half_rf = seg.rf_size as f64 / 16_000.0 / 2.0;
    let mut per_frame: Vec<Option<usize>> = Vec::with_capacity(total_frames);
    for g in 0..total_frames {
        let speaking = seen[g] > 0.0 && talking[g] / seen[g] >= 0.5;
        let best = votes[g].iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).filter(|(_, v)| **v > 0.0).map(|(c, _)| c);
        per_frame.push(if speaking { best } else { None });
    }
    let mut turns: Vec<Turn> = Vec::new();
    for (g, sp) in per_frame.iter().enumerate() {
        let Some(sp) = *sp else { continue };
        let t = g as f64 * frame_s + half_rf;
        match turns.last_mut() {
            Some(last) if last.speaker == sp && t - last.end <= MIN_GAP_S => last.end = t + frame_s,
            _ => turns.push(Turn { start: t, end: t + frame_s, speaker: sp }),
        }
    }
    let turns = smooth_turns(turns);

    // Centroids and speech time per speaker
    let mut centroids = Vec::with_capacity(n_clusters);
    for c in 0..n_clusters {
        let members = embeddings.iter().zip(&clusters).filter(|(_, k)| **k == Some(c)).map(|(e, _)| e);
        centroids.push(centroid(members).unwrap_or_default());
    }
    let mut speech_s = vec![0.0; n_clusters];
    for t in &turns {
        speech_s[t.speaker] += t.end - t.start;
    }
    info!(
        "Diarization: {} speaker(s), {} turn(s); segmentation {:.1}s, {} embeddings {:.1}s, total {:.1}s",
        n_clusters,
        turns.len(),
        seg_s,
        embeddings.len(),
        emb_s,
        started.elapsed().as_secs_f64()
    );
    Ok(Diarization { turns, centroids, speech_s })
}

/// Joins clusters whose centroids are very close, repeatedly, closest first;
/// clusters are renumbered by first appearance
pub(crate) fn merge_close_clusters(vectors: &[Vec<f32>], mut clusters: Vec<usize>) -> Vec<usize> {
    loop {
        let n = clusters.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        let centroids: Vec<Option<Vec<f32>>> = (0..n)
            .map(|c| centroid(vectors.iter().zip(&clusters).filter(|(_, &k)| k == c).map(|(v, _)| v)))
            .collect();
        let mut best: Option<(f32, usize, usize)> = None;
        for a in 0..n {
            for b in (a + 1)..n {
                if let (Some(x), Some(y)) = (&centroids[a], &centroids[b]) {
                    let sim = super::embedding::cosine(x, y);
                    if sim >= CENTROID_MERGE_SIMILARITY && best.map(|(s, _, _)| sim > s).unwrap_or(true) {
                        best = Some((sim, a, b));
                    }
                }
            }
        }
        let Some((_, a, b)) = best else { break };
        clusters.iter_mut().filter(|k| **k == b).for_each(|k| *k = a);
        // Renumber by first appearance
        let mut map: Vec<Option<usize>> = vec![None; n];
        let mut next = 0;
        for k in clusters.iter_mut() {
            *k = *map[*k].get_or_insert_with(|| {
                next += 1;
                next - 1
            });
        }
    }
    clusters
}

/// Absorbs very short turns into the neighbour and joins same-speaker turns
pub(crate) fn smooth_turns(mut turns: Vec<Turn>) -> Vec<Turn> {
    let mut changed = true;
    while changed {
        changed = false;
        let mut i = 0;
        while i < turns.len() {
            if turns[i].end - turns[i].start < MIN_TURN_S && turns.len() > 1 {
                // Give it to the longer neighbour that touches it
                let prev_len = if i > 0 { turns[i - 1].end - turns[i - 1].start } else { -1.0 };
                let next_len = if i + 1 < turns.len() { turns[i + 1].end - turns[i + 1].start } else { -1.0 };
                if prev_len >= next_len && i > 0 {
                    turns[i - 1].end = turns[i].end;
                } else if i + 1 < turns.len() {
                    turns[i + 1].start = turns[i].start;
                }
                turns.remove(i);
                changed = true;
                continue;
            }
            i += 1;
        }
        let mut merged: Vec<Turn> = Vec::with_capacity(turns.len());
        for t in turns.drain(..) {
            match merged.last_mut() {
                Some(last) if last.speaker == t.speaker && t.start - last.end <= MIN_GAP_S => {
                    last.end = last.end.max(t.end);
                    changed = true;
                }
                _ => merged.push(t),
            }
        }
        turns = merged;
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_clusters_are_merged() {
        let v = vec![vec![1.0, 0.0], vec![0.8, 0.6], vec![0.0, 1.0]];
        // 0 and 1: similarity 0.8 → one person; 2 stays apart (0.6 with 1)
        assert_eq!(merge_close_clusters(&v, vec![0, 1, 2]), vec![0, 0, 1]);
    }

    #[test]
    fn short_turns_are_absorbed() {
        let t = |s: f64, e: f64, k: usize| Turn { start: s, end: e, speaker: k };
        let turns = vec![t(0.0, 5.0, 0), t(5.0, 5.2, 1), t(5.2, 9.0, 0), t(9.5, 12.0, 1)];
        assert_eq!(smooth_turns(turns), vec![t(0.0, 9.0, 0), t(9.5, 12.0, 1)]);
    }
}
