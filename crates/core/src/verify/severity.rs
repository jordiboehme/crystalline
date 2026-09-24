//! Resolving a rule's effective severity: a domain's `.crystalline.yaml`
//! override wins outright (including turning a rule `off`); otherwise
//! `--strict` promotes a `Warning`-default rule to `Error`. A word the
//! override map holds that is not a severity is ignored here and reported by
//! the config loader as `M108`, so the rule keeps its default and `--strict`
//! still applies to it.

use crate::config::VerifyConfig;

use super::Severity;

/// A severity word from `.crystalline.yaml`: `Some(None)` is `off`,
/// `Some(Some(level))` a severity, and `None` a word verify does not know.
pub(crate) fn parse_word(word: &str) -> Option<Option<Severity>> {
    match word.trim().to_lowercase().as_str() {
        "off" => Some(None),
        "error" | "e" => Some(Some(Severity::Error)),
        "warning" | "warn" | "w" => Some(Some(Severity::Warning)),
        "info" | "i" => Some(Some(Severity::Info)),
        _ => None,
    }
}

pub(crate) fn resolve(
    rule: &str,
    default: Severity,
    cfg: Option<&VerifyConfig>,
    strict: bool,
) -> Option<Severity> {
    if let Some(cfg) = cfg
        && let Some(over) = cfg.rules.get(rule)
        && let Some(level) = parse_word(over)
    {
        return level;
    }
    if strict && default == Severity::Warning {
        return Some(Severity::Error);
    }
    Some(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(rule: &str, word: &str) -> VerifyConfig {
        let mut c = VerifyConfig::default();
        c.rules.insert(rule.to_string(), word.to_string());
        c
    }

    #[test]
    fn a_known_word_wins_over_strict() {
        assert_eq!(
            resolve("M101", Severity::Warning, Some(&cfg("M101", "info")), true),
            Some(Severity::Info)
        );
        assert_eq!(
            resolve("M101", Severity::Warning, Some(&cfg("M101", "off")), true),
            None
        );
    }

    #[test]
    fn an_unknown_word_keeps_the_default_and_the_strict_promotion() {
        let typo = cfg("M101", "warnig");
        assert_eq!(
            resolve("M101", Severity::Warning, Some(&typo), false),
            Some(Severity::Warning)
        );
        assert_eq!(
            resolve("M101", Severity::Warning, Some(&typo), true),
            Some(Severity::Error)
        );
        assert_eq!(
            resolve("M101", Severity::Error, Some(&cfg("M101", "of")), false),
            Some(Severity::Error)
        );
    }
}
