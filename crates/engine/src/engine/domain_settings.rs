//! `configure` with a domain: the view of how one domain behaves (its
//! MANIFEST policy keys and sections, its `.crystalline.yaml` rule
//! overrides), and the writes an agent makes to it.

use super::*;

use crystalline_core::manifest_view::{PolicyRow, manifest_facts, policy_rows};
use crystalline_core::verify::{
    ConfigEdit, DOMAIN_CONFIG_FILE, VERIFY_RULES, edit_domain_config, is_severity_word,
    load_domain_config, verify_rule,
};

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

/// One change to a MANIFEST policy key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyEdit {
    /// Declare `key: value` in the frontmatter.
    Set {
        /// The policy key.
        key: String,
        /// The value it takes.
        value: String,
    },
    /// Remove the declared key, so its default applies.
    Unset {
        /// The policy key.
        key: String,
    },
}

impl PolicyEdit {
    /// The key this edit is about.
    pub fn key(&self) -> &str {
        match self {
            PolicyEdit::Set { key, .. } | PolicyEdit::Unset { key } => key,
        }
    }
}

/// The registry row `edit` names, once the key and the value are ones it
/// takes. The texts are the ones `set_manifest_policies` has always answered.
fn checked_policy(domain: &str, edit: &PolicyEdit) -> Result<&'static crystalline_core::PolicyKey> {
    let registry = crystalline_core::policy_registry();
    let key = edit.key();
    let Some(spec) = registry.iter().find(|spec| spec.key == key) else {
        let known: Vec<&str> = registry.iter().map(|spec| spec.key).collect();
        return Err(EngineError::Invalid(format!(
            "`{key}` is not a MANIFEST policy; the policy keys are {}",
            known.join(", ")
        )));
    };
    if spec.kind == crystalline_core::PolicyKind::Text {
        return Err(EngineError::Invalid(format!(
            "`{key}` changes through a rename, which also moves this machine's name and rewrites links: use Rename domain on the domain page or `crystalline domain rename {domain} <new>`"
        )));
    }
    if let PolicyEdit::Set { value, .. } = edit
        && !spec.accepts(value)
    {
        return Err(EngineError::Invalid(format!(
            "`{key}: {value}` is not a value `{key}` takes; write one of {}",
            spec.values.join(", ")
        )));
    }
    Ok(spec)
}

impl Engine {
    /// Who may change a domain's policy keys and rule overrides: the
    /// domain's owner or an instance admin, and on the open tier (anonymous
    /// with `auth.mcp` off) whoever may see the domain, which is exactly
    /// where `edit_engram` writes the same frontmatter (`refuse_unwritable`
    /// in the MCP server). A reviewing domain still refuses the open tier
    /// further in, in `DomainView::for_write`, as it refuses its edits.
    ///
    /// Its own gate rather than a looser `require_domain_owner_refusing`:
    /// ending a domain and reading who drafts in it refuse an anonymous
    /// caller outright, and they keep doing so. REST never reaches the open
    /// branch, because it asks for an account first.
    pub(crate) async fn require_policy_writer(
        &self,
        domain: &str,
        key: &str,
        scope: &crate::scope::Scope,
    ) -> Result<()> {
        if matches!(scope, crate::scope::Scope::Anonymous) && !self.auth_mcp() {
            return self.require_domain(domain, scope).await;
        }
        self.require_domain_owner_refusing(
            domain,
            scope,
            EngineError::Forbidden(format!(
                "only the owner of '{domain}' or an instance admin may change `{key}`"
            )),
        )
        .await
    }

    /// Set or remove MANIFEST policy keys through the edit path an engram
    /// edit takes; see `Engine::set_manifest_policies`, which is this with
    /// sets only. Every edit is checked before the first is written.
    pub async fn edit_manifest_policies(
        &self,
        domain: &str,
        edits: &[PolicyEdit],
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if edits.is_empty() {
            return Err(EngineError::Invalid(
                "no policy named: send an object of at least one MANIFEST policy key to its value"
                    .to_string(),
            ));
        }
        for edit in edits {
            let spec = checked_policy(domain, edit)?;
            if spec.changed_by == crystalline_core::PolicyRole::Owner {
                self.require_policy_writer(domain, edit.key(), scope)
                    .await?;
            }
        }
        let view = DomainView::for_write(self, domain, scope).await?;
        let overlay = view.actor().map(str::to_string);
        let actor = self.actor_for(None, overlay.as_deref());
        let (desc, source) = view.resolve("manifest").await?;
        let edits: Vec<PolicyEdit> = edits.to_vec();
        self.apply_source_edit(
            &desc,
            &source,
            &view,
            None,
            &actor,
            None,
            scope,
            move |current| {
                let mut out = current.to_string();
                for edit in &edits {
                    out = match edit {
                        PolicyEdit::Set { key, value } => set_frontmatter_field(&out, key, value),
                        PolicyEdit::Unset { key } => remove_frontmatter_field(&out, key),
                    };
                }
                Ok(out)
            },
        )
        .await?;
        self.refresh_routing_cache().await;
        let markdown = match overlay.as_deref() {
            None => self.manifest_markdown(domain).await?,
            Some(who) => {
                let store = self.store.lock().await;
                store
                    .overlay_entry(desc.domain_id, who, &desc.path)
                    .await?
                    .map(|row| row.content)
                    .ok_or_else(|| {
                        EngineError::Internal(
                            "the MANIFEST draft was written and cannot be read back".to_string(),
                        )
                    })?
            }
        };
        Ok(json!({
            "domain": domain,
            "markdown": markdown,
            "draft": overlay.is_some(),
        }))
    }
}

