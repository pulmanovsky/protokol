//! Voice embeddings: a fixed-length vector per piece of speech, close for
//! the same voice and far apart for different voices.

use std::path::Path;

use anyhow::{anyhow, Result};
use ndarray::{Array1, Array3};
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;

use super::fbank::{normalize_per_feature, subtract_mean, Fbank, FbankOptions};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EmbeddingKind {
    /// WeSpeaker export: input `feats` [B, T, 80], output `embs`
    WeSpeaker,
    /// NeMo TitaNet export: input `audio_signal` [B, 80, T] and `length`
    Nemo,
}

pub(crate) struct Embedder {
    session: Session,
    kind: EmbeddingKind,
    fbank: Fbank,
}

/// Shortest speech an embedding is computed for
pub(crate) const MIN_EMBEDDING_SAMPLES: usize = 16_000 / 2;

impl Embedder {
    pub fn open(path: &Path) -> Result<Self> {
        Self::with_session(super::onnx::open(path)?)
    }

    /// On two threads, for checks in the background
    pub fn open_light(path: &Path) -> Result<Self> {
        Self::with_session(super::onnx::open_with_threads(path, 2)?)
    }

    fn with_session(session: ort::session::Session) -> Result<Self> {
        let framework = super::onnx::meta(&session, "framework").unwrap_or_default();
        let kind = if framework == "nemo" { EmbeddingKind::Nemo } else { EmbeddingKind::WeSpeaker };
        let opts = match kind {
            EmbeddingKind::WeSpeaker => FbankOptions::wespeaker(),
            EmbeddingKind::Nemo => FbankOptions::nemo(),
        };
        Ok(Self { session, kind, fbank: Fbank::new(opts) })
    }

    /// L2-normalised embedding of 16 kHz speech, None if too short
    pub fn embed(&mut self, samples: &[f32]) -> Result<Option<Vec<f32>>> {
        if samples.len() < MIN_EMBEDDING_SAMPLES {
            return Ok(None);
        }
        let mut feats = self.fbank.compute(samples);
        if feats.is_empty() {
            return Ok(None);
        }
        let t = feats.len();
        let dims = feats[0].len();
        let raw: Vec<f32> = match self.kind {
            EmbeddingKind::WeSpeaker => {
                subtract_mean(&mut feats);
                let flat: Vec<f32> = feats.into_iter().flatten().collect();
                let input = Array3::from_shape_vec((1, t, dims), flat)?;
                let outputs = self.session.run(inputs!["feats" => TensorRef::from_array_view(input.view())?])?;
                let (_, data) = outputs["embs"].try_extract_tensor::<f32>()?;
                data.to_vec()
            }
            EmbeddingKind::Nemo => {
                normalize_per_feature(&mut feats);
                let mut flat = vec![0.0f32; dims * t];
                for (j, row) in feats.iter().enumerate() {
                    for (d, v) in row.iter().enumerate() {
                        flat[d * t + j] = *v;
                    }
                }
                let input = Array3::from_shape_vec((1, dims, t), flat)?;
                let length = Array1::from_vec(vec![t as i64]);
                let outputs = self.session.run(inputs![
                    "audio_signal" => TensorRef::from_array_view(input.view())?,
                    "length" => TensorRef::from_array_view(length.view())?,
                ])?;
                let (_, data) = outputs["embs"].try_extract_tensor::<f32>()?;
                data.to_vec()
            }
        };
        let norm = raw.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !norm.is_finite() || norm == 0.0 {
            return Err(anyhow!("embedding is empty"));
        }
        Ok(Some(raw.into_iter().map(|v| v / norm).collect()))
    }
}

/// Cosine similarity of two L2-normalised vectors
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Mean of vectors, L2-normalised
pub(crate) fn centroid<'a>(vectors: impl IntoIterator<Item = &'a Vec<f32>>) -> Option<Vec<f32>> {
    let mut sum: Option<Vec<f32>> = None;
    for v in vectors {
        match sum.as_mut() {
            None => sum = Some(v.clone()),
            Some(s) => s.iter_mut().zip(v).for_each(|(a, b)| *a += b),
        }
    }
    let mut s = sum?;
    let norm = s.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 {
        return None;
    }
    s.iter_mut().for_each(|v| *v /= norm);
    Some(s)
}
