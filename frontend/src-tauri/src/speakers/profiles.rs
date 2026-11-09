//! Voice profiles: the voice of a person the user has named once is
//! recognised in later meetings. Profiles are mean embeddings kept in the
//! app database on this Mac.

use super::embedding::cosine;

/// At or above this similarity a voice gets the profile's name
pub(crate) const NAME_SIMILARITY: f32 = 0.65;
/// Between this and NAME_SIMILARITY the name gets a «?»
pub(crate) const UNSURE_SIMILARITY: f32 = 0.5;

#[derive(Debug, Clone)]
pub struct VoiceProfile {
    pub name: String,
    pub embedding: Vec<f32>,
    pub meetings: i64,
    pub role: Option<String>,
}

pub(crate) fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub(crate) fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Names for the voices of a meeting: each profile is used at most once,
/// best matches first. Returns, per centroid, Some("Name") or Some("Name?").
pub(crate) fn match_profiles(centroids: &[Vec<f32>], profiles: &[VoiceProfile]) -> Vec<Option<String>> {
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (c, centroid) in centroids.iter().enumerate() {
        for (p, profile) in profiles.iter().enumerate() {
            if profile.embedding.len() == centroid.len() {
                let sim = cosine(centroid, &profile.embedding);
                if sim >= UNSURE_SIMILARITY {
                    pairs.push((sim, c, p));
                }
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out: Vec<Option<String>> = vec![None; centroids.len()];
    let mut used = vec![false; profiles.len()];
    for (sim, c, p) in pairs {
        if out[c].is_some() || used[p] {
            continue;
        }
        used[p] = true;
        let name = &profiles[p].name;
        out[c] = Some(if sim >= NAME_SIMILARITY { name.clone() } else { format!("{name}?") });
    }
    out
}

/// A profile updated with one more meeting's voice (running mean)
pub(crate) fn updated_embedding(old: &[f32], meetings: i64, new: &[f32]) -> Vec<f32> {
    if old.len() != new.len() || meetings <= 0 {
        return new.to_vec();
    }
    let n = meetings as f32;
    let mut v: Vec<f32> = old.iter().zip(new).map(|(a, b)| (a * n + b) / (n + 1.0)).collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_name_matching_voices_once() {
        let profiles = vec![
            VoiceProfile { name: "Олена".into(), embedding: vec![1.0, 0.0, 0.0], meetings: 2, role: None },
            VoiceProfile { name: "Богдан".into(), embedding: vec![0.0, 1.0, 0.0], meetings: 1, role: None },
        ];
        let c = vec![vec![0.95, 0.31, 0.0], vec![0.6, 0.0, 0.8], vec![0.0, 0.0, 1.0], vec![0.1, 0.99, 0.0]];
        assert_eq!(
            match_profiles(&c, &profiles),
            vec![Some("Олена".into()), None, None, Some("Богдан".into())]
        );
        let unsure = vec![vec![0.55, 0.0, 0.835]];
        assert_eq!(match_profiles(&unsure, &profiles), vec![Some("Олена?".into())]);
    }

    #[test]
    fn blobs_round_trip() {
        let v = vec![0.25f32, -1.5, 3.0];
        assert_eq!(from_blob(&to_blob(&v)), v);
        let u = updated_embedding(&[1.0, 0.0], 1, &[0.0, 1.0]);
        assert!((u[0] - u[1]).abs() < 1e-6);
    }
}
