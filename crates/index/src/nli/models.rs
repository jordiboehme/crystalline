//! The NLI model the contradiction check knows, one per profile.
//!
//! The user picks a profile with `evolve.contradictions`, never a model: which
//! checkpoint a profile runs is a release decision recorded here, so a later
//! release can move `full` to a better model without anybody touching a
//! setting, and every stored pair is rescored on that change by construction
//! (rows are keyed by the model's repository).
//!
//! Only `off` and `full` ship. Two earlier profiles existed on the
//! development branch only and were removed before release;
//! [`RETIRED_NLI_REPOS`] still names their checkpoints so an install that
//! downloaded them gets them pruned.

use std::path::Path;

use crate::embed::hub_dir_name;

/// One value of `evolve.contradictions` other than `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NliProfile {
    /// Multilingual, best quality, about 1.1 GB while scoring.
    Full,
}

impl NliProfile {
    /// Every profile, in the order the setting lists them.
    pub const ALL: [NliProfile; 1] = [NliProfile::Full];

    /// The setting value.
    pub fn as_str(self) -> &'static str {
        match self {
            NliProfile::Full => "full",
        }
    }

    /// The profile a setting value names. `None` for `off` and for anything
    /// this build does not know: a hand-edited value reads as off, never as a
    /// guess at a model (status and doctor say so through
    /// [`unknown_setting_note`]).
    pub fn from_setting(value: &str) -> Option<NliProfile> {
        let value = value.trim().to_ascii_lowercase();
        NliProfile::ALL.into_iter().find(|p| p.as_str() == value)
    }
}

/// Every value `evolve.contradictions` accepts, in the order the refusal
/// names them.
pub const CONTRADICTION_SETTING_VALUES: [&str; 2] = ["off", "full"];

/// The accepted values as a sentence fragment: `off or full`.
pub fn accepted_setting_values() -> String {
    CONTRADICTION_SETTING_VALUES.join(" or ")
}

/// What status and doctor say about a configured `evolve.contradictions`
/// this build does not know (a config file written by a development build
/// that still had a removed profile, or a hand edit). `None` for `off`, for
/// `full` and for an empty value. The check runs as off meanwhile, and the
/// note names the accepted values so the next step is obvious.
pub fn unknown_setting_note(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let known = trimmed.is_empty()
        || CONTRADICTION_SETTING_VALUES.contains(&trimmed.to_ascii_lowercase().as_str());
    if known {
        return None;
    }
    Some(format!(
        "evolve.contradictions is '{trimmed}', which is not a known value, so the check stays off; accepted values are {}. Fix it with: crystalline config set evolve.contradictions full (or off)",
        accepted_setting_values()
    ))
}

/// Checkpoints of NLI models this build no longer runs. They are never
/// loaded; the list exists only so the cache prune and doctor still
/// recognise the directories of an install that downloaded them on a
/// development build, and remove or report them instead of leaving them
/// behind as unknown files.
pub const RETIRED_NLI_REPOS: [&str; 2] = [
    "MoritzLaurer/multilingual-MiniLMv2-L12-mnli-xnli",
    "MoritzLaurer/DeBERTa-v3-base-mnli-fever-anli",
];

/// Whether `repo` is an NLI checkpoint this build runs or once ran.
pub fn is_nli_checkpoint(repo: &str) -> bool {
    let repo = repo.trim();
    NLI_MODELS.iter().any(|m| m.repo == repo) || RETIRED_NLI_REPOS.contains(&repo)
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
    /// The `model_type` the downloaded `config.json` must declare; the
    /// weights load into candle's DeBERTa-v2 sequence classifier.
    pub model_type: &'static str,
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

/// Every NLI model this build runs, one per profile.
pub const NLI_MODELS: [NliModel; 1] = [NliModel {
    profile: NliProfile::Full,
    id: "mdeberta-v3-base-xnli-2mil7",
    repo: "MoritzLaurer/mDeBERTa-v3-base-xnli-multilingual-nli-2mil7",
    model_type: "deberta-v2",
    languages: "multilingual",
    files: NLI_FILES,
    download_mb: 558,
    // Measured on the probes corpus (research/2026-09-30-contradiction-measurement.md);
    // to be confirmed on real knowledge. With the Min aggregation, 0.70 gave
    // precision 1.00 and recall 0.915 on 400 planted pairs.
    threshold: 0.70,
}];

/// The model a profile runs.
pub fn nli_model(profile: NliProfile) -> &'static NliModel {
    NLI_MODELS
        .iter()
        .find(|m| m.profile == profile)
        .expect("every profile has a model in the table")
}

