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
    /// [`Engine::announce_a_moved_tool_list`], because this method is the one
    /// seam all three settings callers share.
    pub async fn configure(&self, action: &ConfigureAction) -> Result<Value> {
        match action {
            ConfigureAction::Show => {
                let file = self.file_config.read().unwrap();
                Ok(json!({ "settings": settings::snapshot(&file, &self.overlay) }))
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
                    let mut file = self.fresh_file_config(&file_guard);
                    settings::apply(&mut file, key, value)?;
                    self.persist_config(&file)?;
                    // Recompute the effective config from the freshly saved file
                    // plus the overlay, so an env-overridden key keeps reading its
                    // env value even after the file value changes underneath it.
                    let effective = self.overlay.apply(&file);
                    let view = self.setting_view_json(&file, key);
                    *file_guard = file;
                    *self.config.write().unwrap() = effective;
                    view
                };
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
                    let mut file = self.fresh_file_config(&file_guard);
                    settings::unset(&mut file, key)?;
                    self.persist_config(&file)?;
                    let effective = self.overlay.apply(&file);
                    let view = self.setting_view_json(&file, key);
                    *file_guard = file;
                    *self.config.write().unwrap() = effective;
                    view
                };
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
    fn setting_view_json(&self, file: &GlobalConfig, key: &str) -> Value {
        settings::snapshot(file, &self.overlay)
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
            .unwrap_or(Value::Null)
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
    /// one-shot command and many test engines are, or a file that will not
    /// parse, where persisting the snapshot is what happened before too.
    pub(super) fn fresh_file_config(&self, snapshot: &GlobalConfig) -> GlobalConfig {
        let Some(path) = self.config_file_path() else {
            return snapshot.clone();
        };
        if !path.is_file() {
            return snapshot.clone();
        }
        overlay::load_file(&path).unwrap_or_else(|_| snapshot.clone())
    }

    /// Persist a config to the path this engine was started with (its
    /// `--config` override), or the default global config path when none was
    /// given. Never touches unrelated content: the caller always passes the
    /// current, load-modify-save typed config (see
    /// [`Engine::fresh_file_config`] for the load), so the serde round trip
    /// keeps every other key byte-for-byte.
    pub(super) fn persist_config(&self, config: &GlobalConfig) -> Result<()> {
        let path = match &self.config_path {
            Some(p) => p.clone(),
            None => crystalline_core::config::global_config_path()
                .map_err(|e| EngineError::Internal(e.to_string()))?,
        };
        crystalline_core::config::save_yaml(&path, config).map_err(|e| {
            EngineError::Internal(format!("failed to save config {}: {e}", path.display()))
        })
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
                    let mut file = self.fresh_file_config(&file_guard);
                    set_domain_provision_decision(&mut file, domain, allow)?;
                    self.persist_config(&file)?;
                    let effective = self.overlay.apply(&file);
                    *file_guard = file;
                    *self.config.write().unwrap() = effective;
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