/// The line an answer carries when the YAML edit lost comments.
const YAML_COMMENTS_DROPPED: &str =
    "The edit kept every key of .crystalline.yaml but not its comments.";

/// Why a rule override cannot be set in a reviewing domain.
const REVIEW_NO_RULES: &str = "This domain reviews changes before they land, and .crystalline.yaml is not an engram, so a rule override cannot be a draft. Change it in the domain's files instead.";

/// Why a token budget other than a positive number is refused.
const TOKEN_BUDGET_POSITIVE: &str = "token_budget takes a positive whole number of tokens. To turn the size rule off, set rules.Q002: off.";

/// What one key of a configure call with a domain is.
enum DomainKey {
    /// A MANIFEST policy key.
    Policy,
    /// `rules.<id>`, the id as verify matches it.
    Rule(String),
    /// `token_budget`.
    TokenBudget,
}

/// Which part of the domain `key` addresses, or the refusal that says what
/// the call may name instead.
fn classify_domain_key(key: &str) -> Result<DomainKey> {
    let registry = crystalline_core::policy_registry();
    if registry.iter().any(|spec| spec.key == key) {
        return Ok(DomainKey::Policy);
    }
    if key == "token_budget" {
        return Ok(DomainKey::TokenBudget);
    }
    if let Some(rule) = key.strip_prefix("rules.") {
        let id = rule.trim().to_ascii_uppercase();
        if verify_rule(&id).is_some() {
            return Ok(DomainKey::Rule(id));
        }
        return Err(EngineError::Invalid(format!(
            "`{rule}` is not a verify rule id. configure with domain lists every rule under rules, in catalog."
        )));
    }
    if crate::settings::is_known_key(key) {
        return Err(EngineError::Invalid(format!(
            "`{key}` is an instance setting, and this call names a domain. Set instance settings in a configure call without domain."
        )));
    }
    let keys: Vec<&str> = registry.iter().map(|spec| spec.key).collect();
    Err(EngineError::Invalid(format!(
        "`{key}` is not a domain setting. With domain, set and unset take the MANIFEST policy keys ({}), rules.<rule id> and token_budget.",
        keys.join(", ")
    )))
}

