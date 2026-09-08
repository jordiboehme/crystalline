//! `V301`'s semantic twin finder.
//!
//! `V201` finds copies: bodies that share word 3-grams. Two engrams that say
//! the same thing in different words share none, so the lexical pass cannot
//! see them. This pass compares what the embedding model saw: each engram's
//! lead vector, the embedding of its first chunk (title, description and
//! opening body), all pairs, cosine at or above the twin threshold.
//!
//! Two bounds keep it honest about cost. All-pairs is quadratic, so a scope
//! over [`super::MAX_TWIN_VECTORS`] vectors is reported and skipped rather
//! than run; and the pass hands back every pair over the threshold, sorted
//! closest first, so the caller can take its finding cap off the top.
//!
//! What this is not: a contradiction detector. Two texts close in embedding
//! space agree about their topic and nothing else; the finding text says so.

use super::SweepOptions;

/// One pair over the threshold, as indices into the caller's vector list.
#[derive(Debug, Clone, PartialEq)]
pub struct TwinPair {
    /// The lower index of the pair.
    pub a: usize,
    /// The higher index of the pair.
    pub b: usize,
    /// The cosine similarity of the two lead vectors.
    pub cosine: f64,
}

/// The result of one pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TwinPairs {
    /// Every pair at or above the threshold, cosine descending then `(a, b)`
    /// ascending, so the order is the same on every run.
    pub pairs: Vec<TwinPair>,
    /// How many vectors were present (not `None`).
    pub compared: usize,
    /// Whether the vector cap stopped the pass before it compared anything.
    pub capped: bool,
}

/// Cosine similarity in f64, `0.0` for a zero vector or mismatched widths:
/// a mismatch means two models' vectors met, and those must never score.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// All pairs of present vectors at or above `options.twin_threshold`. An
/// absent vector (`None`) takes part in nothing; the indices in the result
/// index `vectors`, so a caller keeps its own alignment.
pub fn find_twins(vectors: &[Option<&[f32]>], options: &SweepOptions) -> TwinPairs {
    let present: Vec<usize> = vectors
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|_| i))
        .collect();
    let mut out = TwinPairs {
        compared: present.len(),
        ..TwinPairs::default()
    };
    if present.len() > options.max_twin_vectors {
        out.capped = true;
        return out;
    }
    for (pi, &i) in present.iter().enumerate() {
        for &j in &present[pi + 1..] {
            let (Some(a), Some(b)) = (vectors[i], vectors[j]) else {
                continue;
            };
            // A width mismatch is a model swap caught mid-backfill: the two
            // vectors live in different spaces, so there is no angle between
            // them to measure.
            if a.len() != b.len() {
                continue;
            }
            let sim = cosine(a, b);
            if sim >= options.twin_threshold {
                out.pairs.push(TwinPair {
                    a: i,
                    b: j,
                    cosine: sim,
                });
            }
        }
    }
    out.pairs.sort_by(|x, y| {
        y.cosine
            .total_cmp(&x.cosine)
            .then_with(|| (x.a, x.b).cmp(&(y.a, y.b)))
    });
    out
}
