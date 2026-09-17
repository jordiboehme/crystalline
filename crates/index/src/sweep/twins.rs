//! `V301`'s semantic twin finder.
//!
//! `V201` finds copies: bodies that share word 3-grams. Two engrams that say
//! the same thing in different words share none, so the lexical pass cannot
//! see them. This pass compares what the embedding model saw: each engram's
//! lead vector, the embedding of its first chunk (title, description and
//! opening body), all pairs, cosine at or above the twin threshold.
//!
//! Three bounds keep it honest about cost. All-pairs is quadratic, so a scope
//! over [`super::MAX_TWIN_VECTORS`] vectors is reported and skipped rather
//! than run; a dense scope under that ceiling could still clear the threshold
//! on millions of pairs, so only the best [`super::MAX_TWIN_PAIRS`] are ever
//! retained; and what comes back is sorted closest first, so the caller takes
//! its finding cap off the top.
//!
//! What this is not: a contradiction detector. Two texts close in embedding
//! space agree about their topic and nothing else; the finding text says so.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

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
///
/// A NaN component gives NaN, which is the right answer rather than an
/// accident: NaN fails every threshold comparison, so a corrupt vector drops
/// its pairs instead of ranking them, and [`find_twins`] orders with
/// `total_cmp`, so one can never destabilize the sort.
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

/// A pair ordered the way the result is: greater is closer to the front, so
/// the highest cosine wins and an exact tie goes to the lower `(a, b)`.
///
/// Wrapping [`TwinPair`] rather than ordering it directly keeps the public
/// type a plain record: the ranking is this module's retention policy, not a
/// property of a pair. `Eq` is sound here because a NaN cosine never reaches
/// the heap, having failed the threshold comparison first.
#[derive(Debug, Clone, PartialEq)]
struct Ranked(TwinPair);

impl Eq for Ranked {}

impl Ord for Ranked {
    fn cmp(&self, other: &Ranked) -> std::cmp::Ordering {
        self.0
            .cosine
            .total_cmp(&other.0.cosine)
            .then_with(|| (other.0.a, other.0.b).cmp(&(self.0.a, self.0.b)))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Ranked) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The best pairs of present vectors at or above `options.twin_threshold`,
/// closest first. An absent vector (`None`) takes part in nothing; the indices
/// in the result index `vectors`, so a caller keeps its own alignment.
///
/// At most `options.max_twin_pairs` pairs come back, and the guard discards
/// from the **bottom**: what is dropped is always further from the front than
/// what is kept, so a bounded run is a prefix of the unbounded one and the
/// caller's queue is unchanged. That is why no truncation line is owed for it,
/// where the vector cap - which skips the pass whole - reports one.
pub fn find_twins(vectors: &[Option<&[f32]>], options: &SweepOptions) -> TwinPairs {
    // Carrying the slice alongside its index keeps the `Option` out of the
    // inner loop, which runs up to `max_twin_vectors` squared over two times.
    let present: Vec<(usize, &[f32])> = vectors
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|slice| (i, slice)))
        .collect();
    let mut out = TwinPairs {
        compared: present.len(),
        ..TwinPairs::default()
    };
    if present.len() > options.max_twin_vectors {
        out.capped = true;
        return out;
    }

    // A min-heap on rank, so the weakest retained pair is always the one at
    // the top and the one a better pair evicts.
    let mut best: BinaryHeap<Reverse<Ranked>> = BinaryHeap::new();
    for (pi, &(i, a)) in present.iter().enumerate() {
        for &(j, b) in &present[pi + 1..] {
            // A width mismatch is a model swap caught mid-backfill: the two
            // vectors live in different spaces, so there is no angle between
            // them to measure.
            if a.len() != b.len() {
                continue;
            }
            let sim = cosine(a, b);
            if sim < options.twin_threshold {
                continue;
            }
            let pair = Ranked(TwinPair {
                a: i,
                b: j,
                cosine: sim,
            });
            if best.len() < options.max_twin_pairs {
                best.push(Reverse(pair));
            } else if let Some(Reverse(weakest)) = best.peek()
                && pair > *weakest
            {
                best.pop();
                best.push(Reverse(pair));
            }
        }
    }

    // Ascending by `Reverse` is descending by rank, which is the order the
    // caller takes its findings off the front of.
    out.pairs = best
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse(Ranked(pair))| pair)
        .collect();
    out
}