impl Engine {
    /// Change `domain`'s policy keys and rule overrides: `set` and `unset`
    /// of a configure call that names it. Every key is checked, and the
    /// `.crystalline.yaml` read and edited in memory, before anything is
    /// written, so one bad key writes nothing. The policy keys go through
    /// [`Engine::edit_manifest_policies`] (a draft in a reviewing domain);
    /// the rule overrides through [`edit_domain_config`], refused on a
    /// virtual domain and in a reviewing one. Both pass
    /// [`Engine::require_policy_writer`]. Answers the fresh view, with
    /// `draft: true` when the policy edit became a draft and `note` when the
    /// file lost its comments. Nothing is shared: on a team domain both files
    /// become unshared local changes.
    ///
    /// A key only an instance admin may change ([`crystalline_core::PolicyRole::Admin`])
    /// is not checked here, since a scope cannot say whether it administers
    /// the instance on every surface: the surface checks it, as REST and MCP do.
    pub async fn change_domain_settings(
        &self,
        domain: &str,
        set: &BTreeMap<String, String>,
        unset: &[String],
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.require_domain(domain, scope).await?;
        let mut policy_edits: Vec<PolicyEdit> = Vec::new();
        let mut config_edits: Vec<ConfigEdit> = Vec::new();
        for (key, value) in set {
            match classify_domain_key(key)? {
                DomainKey::Policy => policy_edits.push(PolicyEdit::Set {
                    key: key.clone(),
                    value: value.clone(),
                }),
                DomainKey::Rule(rule) => {
                    if !is_severity_word(value) {
                        return Err(EngineError::Invalid(format!(
                            "`{value}` is not a severity for {rule}; write off, error, warning or info"
                        )));
                    }
                    config_edits.push(ConfigEdit::SetRule {
                        rule,
                        word: value.trim().to_string(),
                    });
                }
                DomainKey::TokenBudget => match value.trim().parse::<usize>() {
                    Ok(budget) if budget > 0 => {
                        config_edits.push(ConfigEdit::SetTokenBudget(budget))
                    }
                    _ => return Err(EngineError::Invalid(TOKEN_BUDGET_POSITIVE.to_string())),
                },
            }
        }
        for key in unset {
            match classify_domain_key(key)? {
                DomainKey::Policy => policy_edits.push(PolicyEdit::Unset { key: key.clone() }),
                DomainKey::Rule(rule) => config_edits.push(ConfigEdit::UnsetRule { rule }),
                DomainKey::TokenBudget => config_edits.push(ConfigEdit::UnsetTokenBudget),
            }
        }
        for edit in &policy_edits {
            checked_policy(domain, edit)?;
        }
        // An edit that would leave the MANIFEST as it is writes nothing: on a
        // team domain a no-op write would be a local change nobody made. A
        // reviewing domain is left to the draft path, since the caller's
        // draft, not the folder, is what such an edit compares against.
        if !self.reviews_changes(domain) {
            let current = self.manifest_source(domain).await?;
            let facts = manifest_facts(&current.markdown, domain);
            let declared = |key: &str| {
                facts
                    .policies
                    .iter()
                    .find(|row| row.key == key)
                    .and_then(|row| row.declared.clone())
            };
            policy_edits.retain(|edit| match edit {
                PolicyEdit::Set { key, value } => declared(key).as_deref() != Some(value.as_str()),
                PolicyEdit::Unset { key } => declared(key).is_some(),
            });
        }
        let yaml = if config_edits.is_empty() {
            None
        } else {
            let ContentSource::File { root } = self.read_source(domain) else {
                return Err(EngineError::Invalid(format!(
                    "a virtual domain has no .crystalline.yaml, so '{domain}' takes no rule overrides"
                )));
            };
            if self.reviews_changes(domain) {
                return Err(EngineError::Refused(REVIEW_NO_RULES.to_string()));
            }
            let path = root.join(DOMAIN_CONFIG_FILE);
            let current = match std::fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(EngineError::Io {
                        path: path.display().to_string(),
                        source,
                    });
                }
            };
            let edited = edit_domain_config(current.as_deref(), &config_edits)
                .map_err(EngineError::Invalid)?;
            Some((path, edited))
        };
        // Policy keys and rule overrides share one gate; the key it names is
        // only the one the refusal mentions.
        if let Some(first) = set.keys().chain(unset.iter()).next() {
            self.require_policy_writer(domain, first, scope).await?;
        }

        let written = if policy_edits.is_empty() {
            None
        } else {
            Some(
                self.edit_manifest_policies(domain, &policy_edits, scope)
                    .await?,
            )
        };
        let mut note = None;
        if let Some((path, edited)) = yaml.filter(|(_, edited)| edited.changed) {
            // The write a rename waits for, like every engram write.
            self.refuse_shadowed(domain)?;
            let _writing = self.enter_write(domain).await?;
            match &edited.text {
                Some(text) => crystalline_core::config::save_bytes(&path, text.as_bytes())
                    .map_err(|e| {
                        EngineError::Internal(format!("writing {}: {e}", path.display()))
                    })?,
                None if path.exists() => {
                    std::fs::remove_file(&path).map_err(|source| EngineError::Io {
                        path: path.display().to_string(),
                        source,
                    })?
                }
                None => {}
            }
            if edited.dropped_comments {
                note = Some(YAML_COMMENTS_DROPPED);
            }
        }

        let (markdown, missing, draft) = match &written {
            Some(written) => (
                written["markdown"].as_str().unwrap_or_default().to_string(),
                false,
                written["draft"] == Value::Bool(true),
            ),
            None => {
                let source = self.manifest_source(domain).await?;
                (source.markdown, source.missing, false)
            }
        };
        let mut view = self
            .domain_settings_view(domain, &markdown, missing, scope)
            .await?;
        if draft {
            view["draft"] = json!(true);
        }
        if let Some(note) = note {
            view["note"] = json!(note);
        }
        Ok(view)
    }
}

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
        let hidden = self.hidden_for(scope).await?;
        let view = DomainView::for_read(self, domain, &hidden, scope)?;
        if view.actor().is_some() {
            // The caller's own view, as read_engram reads the MANIFEST: in a
            // reviewing domain their draft stands over the folder.
            match view.engram_text("manifest").await {
                Ok(text) => {
                    return self
                        .domain_settings_view(domain, &text.content, false, scope)
                        .await;
                }
                Err(EngineError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let source = self.manifest_source(domain).await?;
        self.domain_settings_view(domain, &source.markdown, source.missing, scope)
            .await
    }

    /// Whether `scope` may change `domain`'s policy keys through configure:
    /// the domain's owner or an instance admin; the open tier (anonymous with
    /// `auth.mcp` off) wherever `edit_engram` writes, which is never a
    /// reviewing domain, since a draft needs an identity; nobody on a
    /// read-only instance.
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
