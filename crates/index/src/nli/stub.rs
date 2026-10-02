//! A scorer that answers from a fixed table, for the pipeline tests in this
//! crate and in the engine and service crates. Public rather than
//! `#[cfg(test)]` because those crates' tests need it and the index crate has
//! no test-support feature; it holds no model and costs nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::ContradictionScorer;
use super::candidates::fold;
use crate::error::Result;

/// Answers each ordered pair from a table keyed by the folded texts, and
/// `default` for anything the table does not name.
pub struct StubScorer {
    repo: String,
    default: f32,
    table: HashMap<(String, String), f32>,
    calls: AtomicUsize,
    forwards: AtomicUsize,
}

impl StubScorer {
    /// A stub that stores against `repo` and scores unknown pairs `default`.
    pub fn new(repo: &str, default: f32) -> StubScorer {
        StubScorer {
            repo: repo.to_string(),
            default,
            table: HashMap::new(),
            calls: AtomicUsize::new(0),
            forwards: AtomicUsize::new(0),
        }
    }

    /// Score `a` against `b` as `p` in both reading orders.
    pub fn with(self, a: &str, b: &str, p: f32) -> StubScorer {
        self.with_directional(a, b, p).with_directional(b, a, p)
    }

    /// Score `premise` against `hypothesis` as `p` in that order only.
    pub fn with_directional(mut self, premise: &str, hypothesis: &str, p: f32) -> StubScorer {
        self.table.insert((fold(premise), fold(hypothesis)), p);
        self
    }

    /// How many times [`ContradictionScorer::score`] ran.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    /// How many ordered pairs it scored in total.
    pub fn forwards(&self) -> usize {
        self.forwards.load(Ordering::Relaxed)
    }
}

impl ContradictionScorer for StubScorer {
    fn score(&self, pairs: &[(String, String)]) -> Result<Vec<f32>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.forwards.fetch_add(pairs.len(), Ordering::Relaxed);
        Ok(pairs
            .iter()
            .map(|(p, h)| *self.table.get(&(fold(p), fold(h))).unwrap_or(&self.default))
            .collect())
    }

    fn model_repo(&self) -> &str {
        &self.repo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_answers_by_order_and_counts() {
        let s = StubScorer::new("repo/x", 0.1)
            .with("Node 18", "Node 20", 0.93)
            .with_directional("A", "B", 0.8);
        let out = s
            .score(&[
                ("Node 20".into(), "Node  18".into()),
                ("A".into(), "B".into()),
                ("B".into(), "A".into()),
            ])
            .unwrap();
        assert_eq!(out, vec![0.93, 0.8, 0.1]);
        assert_eq!((s.calls(), s.forwards()), (1, 3));
        assert_eq!(s.model_repo(), "repo/x");
    }
}
