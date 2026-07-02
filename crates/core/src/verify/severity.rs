//! Resolving a rule's effective severity: a domain's `.crystalline.yaml`
//! override wins outright (including turning a rule `off`); otherwise
//! `--strict` promotes a `Warning`-default rule to `Error`.

use crate::config::VerifyConfig;

use super::Severity;

pub(crate) fn resolve(
    rule: &str,
    default: Severity,
    cfg: Option<&VerifyConfig>,
    strict: bool,
) -> Option<Severity> {
    if let Some(cfg) = cfg
        && let Some(over) = cfg.rules.get(rule)
    {
        return match over.trim().to_lowercase().as_str() {
            "off" => None,
            "error" | "e" => Some(Severity::Error),
            "warning" | "warn" | "w" => Some(Severity::Warning),
            "info" | "i" => Some(Severity::Info),
            // An unrecognized override value is ignored rather than
            // rejected outright; static verify never fails on a config
            // typo, it just falls back to the rule's own default.
            _ => Some(default),
        };
    }
    if strict && default == Severity::Warning {
        return Some(Severity::Error);
    }
    Some(default)
}
