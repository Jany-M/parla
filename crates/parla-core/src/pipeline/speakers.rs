/// Online clustering of voice embeddings (cosine similarity, running centroids).
///
/// Honest limit: short segments and overlapping speech give poor embeddings — the UI offers
/// rename/merge to fix the rest (PLAN §4).
pub struct OnlineClusterer {
    threshold: f32,
    max_speakers: usize,
    // (sum of normalised embeddings, count)
    clusters: Vec<(Vec<f32>, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Assignment {
    pub index: usize,
    pub is_new: bool,
    pub similarity: f32,
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let d = norm(a) * norm(b);
    if d == 0.0 {
        0.0
    } else {
        a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / d
    }
}

impl OnlineClusterer {
    pub fn new(threshold: f32, max_speakers: usize) -> Self {
        Self { threshold, max_speakers: max_speakers.max(1), clusters: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.clusters.len()
    }
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// `None` for unusable embeddings (empty, zero, NaN) — the caller keeps the previous speaker.
    pub fn assign(&mut self, emb: &[f32]) -> Option<Assignment> {
        let n = norm(emb);
        if emb.is_empty() || !n.is_finite() || n == 0.0 {
            return None;
        }
        if self.clusters.first().map(|c| c.0.len() != emb.len()).unwrap_or(false) {
            return None; // dimension changed (model swapped mid-session): ignore
        }
        let unit: Vec<f32> = emb.iter().map(|x| x / n).collect();
        let best = self
            .clusters
            .iter()
            .enumerate()
            .map(|(i, (c, _))| (i, cosine(c, &unit)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, sim)) if sim >= self.threshold || self.clusters.len() >= self.max_speakers => {
                let (sum, count) = &mut self.clusters[i];
                sum.iter_mut().zip(&unit).for_each(|(s, u)| *s += u);
                *count += 1;
                Some(Assignment { index: i, is_new: false, similarity: sim })
            }
            other => {
                self.clusters.push((unit, 1));
                Some(Assignment { index: self.clusters.len() - 1, is_new: true, similarity: other.map(|b| b.1).unwrap_or(1.0) })
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn one_hot(dim: usize, i: usize) -> Vec<f32> {
    let mut v = vec![0.0; dim];
    v[i] = 1.0;
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_voice_same_cluster_different_voice_new_cluster() {
        let mut c = OnlineClusterer::new(0.5, 8);
        let a = c.assign(&one_hot(4, 0)).unwrap();
        assert_eq!((a.index, a.is_new), (0, true));
        let a2 = c.assign(&[0.9, 0.1, 0.0, 0.0]).unwrap();
        assert_eq!((a2.index, a2.is_new), (0, false));
        let b = c.assign(&one_hot(4, 1)).unwrap();
        assert_eq!((b.index, b.is_new), (1, true));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn scale_of_the_embedding_does_not_matter() {
        let mut c = OnlineClusterer::new(0.5, 8);
        c.assign(&[1.0, 0.0]).unwrap();
        assert_eq!(c.assign(&[100.0, 1.0]).unwrap().index, 0);
    }

    #[test]
    fn speaker_cap_forces_nearest_assignment() {
        let mut c = OnlineClusterer::new(0.9, 2);
        c.assign(&one_hot(3, 0)).unwrap();
        c.assign(&one_hot(3, 1)).unwrap();
        let a = c.assign(&[0.2, 0.9, 0.1]).unwrap();
        assert_eq!((a.index, a.is_new), (1, false));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn centroid_follows_slow_drift() {
        let mut c = OnlineClusterer::new(0.8, 8);
        // a voice that drifts gradually stays one speaker
        for k in 0..10 {
            let t = k as f32 * 0.05;
            assert_eq!(c.assign(&[1.0 - t, t, 0.0]).unwrap().index, 0, "step {k}");
        }
    }

    #[test]
    fn invalid_embeddings_are_rejected() {
        let mut c = OnlineClusterer::new(0.5, 8);
        assert!(c.assign(&[]).is_none());
        assert!(c.assign(&[0.0, 0.0]).is_none());
        assert!(c.assign(&[f32::NAN, 1.0]).is_none());
        assert!(c.is_empty());
        c.assign(&[1.0, 0.0]).unwrap();
        assert!(c.assign(&[1.0, 0.0, 0.0]).is_none(), "dimension mismatch");
    }
}
