use super::*;

impl Engine {
    // --- configure -------------------------------------------------------------

    /// Show, set or reset an agent-adjustable setting from the
    /// [`crate::settings`] registry. `show` takes only the config's read lock
    /// and is always allowed, even on a read-only instance; `set` and `unset`
    /// refuse with `EngineError::ReadOnly` on a read-only instance (config is
    /// frozen the same way the four content-mutating methods are), otherwise
    /// they validate and apply the change, persist the config file this engine
    /// was started with (or the default path) and update the in-memory config
    /// so a later read (including a concurrent one, once the write lock
    /// releases) sees it.
    ///
    /// A change that moves the MCP tool list is announced here, by
    /// `Engine::announce_a_moved_tool_list`, because this method is the one
    /// seam all three settings callers share.
    pub async fn configure(&self, action: &ConfigureAction) -> Result<Value> {
        match action {
            ConfigureAction::Show => {
                let views = {
                    let file = self.file_config.read().unwrap();
                    settings::snapshot(&*self.persisted_view(&file)?, &self.overlay)
                };
                let views =
                    settings::with_effective_share_identity(views, self.share_identity_effective());
                Ok(json!({ "settings": views }))
            }
            ConfigureAction::Set { key, value } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                let github_before = self.github_enabled();
                // The inner block scopes the lock guards, so they are released
                // before the announcement below awaits.
                let view = {
                    // Take the file-config write lock first to serialize against a
                    // concurrent configure call, so two tasks cannot both clone the
                    // old file and clobber each other's change. `persist_config` is
                    // synchronous (no .await), so holding the guard across it is
                    // safe. Lock order is always file_config then config.
                    let mut file_guard = self.file_config.write().unwrap();
                    let mut file = self.fresh_file_config(&file_guard)?;
                    self.apply_setting(&mut file, key, Some(value))?;
                    let view = self.setting_view_json(&file, key)?;
                    // The effective config is recomputed from the freshly saved
                    // file plus the overlay, so an env-overridden key keeps
                    // reading its env value even after the file value changes
                    // underneath it.
                    self.commit_config(&mut file_guard, file)?;
                    view
                };
                self.contradiction_setting_touched(key);
                self.announce_a_moved_tool_list(github_before).await;
                Ok(view)
            }
            ConfigureAction::Unset { key } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                let github_before = self.github_enabled();
                let view = {
                    // Same write-lock-first discipline and lock order as Set above.
                    let mut file_guard = self.file_config.write().unwrap();
                    let mut file = self.fresh_file_config(&file_guard)?;
                    self.apply_setting(&mut file, key, None)?;
                    let view = self.setting_view_json(&file, key)?;
                    self.commit_config(&mut file_guard, file)?;
                    view
                };
                self.contradiction_setting_touched(key);
                self.announce_a_moved_tool_list(github_before).await;
                Ok(view)
            }
        }
    }

    /// Announce a moved `tools/list` on every open subscription stream, when
    /// the write that just landed changed what `github.enabled` effectively
    /// reads.
    ///
    /// `github.enabled` gates the listing of the six GitHub collaboration
    /// tools (`crate::mcp`'s `hidden_collab_tool`), so a settings write that
    /// flips it is the one thing on this server that moves a tool list - and
    /// the one that owes an announcement. It lives on the engine rather than
    /// in the MCP handler because three callers write that setting and all
    /// three move every connected peer's list: the `configure` MCP tool,
    /// `crystalline config set` over the control socket (`crate::control`) and
    /// Fluid's Connect button through the REST API
    /// (`crate::rest::github_settings`'s `ensure_enabled`). All three go
    /// through [`Engine::configure`], so putting it here is what makes the
    /// notification unconditional on the route taken.
    ///
    /// The setting is read either side of the write rather than parsed out of
    /// the request: a key can be set to the value it already had, unset back
    /// to the default, or overridden by the environment, and only the
    /// effective setting decides what the next `tools/list` returns.
    ///
    /// It reaches subscribers only. MCP 2026-07-28 removed the unsolicited
    /// channel outright, so a legacy peer - which cannot subscribe at all - is
    /// told nothing and re-reads the list at its own discretion. See
    /// [`crate::subscribers`].
    async fn announce_a_moved_tool_list(&self, github_before: bool) {
        if self.github_enabled() != github_before {
            self.list_subscribers().notify_tool_list_changed().await;
        }
    }

    /// The just-applied setting's snapshot entry, as a JSON value, with a
    /// `note` field attached when [`settings::change_note`] has one (for
    /// example, a startup-effective key reminding the caller that a running
    /// daemon keeps its old value, or an env-overridden key reminding it that
    /// the saved value waits on the variable being removed). `file` is the
    /// freshly saved file config; the snapshot layers the overlay on top, so an
    /// env-overridden key reports its env value with `source: env`. `key` has
    /// already been validated against the registry by `apply`/`unset`, so it is
    /// always found.
    fn setting_view_json(&self, file: &GlobalConfig, key: &str) -> Result<Value> {
        // The config moves only after this answers, so the mode is worked out
        // from the file being saved.
        let view = self.persisted_view(file)?;
        let setting = self.overlay.apply(&view).github_share_identity_setting();
        Ok(settings::with_effective_share_identity(
            settings::snapshot(&view, &self.overlay),
            self.share_identity_given(setting),
        )
        .into_iter()
        .find(|v| v.key == key)
        .map(|v| {
            let mut value = serde_json::to_value(v).unwrap_or(Value::Null);
            if let Some(note) = settings::change_note(key, &self.overlay)
                && let Value::Object(map) = &mut value
            {
                map.insert("note".to_string(), Value::String(note));
            }
            value
        })
        .unwrap_or(Value::Null))
    }

    /// Set (`Some`) or unset (`None`) the setting `key` in `file`, with the
    /// pair rule of [`settings::apply_checked`]. In document mode a
    /// `database.*` key is refused (where the configuration lives is decided
    /// by that block, so it stays in config.yaml and the environment) and the
    /// pair rule is checked over the document merged with its base, the way
    /// the running server reads it.
    fn apply_setting(&self, file: &mut GlobalConfig, key: &str, value: Option<&str>) -> Result<()> {
        if !self.config_in_document() {
            match value {
                Some(value) => settings::apply_checked(file, &self.overlay, key, value)?,
                None => settings::unset_checked(file, &self.overlay, key)?,
            }
            return Ok(());
        }
        settings::refuse_database_key_in_document(key)
            .map_err(|e| EngineError::Refused(e.to_string()))?;
        let mut next = file.clone();
        match value {
            Some(value) => settings::apply(&mut next, key, value)?,
            None => settings::unset(&mut next, key)?,
        }
        settings::check_pair(&*self.persisted_view(&next)?, &self.overlay, key)?;
        *file = next;
        Ok(())
    }

    /// The file config a mutation starts from, read under the `file_config`
    /// write lock the caller holds: the file on disk as it stands right now,
    /// not the snapshot this engine took at startup.
    ///
    /// The file has other writers. The CLI's local `domain add` registers a
    /// domain by editing it from its own process, and an operator edits it by
    /// hand; neither reaches this engine's snapshot. A mutation that started
    /// from the snapshot persisted a copy of the file without whatever those
    /// writers had added, which is how a daemon-side `config set` used to drop
    /// a freshly added domain from the registry, and how `domain remove` of
    /// one answered "not registered" while naming it among the registered.
    /// Loading the file first makes every mutation the load-modify-save
    /// [`Engine::persist_config`] promises.
    ///
    /// The snapshot is the base only when there is no file to load: an engine
    /// built over a configuration never written to disk, which is what a
    /// one-shot command and many test engines are. A file that is there and
    /// does not parse is an error: saving the snapshot over it would throw
    /// away whatever somebody is editing by hand, and a daemon that did so
    /// put a good file back under a broken one `crystalline status` was about
    /// to report. The write is refused and the file stays as it is.
    ///
    /// In document mode the in-memory document is the truth (this engine is
    /// its only writer), so it is the answer and nothing is read.
    pub(super) fn fresh_file_config(&self, snapshot: &GlobalConfig) -> Result<GlobalConfig> {
        if self.config_in_document() {
            return Ok(snapshot.clone());
        }
        match self.config_file_path() {
            Some(path) if path.is_file() => overlay::load_file(&path).map_err(config_left_as_it_is),
            _ => Ok(snapshot.clone()),
        }
    }

    /// Refuse up front when the configuration file is there and does not
    /// parse, with the same error [`Engine::fresh_file_config`] answers.
    ///
    /// A verb whose last step writes the configuration (a rename, a
    /// subscribe, a folder `domain add`) asks this before its first side
    /// effect, so a broken file stops it before it moves, downloads or
    /// scaffolds anything instead of at the end, with the work half done.
    /// The write itself still loads the file again under the lock.
    pub(super) fn refuse_a_config_that_does_not_parse(&self) -> Result<()> {
        if self.config_in_document() {
            return Ok(());
        }
        match self.config_file_path() {
            Some(path) if path.is_file() => overlay::load_file(&path)
                .map(drop)
                .map_err(config_left_as_it_is),
            _ => Ok(()),
        }
    }

    /// Whether this engine keeps its configuration in a database document
    /// over a read-only `config.yaml` base.
    pub(crate) fn config_in_document(&self) -> bool {
        matches!(
            self.persisted,
            Some(crate::persisted::PersistedConfig::Document { .. })
        )
    }

    /// The persisted layers of `file` together, without the environment: the
    /// file itself in file mode, the base with `file` (the document) merged on
    /// top in document mode. What a check that must see every registered
    /// domain or setting reads.
    pub(crate) fn persisted_view<'a>(
        &self,
        file: &'a GlobalConfig,
    ) -> Result<std::borrow::Cow<'a, GlobalConfig>> {
        Ok(match &self.config_base {
            Some(base) => std::borrow::Cow::Owned(crate::persisted::merge(base, file)?),
            None => std::borrow::Cow::Borrowed(file),
        })
    }

    /// The effective configuration `file` gives: the environment overlay
    /// applied to [`Engine::persisted_view`]. In file mode exactly
    /// `overlay.apply(file)`, as it always was.
    pub(crate) fn effective_of(&self, file: &GlobalConfig) -> Result<GlobalConfig> {
        Ok(self.overlay.apply(&*self.persisted_view(file)?))
    }

    /// Make `file` the configuration without persisting it, for a caller
    /// that found the persisted truth already says it: `held` (the
    /// `file_config` write guard the caller holds) and the effective config
    /// both move to it.
    pub(super) fn adopt_config(&self, held: &mut GlobalConfig, file: GlobalConfig) -> Result<()> {
        let effective = self.effective_of(&file)?;
        *held = file;
        *self.config.write().unwrap() = effective;
        Ok(())
    }

    /// Persist `file` (see [`Engine::persist_config`]) and then make it the
    /// configuration, `held` being the `file_config` write guard the caller
    /// holds across the whole load-modify-save. A failed save changes nothing
    /// in memory.
    #[track_caller]
    pub(super) fn commit_config(&self, held: &mut GlobalConfig, file: GlobalConfig) -> Result<()> {
        let effective = self.effective_of(&file)?;
        self.persist_config(held, &file)?;
        *held = file;
        *self.config.write().unwrap() = effective;
        Ok(())
    }

    /// Persist a config to where this engine keeps it: the path this engine
    /// was started with (its `--config` override), or the default global
    /// config path when none was given, or the database document in document
    /// mode (see [`Engine::with_persisted`]). Never touches unrelated
    /// content: the caller always passes the current, load-modify-save typed
    /// config (see [`Engine::fresh_file_config`] for the load), so the serde
    /// round trip keeps every other key byte-for-byte. In document mode
    /// `config` is the document alone, never merged with its base or the
    /// environment.
    ///
    /// `held` is the `file_config` write guard the caller holds. It is
    /// touched only when the database cannot say whether a document save
    /// landed: the document is read back and this engine takes what is
    /// stored, and the error says which way it went (see
    /// [`Engine::settle_an_unknown_save`]).
    ///
    /// Every write logs one line naming its caller, so a daemon log says
    /// which path saved the file when it changed under somebody's feet.
    #[track_caller]
    pub(super) fn persist_config(
        &self,
        held: &mut GlobalConfig,
        config: &GlobalConfig,
    ) -> Result<()> {
        let caller = std::panic::Location::caller();
        match &self.persisted {
            None => {
                let path = match &self.config_path {
                    Some(p) => p.clone(),
                    None => crystalline_core::config::global_config_path()
                        .map_err(|e| EngineError::Internal(e.to_string()))?,
                };
                tracing::info!(caller = %caller, path = %path.display(), "saving the configuration");
                crate::persisted::PersistedConfig::File(path).save(config, &caller.to_string())?;
            }
            Some(crate::persisted::PersistedConfig::File(path)) => {
                tracing::info!(caller = %caller, path = %path.display(), "saving the configuration");
                crate::persisted::PersistedConfig::File(path.clone())
                    .save(config, &caller.to_string())?;
            }
            Some(persisted @ crate::persisted::PersistedConfig::Document { .. }) => {
                tracing::info!(caller = %caller, "saving the configuration document");
                match persisted.save(config, &caller.to_string()) {
                    Ok(()) => {}
                    Err(EngineError::ConfigMaybeStored(detail)) => {
                        return Err(self.settle_an_unknown_save(held, config, &detail));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        // A saved configuration may register, remove or re-alias a domain,
        // and the name table is built from the registrations. Marked here,
        // the one place every configuration write goes through, so a verb
        // that fails after its write (a sync after a registration) still
        // leaves the next lookup building from what the file says.
        self.mark_names_stale();
        Ok(())
    }

    /// After a document save whose outcome the database did not confirm:
    /// read the document back, make what is stored this engine's
    /// configuration (`held` and the effective config) and answer an error
    /// that says whether `intended` landed. When the read back fails too,
    /// memory stays as it was and the error says the outcome is unknown.
    /// Never a plain success or a plain failure: the caller's verb did not
    /// run to its end either way.
    fn settle_an_unknown_save(
        &self,
        held: &mut GlobalConfig,
        intended: &GlobalConfig,
        detail: &str,
    ) -> EngineError {
        let Some(crate::persisted::PersistedConfig::Document { doc, .. }) = &self.persisted else {
            return EngineError::ConfigMaybeStored(detail.to_string());
        };
        let unknown = |why: String| {
            EngineError::ConfigMaybeStored(format!(
                "the database did not confirm saving the configuration ({detail}), and reading it back failed ({why}); the change may or may not be stored, and this instance keeps the configuration it had"
            ))
        };
        let stored = match doc.load().and_then(|body| match body {
            Some(body) => crate::persisted::config_from_document(body).map(|(cfg, _)| cfg),
            None => Ok(GlobalConfig::default()),
        }) {
            Ok(stored) => stored,
            Err(e) => return unknown(e.to_string()),
        };
        let landed = match (
            crate::persisted::document_body(&stored),
            crate::persisted::document_body(intended),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        let effective = match self.effective_of(&stored) {
            Ok(effective) => effective,
            Err(e) => return unknown(e.to_string()),
        };
        *held = stored;
        *self.config.write().unwrap() = effective;
        self.mark_names_stale();
        EngineError::ConfigMaybeStored(if landed {
            format!(
                "the database did not confirm saving the configuration ({detail}); reading it back shows the change was stored, and this instance now uses it"
            )
        } else {
            format!(
                "the database did not confirm saving the configuration ({detail}); reading it back shows the change was not stored, so this instance uses the stored configuration; try again"
            )
        })
    }

    /// In document mode, refuse removing, purging or renaming `name` when the
    /// read-only base registers it, shadowed by the document or not (see
    /// [`crate::persisted::refuse_base_domain`]). Nothing to refuse in file
    /// mode.
    pub(super) fn refuse_base_domain(&self, name: &str) -> Result<()> {
        match (&self.config_base, &self.persisted) {
            (Some(base), Some(persisted)) => {
                let path = persisted
                    .base_path()
                    .map(Path::to_path_buf)
                    .or_else(|| self.config_file_path())
                    .unwrap_or_default();
                crate::persisted::refuse_base_domain(base, name, &path)
            }
            _ => Ok(()),
        }
    }

    /// In document mode, give the document its own copy of the base entry of
    /// `name` when it has none, so an edit of that one entry (its review
    /// mode, a provisioning decision, a rename question) lands in the
    /// document and shadows the base entry whole. Nothing to do in file mode
    /// or when the document already has the entry.
    pub(super) fn shadow_base_entry(&self, file: &mut GlobalConfig, name: &str) {
        if let Some(base) = &self.config_base
            && !file.domains.contains_key(name)
            && let Some(entry) = base.domains.get(name)
        {
            file.domains.insert(name.to_string(), entry.clone());
        }
    }

    // --- provision ---------------------------------------------------------

    /// Whether any registered domain currently declares a `## Provisioning`
    /// section in its MANIFEST, the gate on the `provision` MCP tool's
    /// mutating actions: with no such domain, `allow`, `deny` and `apply`
    /// refuse rather than report a reconcile that touched nothing. It used to
    /// gate the tool's visibility instead, which MCP 2026-07-28 forbids
    /// (SEP-2567: a tool list must not vary as a side effect of other requests
    /// on the connection, and `add_domain` and `update_domain` can create a
    /// declaration mid-session). Wraps
    /// [`crystalline_core::provision::any_domain_declares`] against the live
    /// effective config, read fresh off the config lock on every call rather
    /// than cached - the same cost class as `routing_text`, since a domain's
    /// MANIFEST can gain or lose a `Provisioning` section between calls (a
    /// freshly added domain, or an `update_domain` pull) and the very next
    /// `provision` call must reflect that.
    pub fn provisioning_declared(&self) -> bool {
        crystalline_core::provision::any_domain_declares(&self.config.read().unwrap())
    }

    /// Apply, inspect or record a decision for domain-declared artifact
    /// provisioning (the skills, commands, agents and MCP servers a domain's
    /// `## Provisioning` section ships into a harness's own config
    /// directory). [`ProvisionAction::Status`] reports every domain's
    /// decision and every installed harness's counts, writing nothing -
    /// always allowed, even on a read-only instance, mirroring
    /// `configure`'s `Show`. [`ProvisionAction::Allow`] and
    /// [`ProvisionAction::Deny`] record one domain's decision (the same
    /// file-config write-lock-first discipline as `configure`'s `Set`, see
    /// [`Engine::configure`]) and then reconcile; [`ProvisionAction::Apply`]
    /// reconciles without changing any decision. All three refuse with
    /// `EngineError::ReadOnly` on a read-only instance.
    ///
    /// The harnesses reconciled into always come from this machine's install
    /// receipt (`crystalline install`'s own memory of which harnesses are
    /// onboarded), never a caller-supplied list: provisioning targets every
    /// harness this machine has actually wired up.
    pub async fn provision(
        &self,
        action: &ProvisionAction,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        // Resolved once, and used two different ways below because the two
        // arms owe different things. `Status` is a pure read, so it must not
        // even compute over a domain this caller may not see: the config it is
        // given is narrowed first. `Allow`, `Deny` and `Apply` reconcile this
        // *machine's* harnesses, and narrowing what they reconcile over would
        // make a stranger's call retire a hidden domain's installed artifacts -
        // worse than the disclosure it would close - so those keep the whole
        // config and only their report is narrowed.
        //
        // One consequence of narrowing `Status` at the input, recorded because
        // it is a choice rather than an accident: a hidden domain's artifacts
        // fall out of the desired set with it, so its installed files read back
        // as orphaned or drifted in that caller's `harnesses` counts. Numbers
        // only - `HarnessStatus` carries no name - and the alternative is
        // computing the harness rows over a config the caller may not see,
        // which trades a count nobody acts on for the disclosure this closes.
        let hidden = self.hidden_for(scope).await?;
        let install_receipt = crystalline_core::provision::install_receipt_path()
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        let harnesses = crystalline_core::provision::installed_harnesses(&install_receipt);
        let receipt_path = crystalline_core::provision::receipt_path()
            .map_err(|e| EngineError::Internal(e.to_string()))?;

        let env_domains: HashSet<&str> = self
            .overlay
            .env_domains()
            .map(|(name, _)| name.as_str())
            .collect();

        match action {
            ProvisionAction::Status => {
                let mut config = self.config.read().unwrap().clone();
                // The whole of the scoping for this arm: `provision::status`
                // walks `config.domains` and pushes one entry per registered
                // domain whether or not it declares anything, so its report is
                // a complete list of domain names. Subtracting first drops a
                // hidden domain out of `domains`, `pending` and
                // `virtual_with_decision` at once, and out of the counts that
                // are derived from them.
                config.domains.retain(|name, _| !hidden.contains(name));
                let report = crystalline_core::provision::status(
                    &config,
                    &receipt_path,
                    &harnesses,
                    &env_domains,
                )
                .map_err(|e| EngineError::Internal(e.to_string()))?;
                Ok(status_report_json(&report))
            }
            ProvisionAction::Allow { domain } | ProvisionAction::Deny { domain } => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                // Deciding about a domain is a way of asking whether it exists,
                // so the name is checked before anything can describe the
                // domain: a domain this caller may not see is refused with
                // exactly the bytes a name nobody registered gets, from the one
                // helper every scoped refusal uses, ahead of the env check that
                // would name the domain's variable. The MCP verb asks the same
                // first; asking here keeps the engine safe on its own.
                self.domain_entry_scoped(domain, &hidden)?;
                // An env-defined domain's source of truth is its variable: the
                // overlay re-inserts a fresh entry (provision unset) on every
                // effective-config recompute, so a decision written to the
                // file would be silently discarded. Checked before the
                // registered-domain lookup so a shadowed and an env-only name
                // both get the env message, mirroring `origin_add`.
                if let Some(env) = self.overlay.env_domain(domain) {
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                        env.var
                    )));
                }
                let allow = matches!(action, ProvisionAction::Allow { .. });
                // Take the file-config write lock first, the same discipline
                // `configure`'s Set uses: serialize against a concurrent
                // decision, mutate a clone, persist, then swap both configs
                // in. Lock order is always file_config then config.
                {
                    let mut file_guard = self.file_config.write().unwrap();
                    let mut file = self.fresh_file_config(&file_guard)?;
                    // The name was resolved above; a domain removed from the
                    // file since is refused through the same helper, so even
                    // that race lists only what this caller may see.
                    self.shadow_base_entry(&mut file, domain);
                    set_domain_provision_decision(&mut file, domain, allow).map_err(
                        |e| match e {
                            EngineError::UnknownDomain { domain, .. } => {
                                self.unknown_domain(&domain, &hidden)
                            }
                            other => other,
                        },
                    )?;
                    self.commit_config(&mut file_guard, file)?;
                }
                self.run_provision_apply(&receipt_path, &harnesses, &hidden)
            }
            ProvisionAction::Apply => {
                if self.read_only {
                    return Err(EngineError::ReadOnly);
                }
                self.run_provision_apply(&receipt_path, &harnesses, &hidden)
            }
        }
    }

    /// Reconcile every opted-in domain's declared artifacts into `harnesses`
    /// through the real system MCP runner - the shared tail of
    /// `provision`'s `Allow`, `Deny` and `Apply` arms.
    fn run_provision_apply(
        &self,
        receipt_path: &Path,
        harnesses: &[HarnessKind],
        hidden: &HashSet<String>,
    ) -> Result<Value> {
        let config = self.config.read().unwrap().clone();
        let mut mcp = crate::harness_cli::SystemMcpRunner;
        let env_domains: HashSet<&str> = self
            .overlay
            .env_domains()
            .map(|(name, _)| name.as_str())
            .collect();
        let report = crystalline_core::provision::apply(
            &config,
            receipt_path,
            harnesses,
            &mut mcp,
            &env_domains,
        )
        .map_err(|e| EngineError::Internal(e.to_string()))?;
        let mut value = apply_report_json(&report);
        // The reconcile ran over the whole machine, as it must; the report goes
        // back to one caller, so it names only the domains that caller may see.
        // Two of the three arrays carry a domain name and both are narrowed
        // here. `harnesses[].actions[].target` is the third and carries none -
        // its keys are `{kind}/{rel}` built from the artifact's own filename.
        if let Some(pending) = value["pending"].as_array_mut() {
            pending.retain(|entry| {
                entry["domain"]
                    .as_str()
                    .is_none_or(|name| !hidden.contains(name))
            });
        }
        // `notices` is free prose, so it is filtered by what the prose does
        // rather than by a field: every notice that names a domain writes it
        // between backticks (the virtual-domain skip, the unsupported-kind
        // skip, both collision notices, the foreign-file keep and the
        // already-registered MCP server), so a backtick-anchored match drops
        // exactly those and leaves a notice about a visible domain that merely
        // happens to contain the hidden name as a substring. Anchored rather
        // than bare on purpose: a bare match would silence a visible domain's
        // own collision notice whenever a hidden domain's name appeared inside
        // one of the file names it reports.
        if let Some(notices) = value["notices"].as_array_mut() {
            notices.retain(|notice| {
                notice.as_str().is_none_or(|text| {
                    !hidden
                        .iter()
                        .any(|name| text.contains(&format!("`{name}`")))
                })
            });
        }
        Ok(value)
    }
}

