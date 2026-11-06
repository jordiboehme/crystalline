//! `configure` with a domain: the view of how one domain behaves (its
//! MANIFEST policy keys and sections, its `.crystalline.yaml` rule
//! overrides), and the writes an agent makes to it.

use super::*;

use crystalline_core::manifest_view::{PolicyRow, manifest_facts, policy_rows};
use crystalline_core::reflection::ReflectionState;
use crystalline_core::verify::{
    ConfigEdit, DOMAIN_CONFIG_FILE, VERIFY_RULES, edit_domain_config, is_severity_word,
    load_domain_config, verify_rule,
};

/// What the rules part says about a virtual domain.
pub const VIRTUAL_NO_RULES: &str =
    "A virtual domain has no .crystalline.yaml, so it has no rule overrides.";

/// The `how` lines of the domain view, one per part.
pub const DOMAIN_SETTINGS_HOW: [&str; 4] = [
    "Set a policy: configure with domain and set, for example { \"sharing\": \"direct\" }. unset removes the key, so the default applies. Only a person switches read_only off, in the domain policies.",
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

/// Whether `edits` is exactly the owner's switch-off: `read_only` set to
/// `off`, or `read_only` removed so its default (`off`) applies, and nothing
/// else in the same call (decision D4, spec A-1).
pub(crate) fn is_switch_off(edits: &[PolicyEdit]) -> bool {
    match edits {
        [PolicyEdit::Set { key, value }] => {
            key == crystalline_core::READ_ONLY_KEY
                && value == crystalline_core::ReadOnly::Off.as_str()
        }
        [PolicyEdit::Unset { key }] => key == crystalline_core::READ_ONLY_KEY,
        _ => false,
    }
}

/// Whether `edits` is exactly the switch-on of `read_only`: one `set
/// read_only: on`, nothing else.
pub(crate) fn is_switch_on(edits: &[PolicyEdit]) -> bool {
    matches!(
        edits,
        [PolicyEdit::Set { key, value }]
            if key == crystalline_core::READ_ONLY_KEY
                && value == crystalline_core::ReadOnly::On.as_str()
    )
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
    // The one text key a person may remove: removing the evolve home is how
    // it moves, since the next scheduled evolve run where it was removed
    // claims it again. Setting it stays the engine's, so a home is always an
    // address the instance read from its own settings.
    if key == crystalline_core::EVOLVE_HOME_KEY {
        return match edit {
            PolicyEdit::Unset { .. } => Ok(spec),
            PolicyEdit::Set { .. } => Err(EngineError::Invalid(format!(
                "`{key}` is filled in by the first scheduled evolve run at an instance with a public address; it is not set by hand. To move the home, remove it at the instance that should run evolve (unset {key}), then run evolve there. On a domain that reviews changes the removal is a draft, and the old home holds until its proposal is merged and pulled here"
            ))),
        };
    }
    if spec.kind == crystalline_core::PolicyKind::Text {
        return Err(EngineError::Invalid(
            if key == crystalline_core::REFLECTION_HOME_KEY {
                format!(
                    "`{key}` is filled in when Reflection is switched on for a domain connected to GitHub; it is not set by hand"
                )
            } else if key == crystalline_core::TEAM_KEY {
                format!(
                    "`{key}` is added by the next share from an instance connected to GitHub (through the proposal on a domain that reviews changes); it is not set by hand"
                )
            } else {
                format!(
                    "`{key}` changes through a rename, which also moves this machine's name and rewrites links: use Rename domain on the domain page or `crystalline domain rename {domain} <new>`"
                )
            },
        ));
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
    /// Except `read_only`, which only the owner or an instance admin
    /// switches, on every tier.
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
        // `read_only` is the owner's on every tier (spec A-1): the open tier
        // keeps the other keys where edit_engram writes, never this one.
        // Not on a domain GitHub governs, where the open tier is decided as
        // `github.agent_identity` (or as anonymous without one).
        if key != crystalline_core::READ_ONLY_KEY && self.open_tier_carve_out(scope, domain) {
            return self.require_domain(domain, scope).await;
        }
        self.require_domain(domain, scope).await?;
        let scope = &self.scope_on(scope, domain);
        self.require_domain_owner_refusing(domain, scope, Self::owner_key_refusal(domain, key))
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
        // A read-only domain takes one change: its owner switching the flag
        // off, alone (`read_only: off`, or removing the key). Anything else is
        // the refusal every write gets. Over MCP even the switch-off is
        // refused, by the MCP handlers and not here: Fluid's policies card and
        // REST PATCH reach this function with the same scope, and a person
        // must be able to switch it off there (spec A-1). The owner check for
        // `read_only` ran in the loop above.
        let unlocking = self.domain_is_read_only(domain).await;
        if unlocking && !is_switch_off(edits) {
            self.require_domain(domain, scope).await?;
            return Err(EngineError::DomainReadOnly(read_only_sentence(domain)));
        }
        // The switch-off is a MANIFEST write as well, and an archived
        // repository takes no write from anybody, the owner included. The
        // unlock view below skips the archived check, so it is asked here.
        if unlocking && let Some(repo) = self.github_archived_repo(domain) {
            self.require_domain(domain, scope).await?;
            return Err(EngineError::DomainReadOnly(archived_sentence(
                domain, &repo,
            )));
        }
        // Reflection: a switch-on is screened against the text the edits
        // would produce, so the policy PATCH, configure and a raw MANIFEST
        // edit refuse with one sentence. A switch-off (`set reflection: off`,
        // or `unset reflection`) is never screened.
        let unsets_evolve_home = edits.iter().any(
            |edit| matches!(edit, PolicyEdit::Unset { key } if key == crystalline_core::EVOLVE_HOME_KEY),
        );
        let mut edits: Vec<PolicyEdit> = edits.to_vec();
        let wants_on = edits.iter().any(|edit| {
            matches!(edit, PolicyEdit::Set { key, value }
                if key == crystalline_core::REFLECTION_KEY
                    && value == crystalline_core::ReflectionSwitch::On.as_str())
        });
        let already_on =
            wants_on && self.reflection_switch(domain) == crystalline_core::ReflectionSwitch::On;
        // A switch-on of a domain that already says `on` but has no home
        // (state Homeless) refills the home instead of doing nothing.
        let refill =
            already_on && self.reflection_state(domain).await? == ReflectionState::Homeless;
        let switching_on = wants_on && (refill || !already_on);
        // A switch-off of a Reflection that is on: on a domain shared
        // through GitHub, its folder turns into engram content then.
        let switching_off = edits.iter().any(|edit| match edit {
            PolicyEdit::Set { key, value } => {
                key == crystalline_core::REFLECTION_KEY
                    && value == crystalline_core::ReflectionSwitch::Off.as_str()
            }
            PolicyEdit::Unset { key } => key == crystalline_core::REFLECTION_KEY,
        }) && self.reflection_switch(domain)
            == crystalline_core::ReflectionSwitch::On;
        if switching_on {
            let facts = self.switch_facts(domain).await?;
            // Before the MANIFEST is read: a virtual domain has none on disk
            // to read, and the refusal is the one `screen_switch` gives.
            if facts.virtual_domain {
                return Err(super::reflection::virtual_refusal());
            }
            if facts.origin
                && let Some(own) = facts.own_url.clone()
            {
                edits.push(PolicyEdit::Set {
                    key: crystalline_core::REFLECTION_HOME_KEY.to_string(),
                    value: own,
                });
            }
            let before = self.manifest_source(domain).await?.markdown;
            let after = apply_policy_edits(&before, &edits);
            if refill {
                super::reflection::screen_home(&after, &facts)?;
            } else {
                super::reflection::screen_switch(&before, &after, &facts)?;
            }
        }
        let view = if unlocking {
            DomainView::for_unlock(self, domain).await?
        } else {
            DomainView::for_write(self, domain, scope).await?
        };
        let overlay = view.actor().map(str::to_string);
        let actor = self.actor_for(None, overlay.as_deref());
        let (desc, source) = view.resolve("manifest").await?;
        // The pact is seeded before the switch is written: a seed that fails
        // leaves the switch off, so sending the switch again seeds it. A
        // draft seeds nothing, since the switch is not in effect yet.
        if switching_on && overlay.is_none() {
            self.seed_pact(domain).await?;
        }
        let switch_before = self.reflection_switch(domain);
        // A switch-off saves and closes the domain's reflection rooms first,
        // after every check that can still refuse this edit and while the
        // state is still on, so each room's last save lands in its page. A
        // refused switch-off never gets here and closes nobody's room. If the
        // write below fails after this, the rooms' text is in the pages
        // already; their editors reopen them.
        if switching_off && overlay.is_none() {
            self.close_reflection_rooms(domain).await;
        }
        self.apply_source_edit(
            &desc,
            &source,
            &view,
            None,
            &actor,
            None,
            scope,
            move |current| Ok(apply_policy_edits(current, &edits)),
        )
        .await?;
        self.refresh_routing_cache().await;
        // Removing the evolve home is how a person lets this instance claim
        // it again after the team declined its claim.
        if unsets_evolve_home {
            self.forget_declined_evolve_claim(domain);
        }
        // A draft moves no switch: the folder's MANIFEST decides.
        if overlay.is_none() {
            self.announce_switch_if_moved(domain, switch_before, change_label(scope));
        }
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
        let mut receipt = json!({
            "domain": domain,
            "markdown": markdown,
            "draft": overlay.is_some(),
        });
        // Said only where it is true: the switch is off in the folder (not a
        // draft) of a domain with a GitHub origin.
        if switching_off && overlay.is_none() && self.domain_has_origin(domain)? {
            receipt["note"] = json!(SWITCH_OFF_SHARED_NOTE);
        }
        Ok(receipt)
    }
}

impl Engine {
    /// Forget that the team declined this instance's claim of the evolve
    /// home of `domain`, so its next scheduled evolve run claims it again.
    fn forget_declined_evolve_claim(&self, domain: &str) {
        if let Some(path) = self.evolve_claim_declined_path(domain)
            && path.is_file()
            && let Err(e) = std::fs::remove_file(&path)
        {
            tracing::warn!("forgetting the declined evolve home claim of '{domain}': {e}");
        }
    }

    /// Write `edits` into the MANIFEST of `domain` for the engine itself,
    /// through the edit path an engram edit takes and without the policy
    /// checks a caller's edit gets: the keys only the engine fills in
    /// (`evolve_home`) land this way. On a domain that reviews changes it is
    /// `scope`'s draft, like any other change of theirs, so it goes through
    /// the review. Answers whether it became a draft.
    pub(crate) async fn write_engine_policies(
        &self,
        domain: &str,
        edits: Vec<PolicyEdit>,
        scope: &crate::scope::Scope,
    ) -> Result<bool> {
        let view = DomainView::for_write(self, domain, scope).await?;
        let overlay = view.actor().map(str::to_string);
        let actor = self.actor_for(None, overlay.as_deref());
        let (desc, source) = view.resolve("manifest").await?;
        self.apply_source_edit(
            &desc,
            &source,
            &view,
            None,
            &actor,
            None,
            scope,
            move |current| Ok(apply_policy_edits(current, &edits)),
        )
        .await?;
        self.refresh_routing_cache().await;
        Ok(overlay.is_some())
    }
}

/// What a switch-off of a Reflection on a domain shared through GitHub says.
const SWITCH_OFF_SHARED_NOTE: &str = "Reflection is off now, so \"share changes\" offers the files in reflections/ as engram changes. Do not share changes while it is off. To move the Reflection's home, switch it on again here at once and share the MANIFEST change.";

/// The line an answer carries when the YAML edit lost comments.
const YAML_COMMENTS_DROPPED: &str =
    "The edit kept every key of .crystalline.yaml but not its comments.";

/// Why a rule override cannot be set in a reviewing domain.
const REVIEW_NO_RULES: &str = "This domain reviews changes before they land, and .crystalline.yaml is not an engram, so a rule override cannot be a draft. Change it in the domain's files instead.";

/// Why a token budget other than a positive number is refused.
const TOKEN_BUDGET_POSITIVE: &str = "token_budget takes a positive whole number of tokens. Two rules read it: verify's Q002 and the evolve sweep's V105. To turn Q002 off, set rules.Q002: off.";

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
    /// `Engine::require_policy_writer`. Answers the fresh view, with
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
        // Each key once: `rules.e007` and `rules.E007` are one key, and a key
        // both set and unset has no single meaning.
        let mut named = BTreeSet::new();
        let mut once = |kind: &DomainKey, key: &str| -> Result<()> {
            let target = match kind {
                DomainKey::Policy => key.to_string(),
                DomainKey::Rule(rule) => format!("rules.{rule}"),
                DomainKey::TokenBudget => "token_budget".to_string(),
            };
            if named.insert(target.clone()) {
                Ok(())
            } else {
                Err(EngineError::Invalid(format!(
                    "`{target}` is named twice in this call. Name each key once."
                )))
            }
        };
        for (key, value) in set {
            let kind = classify_domain_key(key)?;
            once(&kind, key)?;
            match kind {
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
            let kind = classify_domain_key(key)?;
            once(&kind, key)?;
            match kind {
                DomainKey::Policy => policy_edits.push(PolicyEdit::Unset { key: key.clone() }),
                DomainKey::Rule(rule) => config_edits.push(ConfigEdit::UnsetRule { rule }),
                DomainKey::TokenBudget => config_edits.push(ConfigEdit::UnsetTokenBudget),
            }
        }
        // Policy keys and rule overrides share one gate, asked before anything
        // about the domain's files is read, so a caller who may not change the
        // domain learns nothing about them. The key it names is only the one
        // the refusal mentions.
        if let Some(first) = set.keys().chain(unset.iter()).next() {
            self.require_policy_writer(domain, first, scope).await?;
        }
        for edit in &policy_edits {
            checked_policy(domain, edit)?;
        }
        // Switching the lock on where it is on already changes nothing: it
        // writes nothing and says so, rather than answering a refusal.
        if config_edits.is_empty()
            && is_switch_on(&policy_edits)
            && self.domain_is_read_only(domain).await
        {
            let mut view = self.domain_settings(domain, scope).await?;
            view["note"] = json!(format!("'{domain}' is already read-only"));
            return Ok(view);
        }
        // A read-only domain is decided for the whole call before anything is
        // compared, read or written (spec A-2): the owner's switch-off alone
        // passes, and a call that mixes it with anything else, a rule
        // override or another policy key, writes nothing at all.
        if self.domain_is_read_only(domain).await
            && !(config_edits.is_empty() && is_switch_off(&policy_edits))
        {
            return Err(EngineError::DomainReadOnly(read_only_sentence(domain)));
        }
        // An edit that would leave the MANIFEST as it is writes nothing: on a
        // team domain a no-op write would be a local change nobody made. A
        // reviewing domain is left to the draft path, since the caller's
        // draft, not the folder, is what such an edit compares against.
        //
        // An unset of `evolve_home` the folder no longer declares (the
        // withdrawn claim of a declined one) is not a write, but it is still
        // the person's word that this instance may claim the home again.
        if policy_edits.iter().any(
            |edit| matches!(edit, PolicyEdit::Unset { key } if key == crystalline_core::EVOLVE_HOME_KEY),
        ) {
            self.forget_declined_evolve_claim(domain);
        }
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
            // A switch-on of a Homeless domain is kept although the MANIFEST
            // says `on` already: it refills the home (R22). The state is
            // asked only when the call names the switch.
            let names_switch = policy_edits.iter().any(|edit| {
                matches!(edit, PolicyEdit::Set { key, .. } if key == crystalline_core::REFLECTION_KEY)
            });
            let homeless =
                names_switch && self.reflection_state(domain).await? == ReflectionState::Homeless;
            policy_edits.retain(|edit| match edit {
                PolicyEdit::Set { key, value } => {
                    declared(key).as_deref() != Some(value.as_str())
                        || (homeless && key == crystalline_core::REFLECTION_KEY)
                }
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
            if let Err(e) = self.write_domain_config(domain, &path, &edited).await {
                // The policy keys have landed by now, so the answer says so.
                return Err(match &written {
                    Some(written) => {
                        let keys: Vec<&str> = policy_edits.iter().map(PolicyEdit::key).collect();
                        half_applied(e, &keys, written["draft"] == Value::Bool(true))
                    }
                    None => e,
                });
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
        // The switch-off note first, then the YAML one, in one line.
        let notes: Vec<&str> = written
            .as_ref()
            .and_then(|written| written["note"].as_str())
            .into_iter()
            .chain(note)
            .collect();
        if !notes.is_empty() {
            view["note"] = json!(notes.join(" "));
        }
        Ok(view)
    }
}

/// A MANIFEST's text with the policy edits applied in order: the one fold
/// the switch screen and the write share.
fn apply_policy_edits(current: &str, edits: &[PolicyEdit]) -> String {
    let mut out = current.to_string();
    for edit in edits {
        out = match edit {
            PolicyEdit::Set { key, value } => set_frontmatter_field(&out, key, value),
            PolicyEdit::Unset { key } => remove_frontmatter_field(&out, key),
        };
    }
    out
}

/// The answer of a configure call whose policy keys landed and whose
/// `.crystalline.yaml` step then failed: the keys that landed, then why the
/// file was not changed. A conflict stays a conflict, so a caller that reads
/// the class knows to send the call again. Everything else is a refusal the
/// model reads, never an internal error.
fn half_applied(error: EngineError, keys: &[&str], draft: bool) -> EngineError {
    let how = if draft { " as a draft" } else { "" };
    let head = format!(
        "applied [{}]{how}; {DOMAIN_CONFIG_FILE} was not changed",
        keys.join(", ")
    );
    match error {
        EngineError::Conflict(text) => EngineError::Conflict(format!("{head}: {text}")),
        other => EngineError::Refused(format!("{head}: {other}")),
    }
}

/// A failed write of `.crystalline.yaml`: an IO error keeps its path, and
/// anything else is a refusal that says the file was not written.
fn config_write_error(path: &Path, error: crystalline_core::config::ConfigError) -> EngineError {
    match error {
        crystalline_core::config::ConfigError::Io { path, source } => {
            EngineError::Io { path, source }
        }
        other => EngineError::Refused(format!("{} was not written: {other}", path.display())),
    }
}

impl Engine {
    /// Write `edited` to the domain's `.crystalline.yaml` at `path`, or
    /// remove the file when no key is left: the write a rename waits for,
    /// like every engram write.
    async fn write_domain_config(
        &self,
        domain: &str,
        path: &Path,
        edited: &crystalline_core::verify::EditedConfig,
    ) -> Result<()> {
        self.refuse_shadowed(domain)?;
        let _writing = self.enter_write(domain).await?;
        // Its one caller decided the read-only question for the whole call;
        // this is the floor under any later caller, inside the ticket.
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        match &edited.text {
            Some(text) => crystalline_core::config::save_bytes(path, text.as_bytes())
                .map_err(|e| config_write_error(path, e)),
            None if path.exists() => std::fs::remove_file(path).map_err(|source| EngineError::Io {
                path: path.display().to_string(),
                source,
            }),
            None => Ok(()),
        }
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
                // The caller's own draft deletes the MANIFEST: their view has
                // none, so it shows as missing rather than as the folder's.
                Err(EngineError::NotFound(_)) => {
                    let (domain_id, _) = self.domain_source(domain).await?;
                    if view.deletes(domain_id, "MANIFEST.md").await? {
                        return self.domain_settings_view(domain, "", true, scope).await;
                    }
                }
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
        if self.open_tier_carve_out(scope, domain) {
            return !self.reviews_changes(domain);
        }
        // The open tier on a domain GitHub governs is decided as
        // `github.agent_identity`, or as anonymous without one.
        let scope = &self.scope_on(scope, domain);
        match scope {
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
        // On a read-only domain only `read_only` itself can change, and only
        // the owner's switch-off; `read_only` is never the open tier's.
        let locked = self.domain_is_read_only(domain).await;
        let may_own = may_change && !matches!(scope, crate::scope::Scope::Anonymous);
        let policies: Vec<Value> = rows
            .iter()
            .map(|row| {
                let may = if row.key == crystalline_core::READ_ONLY_KEY {
                    may_own
                } else {
                    may_change && !locked
                };
                policy_json(row, may, domain)
            })
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
    value["how"] = json!(if row.key == crystalline_core::REFLECTION_HOME_KEY {
        "filled in when Reflection is switched on for a domain connected to GitHub; never set by hand"
            .to_string()
    } else if row.key == crystalline_core::TEAM_KEY {
        "added by the next share from an instance connected to GitHub (through the proposal on a domain that reviews changes); never set by hand".to_string()
    } else if row.key == crystalline_core::EVOLVE_HOME_KEY {
        "filled in by the first scheduled evolve run at an instance with a public address; to move the home, unset it with configure at the instance that should run evolve (on a domain that reviews changes, it moves once that proposal is merged and pulled)".to_string()
    } else if text {
        rename_how(domain)
    } else if row.key == crystalline_core::READ_ONLY_KEY {
        "switch on with configure; a person switches it off in the domain policies".to_string()
    } else if row.key == crystalline_core::REFLECTION_KEY {
        "set with configure; switching on refuses while engrams live in reflections/".to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// T6-b: a conflict on the yaml step (a rename began between the two
    /// writes) stays a conflict, with the keys that did land in front.
    #[test]
    fn a_half_applied_conflict_keeps_its_class() {
        let error = half_applied(
            EngineError::Conflict("domain 'eng' is being renamed; send it again".to_string()),
            &["sharing"],
            false,
        );
        match error {
            EngineError::Conflict(text) => assert_eq!(
                text,
                "applied [sharing]; .crystalline.yaml was not changed: domain 'eng' is being renamed; send it again"
            ),
            other => panic!("expected a Conflict, got {other:?}"),
        }
    }

    /// Every other failure of the yaml step is a refusal the model reads,
    /// as 9724dca5 made it.
    #[test]
    fn a_half_applied_io_error_is_a_refusal() {
        let error = half_applied(
            EngineError::Io {
                path: "/d/.crystalline.yaml".to_string(),
                source: std::io::Error::other("disk full"),
            },
            &["sharing", "generated_indexes"],
            false,
        );
        match error {
            EngineError::Refused(text) => assert!(
                text.starts_with(
                    "applied [sharing, generated_indexes]; .crystalline.yaml was not changed: "
                ),
                "{text}"
            ),
            other => panic!("expected a Refused, got {other:?}"),
        }
    }

    /// T6-a: when the policy keys became a draft, the answer says so.
    #[test]
    fn a_half_applied_draft_says_as_a_draft() {
        let error = half_applied(EngineError::Refused("no".to_string()), &["sharing"], true);
        assert_eq!(
            error.to_string(),
            "applied [sharing] as a draft; .crystalline.yaml was not changed: no"
        );
    }

    /// T6-b: a failed write of .crystalline.yaml that is not an IO error is
    /// a refusal, never an internal error a client may hide.
    #[test]
    fn a_non_io_config_write_error_is_a_refusal() {
        let error = config_write_error(
            Path::new("/d/.crystalline.yaml"),
            crystalline_core::config::ConfigError::Yaml("bad".to_string()),
        );
        assert!(matches!(error, EngineError::Refused(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            "/d/.crystalline.yaml was not written: config yaml error: bad"
        );
        let io = config_write_error(
            Path::new("/d/.crystalline.yaml"),
            crystalline_core::config::ConfigError::Io {
                path: "/d/.crystalline.yaml".to_string(),
                source: std::io::Error::other("disk full"),
            },
        );
        assert!(matches!(io, EngineError::Io { .. }), "{io:?}");
    }
}
