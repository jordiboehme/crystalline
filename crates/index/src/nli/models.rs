//! The NLI models the contradiction check knows, one per profile.
//!
//! The user picks a profile with `evolve.contradictions`, never a model: which
//! checkpoint a profile runs is a release decision recorded here, so a later
//! release can move `full` or `light` to a better model without anybody
//! touching a setting, and every stored pair is rescored on that change by
//! construction (rows are keyed by the model's repository).

use std::path::Path;

use crate::embed::hub_dir_name;

/// One value of `evolve.contradictions` other than `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NliProfile {
    /// Multilingual, best quality, about 1.1 GB while scoring.
    Full,
    /// Multilingual on a small machine: about a quarter of the compute.
    Light,
    /// English-only knowledge, best English quality.
    EnglishOnly,
}

impl NliProfile {
    /// Every profile, in the order the setting lists them.
    pub const ALL: [NliProfile; 3] = [NliProfile::Full, NliProfile::Light, NliProfile::EnglishOnly];

    /// The setting value.
    pub fn as_str(self) -> &'static str {
        match self {
            NliProfile::Full => "full",
            NliProfile::Light => "light",
            NliProfile::EnglishOnly => "english-only",
        }
    }

    /// The profile a setting value names. `None` for `off` and for anything
    /// this build does not know: a hand-edited value reads as off, never as a
    /// guess at a model.
    pub fn from_setting(value: &str) -> Option<NliProfile> {
        let value = value.trim().to_ascii_lowercase();
        NliProfile::ALL.into_iter().find(|p| p.as_str() == value)
    }
}

/// Every value `evolve.contradictions` accepts, in the order the refusal
/// names them.
pub const CONTRADICTION_SETTING_VALUES: [&str; 4] = ["off", "full", "light", "english-only"];

/// The classifier family a checkpoint's weights load into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NliArch {
    /// candle's `DebertaV2SeqClassificationModel` (`model_type: "deberta-v2"`).
    DebertaV2,
    /// candle's `XLMRobertaForSequenceClassification` (`model_type: "xlm-roberta"`).
    XlmRoberta,
}

impl NliArch {
    /// The `model_type` the downloaded `config.json` must declare.
    pub fn model_type(self) -> &'static str {
        match self {
            NliArch::DebertaV2 => "deberta-v2",
            NliArch::XlmRoberta => "xlm-roberta",
        }
    }
}

/// One NLI checkpoint and the profile that runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NliModel {
    /// The profile that selects it.
    pub profile: NliProfile,
    /// The short id status and findings name it by.
    pub id: &'static str,
    /// The Hugging Face repository, the key every stored row carries.
    pub repo: &'static str,
    /// Which classifier the weights load into.
    pub architecture: NliArch,
    /// `multilingual` or `English`, for status and the docs.
    pub languages: &'static str,
    /// The files the downloader fetches, in fetch order.
    pub files: &'static [&'static str],
    /// The approximate first-use download, in megabytes.
    pub download_mb: u64,
    /// The `V302` finding line for this model's aggregated score.
    pub threshold: f32,
}

const NLI_FILES: &[&str] = &[
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "model.safetensors",
];

/// Every NLI model this build knows, one per profile.
pub const NLI_MODELS: [NliModel; 3] = [
    NliModel {
        profile: NliProfile::Full,
        id: "mdeberta-v3-base-xnli-2mil7",
        repo: "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7",
        architecture: NliArch::DebertaV2,
        languages: "multilingual",
        files: NLI_FILES,
        download_mb: 558,
        // Placeholder until the measurement sets it (plan Task 11).
        threshold: 0.85,
    },
    NliModel {
        profile: NliProfile::Light,
        id: "multilingual-minilmv2-l12-mnli-xnli",
        repo: "MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli",
        architecture: NliArch::XlmRoberta,
        languages: "multilingual",
        files: NLI_FILES,
        download_mb: 471,
        // Placeholder until the measurement sets it (plan Task 11).
        threshold: 0.85,
    },
    NliModel {
        profile: NliProfile::EnglishOnly,
        id: "deberta-v3-base-mnli-fever-anli",
        repo: "MoritzLaurer/DeBERTa-v3-base-mnli-fever-anli",
        architecture: NliArch::DebertaV2,
        languages: "English",
        files: NLI_FILES,
        download_mb: 369,
        // Placeholder until the measurement sets it (plan Task 11).
        threshold: 0.85,
    },
];

/// The model a profile runs.
pub fn nli_model(profile: NliProfile) -> &'static NliModel {
    NLI_MODELS
        .iter()
        .find(|m| m.profile == profile)
        .expect("every profile has a model in the table")
}

/// The table entry for a repository or short id, `None` for anything else.
pub fn nli_model_by_repo(repo: &str) -> Option<&'static NliModel> {
    let repo = repo.trim();
    NLI_MODELS.iter().find(|m| m.repo == repo || m.id == repo)
}

/// Whether the model's weights sit in the hf-hub cache under `models_dir`,
/// read from the directory alone (no network, no feature needed).
pub fn weights_cached(models_dir: &Path, model: &NliModel) -> bool {
    let snapshots = models_dir.join(hub_dir_name(model.repo)).join("snapshots");
    let Ok(entries) = std::fs::read_dir(snapshots) else {
        return false;
    };
    entries
        .flatten()
        .any(|e| e.path().join("model.safetensors").exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_maps_to_exactly_one_model() {
        assert_eq!(
            nli_model(NliProfile::Full).id,
            "mdeberta-v3-base-xnli-2mil7"
        );
        assert_eq!(
            nli_model(NliProfile::Light).architecture,
            NliArch::XlmRoberta
        );
        assert_eq!(nli_model(NliProfile::EnglishOnly).languages, "English");
        for m in NLI_MODELS {
            assert!(m.files.contains(&"model.safetensors") && m.files.contains(&"tokenizer.json"));
            assert!(
                m.threshold > 0.5,
                "the finding line sits above the store floor"
            );
        }
    }

    #[test]
    fn the_setting_names_a_profile_or_nothing() {
        assert_eq!(NliProfile::from_setting("full"), Some(NliProfile::Full));
        assert_eq!(
            NliProfile::from_setting(" English-Only "),
            Some(NliProfile::EnglishOnly)
        );
        assert_eq!(NliProfile::from_setting("off"), None);
        assert_eq!(NliProfile::from_setting("mdeberta"), None, "never a guess");
        assert_eq!(
            nli_model_by_repo("MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli")
                .map(|m| m.profile),
            Some(NliProfile::Light)
        );
        assert!(nli_model_by_repo("BAAI/bge-small-en-v1.5").is_none());
    }

    #[test]
    fn cached_weights_are_read_off_the_snapshot_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let m = nli_model(NliProfile::Light);
        assert!(!weights_cached(tmp.path(), m));
        let snap = tmp.path().join(hub_dir_name(m.repo)).join("snapshots/abc");
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("config.json"), b"{}").unwrap();
        assert!(
            !weights_cached(tmp.path(), m),
            "a config alone is not a download"
        );
        std::fs::write(snap.join("model.safetensors"), b"w").unwrap();
        assert!(weights_cached(tmp.path(), m));
    }
}