/// The refusal of a configuration write over a file that does not parse.
fn config_left_as_it_is(e: impl std::fmt::Display) -> EngineError {
    EngineError::Invalid(format!(
        "{e}; the configuration file is left as it is until it parses again"
    ))
}

#[cfg(test)]
mod document_tests {
    use super::*;
    use crate::persisted::{MemoryDocument, PersistedConfig};
    use crystalline_core::config::SearchConfig;
    use crystalline_index::TursoStore;

    /// The read-only base: one virtual domain and one search weight.
    fn base() -> GlobalConfig {
        let mut base = GlobalConfig {
            search: Some(SearchConfig {
                salience_weight: None,
                retired_weight: Some(0.1),
            }),
            ..Default::default()
        };
        base.domains
            .insert("base-only".into(), DomainEntry::virtual_domain());
        base
    }

    /// An engine in document mode over `doc` and [`base`], with a setting and
    /// a domain from the environment, and a config.yaml on disk holding the
    /// base. Answers the engine and the bytes of that file.
    async fn engine_on(doc: Arc<MemoryDocument>) -> (Engine, Vec<u8>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        crystalline_core::config::save_yaml(&path, &base()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let overlay = EnvOverlay::from_vars([
            (
                "CRYSTALLINE_SEARCH_SALIENCE_WEIGHT".to_string(),
                "0.3".to_string(),
            ),
            (
                "CRYSTALLINE_DOMAIN_FROM_ENV".to_string(),
                tmp.path().display().to_string(),
            ),
        ])
        .unwrap();
        let document = match doc.stored() {
            Some(body) => crate::persisted::config_from_document(body).unwrap().0,
            None => GlobalConfig::default(),
        };
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(
            Arc::new(Mutex::new(store)),
            document,
            None,
            Some(path.clone()),
        )
        .with_env_overlay(overlay)
        .with_state_dir(tmp.path().join("state"))
        .with_persisted(
            PersistedConfig::Document {
                doc,
                base: Some(path),
            },
            Some(base()),
        )
        .unwrap();
        (engine, bytes, tmp)
    }

    fn set(key: &str, value: &str) -> ConfigureAction {
        ConfigureAction::Set {
            key: key.into(),
            value: value.into(),
        }
    }

    #[tokio::test]
    async fn config_set_database_url_refused_in_document_mode() {
        let doc = Arc::new(MemoryDocument::default());
        let (engine, _, _tmp) = engine_on(doc.clone()).await;
        for key in ["database.url", "database.schema", "database.backend"] {
            let err = engine
                .configure(&set(key, "postgres://elsewhere/db"))
                .await
                .unwrap_err();
            assert!(matches!(err, EngineError::Refused(_)), "{key}: {err}");
            assert!(err.to_string().contains("kept in the database"), "{err}");
            let err = engine
                .configure(&ConfigureAction::Unset { key: key.into() })
                .await
                .unwrap_err();
            assert!(matches!(err, EngineError::Refused(_)), "{key}: {err}");
        }
        assert_eq!(doc.stored(), None, "nothing was saved");
    }

    /// A save stores the document alone: no base domain, no base setting,
    /// no environment value or domain, no database block. The effective
    /// configuration still has all of them, and config.yaml is untouched.
    #[tokio::test]
    async fn a_document_save_stores_document_only_state() {
        let doc = Arc::new(MemoryDocument::default());
        let (engine, bytes, tmp) = engine_on(doc.clone()).await;
        engine
            .configure(&set("service.response_format", "json"))
            .await
            .unwrap();
        engine.domain_add_virtual("fresh").await.unwrap();
        let stored = doc.stored().unwrap();
        assert_eq!(stored["service"]["response_format"], "json", "{stored}");
        let names: Vec<_> = stored["domains"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["fresh"], "{stored}");
        assert!(stored.get("search").is_none(), "{stored}");
        assert!(stored.get("database").is_none(), "{stored}");

        let effective = engine.config();
        assert!(effective.domains.contains_key("base-only"));
        assert!(effective.domains.contains_key("fresh"));
        assert!(effective.domains.contains_key("from-env"));
        let search = effective.search.unwrap();
        assert_eq!(search.salience_weight, Some(0.3));
        assert_eq!(search.retired_weight, Some(0.1));
        assert_eq!(
            std::fs::read(tmp.path().join("config.yaml")).unwrap(),
            bytes
        );
    }

    /// Document mode never reads config.yaml again: an edit of the file
    /// after start is not seen, and the document in memory is the truth.
    #[tokio::test]
    async fn document_mode_reads_memory_not_the_file() {
        let doc = Arc::new(MemoryDocument::default());
        let (engine, _, tmp) = engine_on(doc).await;
        let mut edited = base();
        edited
            .domains
            .insert("added-by-hand".into(), DomainEntry::virtual_domain());
        crystalline_core::config::save_yaml(&tmp.path().join("config.yaml"), &edited).unwrap();
        let names = engine.registered_domain_names();
        assert!(names.contains("base-only"));
        assert!(!names.contains("added-by-hand"), "{names:?}");
    }

    #[tokio::test]
    async fn a_failed_document_save_changes_nothing() {
        let doc = Arc::new(MemoryDocument::default());
        let (engine, _, _tmp) = engine_on(doc.clone()).await;
        let before = engine.config();
        *doc.fail.lock().unwrap() = Some(EngineError::Internal("down".into()));
        let err = engine
            .configure(&set("service.response_format", "json"))
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Internal(_)), "{err}");
        assert_eq!(engine.config(), before);
        assert_eq!(doc.stored(), None);
    }

    /// A save the database did not confirm: memory becomes what is stored,
    /// and the answer says which way it went, never a plain outcome.
    #[tokio::test]
    async fn an_unconfirmed_save_takes_what_is_stored() {
        let doc = Arc::new(MemoryDocument::default());
        let (engine, _, _tmp) = engine_on(doc.clone()).await;

        // Stored after all.
        *doc.fail_after_store.lock().unwrap() =
            Some(EngineError::ConfigMaybeStored("no answer".into()));
        let err = engine
            .configure(&set("service.response_format", "json"))
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfigMaybeStored(_)), "{err}");
        assert!(err.to_string().contains("the change was stored"), "{err}");
        assert_eq!(
            engine.config().service.unwrap().response_format,
            Some(crystalline_core::config::ResponseFormat::Json)
        );
        assert!(engine.config().domains.contains_key("base-only"));

        // Not stored: memory is what is stored, the earlier change.
        *doc.fail.lock().unwrap() = Some(EngineError::ConfigMaybeStored("no answer".into()));
        let err = engine
            .configure(&set("service.response_format", "toon"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("was not stored"), "{err}");
        assert_eq!(
            engine.config().service.unwrap().response_format,
            Some(crystalline_core::config::ResponseFormat::Json)
        );

        // The read back fails too: memory stays as it was.
        *doc.fail.lock().unwrap() = Some(EngineError::ConfigMaybeStored("no answer".into()));
        *doc.fail_load.lock().unwrap() = Some(EngineError::Internal("down".into()));
        let err = engine
            .configure(&set("service.response_format", "toon"))
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfigMaybeStored(_)), "{err}");
        assert!(
            err.to_string().contains("may or may not be stored"),
            "{err}"
        );
        assert_eq!(
            engine.config().service.unwrap().response_format,
            Some(crystalline_core::config::ResponseFormat::Json)
        );
    }

    #[tokio::test]
    async fn removing_a_base_only_domain_is_refused() {
        let doc = Arc::new(MemoryDocument::default());
        let (engine, _, tmp) = engine_on(doc.clone()).await;
        let err = engine.domain_remove("base-only").await.unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{err}");
        assert_eq!(
            err.to_string(),
            format!(
                "domain 'base-only' comes from config.yaml at {}; edit that file",
                tmp.path().join("config.yaml").display()
            )
        );
        // A rename of it refuses before its first step.
        let err = engine
            .rename_domain_local(
                "base-only",
                "renamed",
                NameOrigin::Explicit,
                &crate::scope::Scope::Unrestricted,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{err}");
        // The whole removal refuses before it purges or sweeps anything.
        let err = engine
            .unregister_domain("base-only", &crate::scope::Scope::Unrestricted, true, &[])
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{err}");
        assert_eq!(doc.stored(), None);
        assert!(engine.config().domains.contains_key("base-only"));
    }

    /// A base domain the document shadows (its review mode was changed, which
    /// copies the entry into the document) is still the base's: removing,
    /// purging and renaming it are refused, and nothing is dropped, purged or
    /// moved.
    #[tokio::test]
    async fn a_shadowed_base_domain_is_not_removed_or_renamed() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("team");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("MANIFEST.md"),
            "---\ntitle: team\ntype: manifest\nstatus: active\n---\n# team\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("note.md"),
            "---\ntitle: note\ntype: note\nstatus: active\n---\nbody\n",
        )
        .unwrap();
        let mut entry = DomainEntry::file(folder.clone());
        entry.review = Some(crystalline_core::config::ReviewMode::Overlay);
        entry.origin = Some(crystalline_core::config::OriginConfig {
            repo: "acme/team".into(),
            path: None,
            branch: Some("main".into()),
            poll_secs: None,
        });
        let mut base = GlobalConfig::default();
        base.domains.insert("team".into(), entry);
        let path = tmp.path().join("config.yaml");
        crystalline_core::config::save_yaml(&path, &base).unwrap();
        let doc = Arc::new(MemoryDocument::default());
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(
            Arc::new(Mutex::new(store)),
            GlobalConfig::default(),
            None,
            Some(path.clone()),
        )
        .with_state_dir(tmp.path().join("state"))
        .with_persisted(
            PersistedConfig::Document {
                doc: doc.clone(),
                base: Some(path),
            },
            Some(base),
        )
        .unwrap();
        let scope = crate::scope::Scope::Unrestricted;

        // Leaving review mode shadows the base entry in the document.
        engine
            .set_review_mode(
                "team",
                None,
                ReviewModeConfirm::Confirmed { folds: Vec::new() },
                &scope,
            )
            .await
            .unwrap();
        let shadowed = doc.stored().unwrap();
        assert!(shadowed["domains"]["team"].is_object(), "{shadowed}");
        assert!(engine.config().domains["team"].review.is_none());

        let err = engine.domain_remove("team").await.unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{err}");
        assert!(err.to_string().contains("comes from config.yaml"), "{err}");
        let err = engine
            .unregister_domain("team", &scope, true, &[])
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{err}");
        let err = engine
            .rename_domain_local("team", "renamed", NameOrigin::Explicit, &scope)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{err}");

        // Nothing dropped, purged or moved.
        assert_eq!(doc.stored().unwrap(), shadowed);
        assert!(engine.config().domains.contains_key("team"));
        assert!(engine.registered_domain_names().contains("team"));
        assert!(folder.join("note.md").is_file());
        assert!(!tmp.path().join("renamed").exists());
    }

    /// File mode is what it always was: no document, the file written.
    #[tokio::test]
    async fn file_mode_writes_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(
            Arc::new(Mutex::new(store)),
            GlobalConfig::default(),
            None,
            Some(path.clone()),
        );
        assert!(!engine.config_in_document());
        engine
            .configure(&set("service.response_format", "json"))
            .await
            .unwrap();
        let mut expected = GlobalConfig::default();
        settings::apply(&mut expected, "service.response_format", "json").unwrap();
        let theirs = tmp.path().join("theirs.yaml");
        crystalline_core::config::save_yaml(&theirs, &expected).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(&theirs).unwrap()
        );
    }
}
