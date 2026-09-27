//! The contradiction check's model side: which NLI model a profile runs, the
//! pure rules that pick and pair what gets scored, the scorer seam and a stub
//! for tests. The daemon pass that drives it lives in the engine; the sweep
//! that reads its stored output lives in [`crate::sweep`].
//!
//! Everything here but the candle loader is compiled into every build, so the
//! profile table, status and doctor behave the same with and without the
//! `local-embeddings` feature.

pub mod candidates;
#[cfg(feature = "local-embeddings")]
pub mod local;
pub mod models;
pub mod period;
mod stub;

pub use candidates::{
    CandidateFacts, CandidatePair, Candidates, LinePair, contradiction_candidates, eligible, fold,
    line_pairs, max_related_pairs, observation_hash, pending_pairs, related_threshold, score_rows,
    scorer_inputs, windows_overlap,
};
pub use models::{
    CONTRADICTION_SETTING_VALUES, NLI_MODELS, NliArch, NliModel, NliProfile, nli_model,
    nli_model_by_repo, weights_cached,
};
pub use period::names_period;
pub use stub::StubScorer;

use crate::error::Result;

/// How many ordered pairs one scorer call takes.
pub const NLI_BATCH_SIZE: usize = 16;

/// Each line is cut to this many model tokens, so a pair and its separators
/// fit the 512-token window.
pub const MAX_LINE_TOKENS: usize = 254;

/// The line pairs one daemon pass scores before it yields; each costs two
/// forward passes, one per reading order.
pub const MAX_INFERENCES_PER_PASS: usize = 2000;

/// Whether this build carries the local NLI loader.
pub const LOCAL_NLI_AVAILABLE: bool = cfg!(feature = "local-embeddings");

/// What a build without the loader answers when a profile is asked for.
pub const NLI_FEATURE_MISSING: &str =
    "contradiction scoring needs the local-embeddings feature, which this build does not carry";

/// Scores ordered sentence pairs for contradiction. Synchronous: inference is
/// CPU work, and the caller runs it on a blocking thread.
pub trait ContradictionScorer: Send + Sync {
    /// The softmax contradiction probability of each `(premise, hypothesis)`
    /// pair, in order. The caller batches by [`NLI_BATCH_SIZE`].
    fn score(&self, pairs: &[(String, String)]) -> Result<Vec<f32>>;

    /// The Hugging Face repository the scores are stored against.
    fn model_repo(&self) -> &str;
}

/// The classifier row that means "contradiction", read from the checkpoint's
/// `id2label` and never assumed: label order differs between checkpoints. A
/// checkpoint without the label (a binary entailment model) is refused with
/// its label list.
pub(crate) fn contradiction_index(
    id2label: &std::collections::BTreeMap<u32, String>,
) -> Result<usize> {
    match id2label
        .iter()
        .find(|(_, l)| l.eq_ignore_ascii_case("contradiction"))
    {
        Some((i, _)) => Ok(*i as usize),
        None => Err(crate::error::IndexError::Invalid(format!(
            "the checkpoint's labels carry no 'contradiction' entry: {}",
            id2label
                .iter()
                .map(|(i, l)| format!("{i}={l}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Load `model`, downloading it on first use. A build without the
/// `local-embeddings` feature refuses with [`NLI_FEATURE_MISSING`].
#[cfg(feature = "local-embeddings")]
pub async fn load_scorer(
    model: &'static NliModel,
) -> Result<std::sync::Arc<dyn ContradictionScorer>> {
    Ok(std::sync::Arc::new(local::LocalNli::load(model).await?))
}

/// Load `model`, downloading it on first use. A build without the
/// `local-embeddings` feature refuses with [`NLI_FEATURE_MISSING`].
#[cfg(not(feature = "local-embeddings"))]
pub async fn load_scorer(
    _model: &'static NliModel,
) -> Result<std::sync::Arc<dyn ContradictionScorer>> {
    Err(crate::error::IndexError::Unsupported(
        NLI_FEATURE_MISSING.to_string(),
    ))
}

/// How the two reading orders of a line pair combine into one score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderAggregation {
    /// The mean of both orders.
    Mean,
    /// The lower of both orders: a true contradiction is symmetric, and a
    /// one-sided high score often means one line is only more specific.
    Min,
}

impl OrderAggregation {
    /// The combined score of `ab` (a as premise) and `ba` (b as premise).
    pub fn combine(self, ab: f32, ba: f32) -> f32 {
        match self {
            OrderAggregation::Mean => (ab + ba) / 2.0,
            OrderAggregation::Min => ab.min(ba),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contradiction_label_is_read_never_assumed() {
        let labels: std::collections::BTreeMap<u32, String> = [
            (0, "entailment".to_string()),
            (1, "neutral".to_string()),
            (2, "contradiction".to_string()),
        ]
        .into();
        assert_eq!(contradiction_index(&labels).unwrap(), 2);
        let shouting: std::collections::BTreeMap<u32, String> = [
            (0, "CONTRADICTION".to_string()),
            (1, "ENTAILMENT".to_string()),
            (2, "NEUTRAL".to_string()),
        ]
        .into();
        assert_eq!(
            contradiction_index(&shouting).unwrap(),
            0,
            "case-insensitive"
        );
        let binary: std::collections::BTreeMap<u32, String> = [
            (0, "entailment".to_string()),
            (1, "not_entailment".to_string()),
        ]
        .into();
        let err = contradiction_index(&binary).unwrap_err().to_string();
        assert!(
            err.contains("0=entailment, 1=not_entailment"),
            "the refusal lists the labels: {err}"
        );
    }

    #[cfg(not(feature = "local-embeddings"))]
    #[tokio::test]
    async fn a_build_without_the_feature_refuses_every_profile() {
        for model in &NLI_MODELS {
            let err = load_scorer(model).await.err().expect("refused").to_string();
            assert!(err.ends_with(NLI_FEATURE_MISSING), "{err}");
        }
    }

    #[test]
    fn the_two_aggregations_combine_both_orders() {
        assert!((OrderAggregation::Mean.combine(0.9, 0.5) - 0.7).abs() < 1e-6);
        assert_eq!(OrderAggregation::Min.combine(0.9, 0.5), 0.5);
    }
}