/// The table entry for a repository or short id, `None` for anything else
/// (a retired checkpoint included: it is not a model this build runs).
pub fn nli_model_by_repo(repo: &str) -> Option<&'static NliModel> {
    let repo = repo.trim();
    NLI_MODELS.iter().find(|m| m.repo == repo || m.id == repo)
}

/// Whether the model's weights sit in the hf-hub cache under `models_dir`,
/// read from the directory alone (no network, no feature needed).
pub fn weights_cached(models_dir: &Path, model: &NliModel) -> bool {
    repo_weights_cached(models_dir, model.repo)
}

/// [`weights_cached`] for a bare repository, which a retired checkpoint is.
pub fn repo_weights_cached(models_dir: &Path, repo: &str) -> bool {
    let snapshots = models_dir.join(hub_dir_name(repo)).join("snapshots");
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
        assert_eq!(nli_model(NliProfile::Full).model_type, "deberta-v2");
        assert_eq!(NLI_MODELS.len(), NliProfile::ALL.len());
        for m in NLI_MODELS {
            assert!(m.files.contains(&"model.safetensors") && m.files.contains(&"tokenizer.json"));
            assert!(
                m.threshold > 0.5,
                "the finding line sits above the store floor"
            );
        }
    }

    #[test]
    fn only_off_and_full_ship() {
        assert_eq!(CONTRADICTION_SETTING_VALUES, ["off", "full"]);
        assert_eq!(accepted_setting_values(), "off or full");
    }

    #[test]
    fn the_setting_names_a_profile_or_nothing() {
        assert_eq!(NliProfile::from_setting("full"), Some(NliProfile::Full));
        assert_eq!(
            NliProfile::from_setting(" FULL "),
            Some(NliProfile::Full),
            "case and padding do not matter"
        );
        assert_eq!(NliProfile::from_setting("off"), None);
        assert_eq!(NliProfile::from_setting("mdeberta"), None, "never a guess");
        for removed in ["light", "english-only", "English-Only"] {
            assert_eq!(
                NliProfile::from_setting(removed),
                None,
                "{removed} no longer names a model"
            );
        }
        assert!(nli_model_by_repo("BAAI/bge-small-en-v1.5").is_none());
        assert!(
            nli_model_by_repo(RETIRED_NLI_REPOS[0]).is_none(),
            "a retired checkpoint is not a model this build runs"
        );
    }

    #[test]
    fn a_value_from_a_development_build_gets_a_note_naming_the_accepted_ones() {
        for known in ["off", "full", " Full ", ""] {
            assert_eq!(unknown_setting_note(known), None, "{known:?}");
        }
        for removed in ["light", "english-only"] {
            let note = unknown_setting_note(removed).unwrap();
            assert!(note.contains(&format!("'{removed}'")), "{note}");
            assert!(note.contains("not a known value"), "{note}");
            assert!(note.contains("off or full"), "{note}");
            assert!(
                note.contains("crystalline config set evolve.contradictions"),
                "{note}"
            );
        }
    }

    #[test]
    fn retired_checkpoints_are_still_recognised_as_nli_checkpoints() {
        assert!(is_nli_checkpoint(nli_model(NliProfile::Full).repo));
        for repo in RETIRED_NLI_REPOS {
            assert!(is_nli_checkpoint(repo), "{repo}");
        }
        assert!(!is_nli_checkpoint("BAAI/bge-small-en-v1.5"));
    }

    #[test]
    fn cached_weights_are_read_off_the_snapshot_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let m = nli_model(NliProfile::Full);
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
