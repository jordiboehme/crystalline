//! Loading a domain's `.crystalline.yaml` for verify, together with what went
//! wrong reading it. The one loader `crystalline verify` and the engine's
//! `validate_engrams` share, so both report the same problems as rule `M108`
//! instead of one of them quietly running every rule at its default.
//!
//! A problem never fails a run on its own: `M108` is a warning, so only
//! `--strict` (the explicit "fail on anything" switch) turns a lost override
//! into a red build.

use std::path::Path;

use crate::config::DomainConfig;

use super::severity;

/// The per-domain config file, at the domain root.
pub const DOMAIN_CONFIG_FILE: &str = ".crystalline.yaml";

/// One thing wrong with a domain's `.crystalline.yaml`, as the `M108` message
/// says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    /// The finding's message: the file, what is wrong and what applies instead.
    pub message: String,
    /// What fixes it: the severity-word hint for an unknown word, or a
    /// YAML-syntax hint for a file that does not parse. `M108` always carries
    /// one; the two problem kinds just carry different ones, since a
    /// finding about broken YAML pointing at severity words is nonsense.
    pub fix: Option<String>,
}

/// A domain's config and the problems met loading it. A missing file is the
/// default config and no problem; a file that does not parse is the default
/// config and one problem.
#[derive(Debug, Clone, Default)]
pub struct DomainConfigLoad {
    /// The config to apply.
    pub config: DomainConfig,
    /// What went wrong reading it, one entry per `M108` finding.
    pub problems: Vec<ConfigProblem>,
}

/// Load `<root>/.crystalline.yaml`.
pub fn load_domain_config(root: &Path) -> DomainConfigLoad {
    let path = root.join(DOMAIN_CONFIG_FILE);
    if !path.is_file() {
        return DomainConfigLoad::default();
    }
    match crate::config::load_yaml::<DomainConfig>(&path) {
        Ok(config) => {
            let problems = config_problems(&config);
            DomainConfigLoad { config, problems }
        }
        Err(e) => DomainConfigLoad {
            config: DomainConfig::default(),
            problems: vec![ConfigProblem {
                message: format!(
                    "`{DOMAIN_CONFIG_FILE}` does not parse, so none of its verify settings \
                     apply and every rule runs at its default: {e}"
                ),
                fix: Some(format!("fix the YAML syntax of {DOMAIN_CONFIG_FILE}")),
            }],
        },
    }
}

/// The problems inside a config that parsed: one per rule override whose
/// value is not a severity word, naming the rule and the word.
pub fn config_problems(config: &DomainConfig) -> Vec<ConfigProblem> {
    let Some(verify) = &config.verify else {
        return Vec::new();
    };
    verify
        .rules
        .iter()
        .filter(|(_, word)| severity::parse_word(word).is_none())
        .map(|(rule, word)| ConfigProblem {
            message: format!(
                "`{DOMAIN_CONFIG_FILE}` sets {rule} to '{word}', which is not a severity \
                 (off, error, warning or info), so {rule} keeps its default"
            ),
            fix: Some("use off, error, warning or info for each rule".to_string()),
        })
        .collect()
}
