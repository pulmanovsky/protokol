//! Agglomerative clustering of voice embeddings (average linkage on cosine
//! distance), stopped at a distance threshold, so the number of speakers is
//! found rather than given.

/// Cluster index per input vector, clusters numbered by first appearance.
/// `threshold` is the largest average cosine distance (1 - similarity) at
/// which two clusters are still merged.
pub(crate) fn agglomerative(vectors: &[Vec<f32>], threshold: f32) -> Vec<usize> {
    let n = vectors.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0];
    }
    // Condensed full distance matrix, updated with Lance–Williams (average linkage)
    let mut dist = vec![0.0f32; n * n];
    for i in 0..n {
        for j in (i + 1)..n {
            let d = 1.0 - super::embedding::cosine(&vectors[i], &vectors[j]);
            dist[i * n + j] = d;
            dist[j * n + i] = d;
        }
    }
    let mut size = vec![1usize; n];
    let mut active = vec![true; n];
    // Nearest-neighbour chain builds the full average-linkage tree in O(n²).
    // Each merge keeps the surviving index; merges are recorded with their
    // distance and applied afterwards up to the threshold (average linkage is
    // monotone, so this is the same as stopping at the threshold).
    let mut merges: Vec<(usize, usize, f32)> = Vec::with_capacity(n - 1);
    let mut chain: Vec<usize> = Vec::new();
    let mut remaining = n;
    while remaining > 1 {
        if chain.is_empty() {
            chain.push((0..n).find(|&i| active[i]).unwrap());
        }
        let a = *chain.last().unwrap();
        let prev = if chain.len() >= 2 { Some(chain[chain.len() - 2]) } else { None };
        // Nearest active neighbour of a, preferring prev on ties
        let mut best = prev.unwrap_or(usize::MAX);
        let mut best_d = prev.map(|p| dist[a * n + p]).unwrap_or(f32::INFINITY);
        for j in 0..n {
            if active[j] && j != a && dist[a * n + j] < best_d {
                best = j;
                best_d = dist[a * n + j];
            }
        }
        if Some(best) != prev {
            chain.push(best);
            continue;
        }
        // a and best are mutual nearest neighbours: merge best into a
        chain.pop();
        chain.pop();
        let (sa, sb) = (size[a] as f32, size[best] as f32);
        for k in 0..n {
            if active[k] && k != a && k != best {
                let d = (sa * dist[a * n + k] + sb * dist[best * n + k]) / (sa + sb);
                dist[a * n + k] = d;
                dist[k * n + a] = d;
            }
        }
        size[a] += size[best];
        active[best] = false;
        merges.push((a, best, best_d));
        remaining -= 1;
    }
    let mut parent: Vec<usize> = (0..n).collect();
    for (a, b, d) in merges {
        if d <= threshold {
            parent[b] = a;
        }
    }
    // Resolve roots and number clusters by first appearance
    let root = |mut i: usize| {
        while parent[i] != i {
            i = parent[i];
        }
        i
    };
    let mut ids: Vec<Option<usize>> = vec![None; n];
    let mut next = 0;
    (0..n)
        .map(|i| {
            let r = root(i);
            *ids[r].get_or_insert_with(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn finds_the_number_of_groups() {
        let v = vec![
            unit(&[1.0, 0.0, 0.05]),
            unit(&[0.0, 1.0, 0.0]),
            unit(&[1.0, 0.1, 0.0]),
            unit(&[0.05, 1.0, 0.1]),
            unit(&[0.0, 0.0, 1.0]),
        ];
        assert_eq!(agglomerative(&v, 0.3), vec![0, 1, 0, 1, 2]);
        // A loose threshold joins everything
        assert_eq!(agglomerative(&v, 1.5), vec![0, 0, 0, 0, 0]);
        assert_eq!(agglomerative(&[], 0.3), Vec::<usize>::new());
    }
}
