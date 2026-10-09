//! `configure` with a domain: the view of how one domain behaves (its
//! MANIFEST policy keys and sections, its `.crystalline.yaml` rule
//! overrides), and the writes an agent makes to it.

use super::*;

use crystalline_core::manifest_view::{PolicyRow, manifest_facts, policy_rows};
use crystalline_core::verify::{DOMAIN_CONFIG_FILE, VERIFY_RULES, load_domain_config, verify_rule};

/// What the rules part says about a virtual domain.
pub const VIRTUAL_NO_RULES: &str =
    "A virtual domain has no .crystalline.yaml, so it has no rule overrides.";

/// The `how` lines of the domain view, one per part.
pub const DOMAIN_SETTINGS_HOW: [&str; 4] = [
    "Set a policy: configure with domain and set, for example { \"sharing\": \"direct\" }. unset removes the key, so the default applies.",
    "Edit a section: read_engram the MANIFEST (identifier manifest), then edit_engram with replace_section or insert_after_section.",
    "Set a rule override: configure with domain and set { \"rules.<rule id>\": \"off\" } (off, error, warning or info), or { \"token_budget\": \"<tokens>\" }. unset removes it.",
    "Rename the domain: crystalline domain rename <old> <new>, or Rename domain on the domain page in Fluid.",
];

/// What an override row says about an id written in lower case.
const OVERRIDE_LOWER_CASE: &str = "Written in lower case, so verify does not match it and the override does nothing. Rule ids are upper case.";

/// What an override row says about an id no verify rule has.
const OVERRIDE_UNKNOWN: &str = "Not a verify rule id, so the override does nothing.";

/// How the `text` policy key changes.
fn rename_how(domain: &str) -> String {
    format!(
        "changes through a rename: crystalline domain rename {domain} <new>, or Rename domain on the domain page in Fluid"
    )
}

impl Engine {
    /// How `domain` behaves, for `scope`: every MANIFEST policy key with
    /// whether this caller may change it, every section that does something,
    /// the `.crystalline.yaml` rule overrides and one `how` line per part.
    ///
    /// A domain this caller may not see is the same `UnknownDomain` a read of
    /// it gets. A domain with no MANIFEST yet answers with the registry
    /// defaults and `manifest: "missing"`.
    pub async fn domain_settings(
        &self,
        domain: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.require_domain(domain, scope).await?;
        let source = self.manifest_source(domain).await?;
        self.domain_settings_view(domain, &source.markdown, source.missing, scope)
            .await
    }

    /// Whether `scope` may change `domain`'s policy keys and rule overrides
    /// through configure: the domain's owner or an instance admin; the open
    /// tier (anonymous with `auth.mcp` off) wherever `edit_engram` writes,
    /// which is never a reviewing domain, since a draft needs an identity;
    /// nobody on a read-only instance.
    pub(crate) async fn may_change_domain(
        &self,
        domain: &str,
        scope: &crate::scope::Scope,
    ) -> bool {
        if self.read_only {
            return false;
        }
        match scope {
            crate::scope::Scope::Anonymous if !self.auth_mcp() => !self.reviews_changes(domain),
            crate::scope::Scope::Anonymous => false,
            _ => matches!(
                self.domain_right(scope, domain).await,
                Ok(right) if right >= crate::scope::DomainRight::Own
            ),
        }
    }

    /// The view, built from `markdown` rather than read again: after a policy
    /// write in a reviewing domain the caller's draft is what they changed,
    /// and the folder still holds the old value.
    pub(crate) async fn domain_settings_view(
        &self,
        domain: &str,
        markdown: &str,
        missing: bool,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let facts = manifest_facts(markdown, domain);
        let may_change = self.may_change_domain(domain, scope).await;
        let rows: Vec<PolicyRow> = if facts.parsed {
            facts.policies.clone()
        } else {
            policy_rows(None, domain)
        };
        let policies: Vec<Value> = rows
            .iter()
            .map(|row| policy_json(row, may_change, domain))
            .collect();
        let sections: Vec<Value> = facts
            .sections
            .iter()
            .map(|section| {
                json!({
                    "name": section.name,
                    "present": section.present,
                    "required": section.required,
                    "meaning": section.meaning,
                    "how": format!("edit_engram on the MANIFEST (section ## {})", section.name),
                })
            })
            .collect();
        let manifest = if missing {
            "missing"
        } else if facts.parsed {
            "present"
        } else {
            "unreadable"
        };
        Ok(json!({
            "domain": domain,
            "manifest": manifest,
            "policies": policies,
            "sections": sections,
            "routing": facts.routing,
            "missing": facts.missing,
            "rules": self.rules_view(domain),
            "how": DOMAIN_SETTINGS_HOW,
        }))
    }

    /// The `.crystalline.yaml` part of the view, read through the loader
    /// verify uses, so its problems are the `M108` problems.
    fn rules_view(&self, domain: &str) -> Value {
        let ContentSource::File { root } = self.read_source(domain) else {
            return json!({ "available": false, "note": VIRTUAL_NO_RULES });
        };
        let file_present = root.join(DOMAIN_CONFIG_FILE).is_file();
        let load = load_domain_config(&root);
        let verify = load.config.verify.unwrap_or_default();
        let overrides: Vec<Value> = verify
            .rules
            .iter()
            .map(|(rule, word)| override_row(rule, word))
            .collect();
        let catalog: Vec<Value> = VERIFY_RULES
            .iter()
            .map(
                |rule| json!({ "rule": rule.id, "default": rule.default, "meaning": rule.summary }),
            )
            .collect();
        json!({
            "available": true,
            "file_present": file_present,
            "overrides": overrides,
            "token_budget": verify.token_budget,
            "token_budgets": verify.token_budgets,
            "required_files": verify.required_files,
            "problems": load.problems.iter().map(|p| p.message.clone()).collect::<Vec<_>>(),
            "catalog": catalog,
        })
    }
}

/// A policy row with this caller's `can_change` and the way it changes.
fn policy_json(row: &PolicyRow, may_change: bool, domain: &str) -> Value {
    let text = row.kind == crystalline_core::PolicyKind::Text.as_str();
    let mut value = serde_json::to_value(row).expect("a policy row serializes");
    value["can_change"] = json!(may_change && !text);
    value["how"] = json!(if text {
        rename_how(domain)
    } else {
        "set with configure".to_string()
    });
    value
}

/// One override as the file writes it, beside the rule's default and meaning.
fn override_row(rule: &str, word: &str) -> Value {
    if let Some(known) = verify_rule(rule) {
        return json!({ "rule": rule, "severity": word, "default": known.default, "meaning": known.summary });
    }
    match verify_rule(&rule.to_ascii_uppercase()) {
        Some(known) => {
            json!({ "rule": rule, "severity": word, "default": known.default, "meaning": OVERRIDE_LOWER_CASE })
        }
        None => {
            json!({ "rule": rule, "severity": word, "default": "unknown", "meaning": OVERRIDE_UNKNOWN })
        }
    }
}
