use super::*;

impl Engine {
    // --- origin (GitHub collaboration) ----------------------------------------

    /// Connects a new domain to a GitHub repository: downloads its tracked
    /// subtree, registers it in the global config and brings it into the
    /// index, mirroring what `domain add` does for a local folder.
    ///
    /// `domain` defaults to the repository's own name segment; `folder`
    /// defaults to `~/Documents/Crystalline/<domain>`. `path` is the
    /// subfolder within the repository that is the domain root (absent means
    /// the repository root); `branch` defaults to the repository's default
    /// branch, asked from the forge and recorded in the entry.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (this both
    /// writes content and mutates config, exactly the two things read-only
    /// mode protects). A fresh connect returns `{ domain, root, engrams,
    /// base_commit, adopted, files_added, local_changes }`, so a caller knows
    /// what landed and whether existing local knowledge was adopted. A retry
    /// of the exact same connect - matching repo, subpath, branch and folder -
    /// instead returns `{ domain, root, engrams, base_commit, already_connected:
    /// true }`, so a client that timed out on the first attempt reads the
    /// connected state rather than a conflict.
    pub async fn origin_add(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Value> {
        self.origin_add_with_progress(repo, domain, path, branch, folder, None)
            .await
    }

    /// [`origin_add`](Self::origin_add) with an optional stage-boundary
    /// progress callback. A real connect reports four stages through it -
    /// downloading, downloaded, indexing, connected - so a client can keep
    /// its request timeout alive during a long download and index; an
    /// already-connected retry is instant and reports none.
    pub async fn origin_add_with_progress(
        &self,
        repo: &str,
        domain: Option<&str>,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
        progress: Option<OriginProgress>,
    ) -> Result<Value> {
        let progress_at = |step: u64, msg: &str| {
            if let Some(p) = &progress {
                p(step, 4, msg);
            }
        };
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }

        let domain_name = match domain {
            Some(d) => d.to_string(),
            None => origin::default_domain_name(repo),
        };
        // A name nothing holds is a new registration: check it before the
        // default folder is derived from it. A derived name passes by
        // construction (`origin::default_domain_name`); an explicit one may
        // not.
        if self.domain_entry(&domain_name).is_err() {
            validate_domain_name(&domain_name).map_err(EngineError::Invalid)?;
        }
        // A registered name is adoptable when it is an origin-less file
        // domain and the caller does not point somewhere else: the origin
        // attaches to the existing root in place and local knowledge is
        // kept. Anything else stays a conflict.
        let existing = match self.domain_entry(&domain_name) {
            Err(_) => None,
            Ok(entry) => {
                // An env-defined domain names the variable that owns it, so
                // the operator knows to unset it rather than pick another
                // name.
                if let Some(env) = self.overlay.env_domain(&domain_name) {
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain_name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                        env.var
                    )));
                }
                if let Some(origin_cfg) = &entry.origin {
                    // A retry of the exact connect that already succeeded answers
                    // with the connected state instead of a conflict, so a client
                    // that timed out waiting for the first response never reads
                    // success as failure. This pre-lock guard keeps the common
                    // retry-after-completion case instant and lock-free; a re-read
                    // under the lock below catches a retry that raced an in-flight
                    // connect (see `origin_add_with_progress`).
                    if Self::origin_matches_request(&entry, origin_cfg, repo, path, branch, folder)
                    {
                        return self.origin_already_connected(&domain_name, &entry).await;
                    }
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain_name}' is already connected to {}; pass a domain name to connect this origin under a different one",
                        origin_cfg.repo
                    )));
                }
                let Some(registered_root) = entry.file_path() else {
                    return Err(EngineError::Conflict(format!(
                        "domain '{domain_name}' is a virtual domain; an origin connects a file domain, so pass a different domain name"
                    )));
                };
                if let Some(f) = folder {
                    let wanted = crystalline_core::config::expand_tilde(f);
                    if wanted != registered_root {
                        return Err(EngineError::Conflict(format!(
                            "domain '{domain_name}' is rooted at {}; omit the folder to connect it in place, or pass a different domain name",
                            registered_root.display()
                        )));
                    }
                }
                Some((registered_root, entry))
            }
        };

        let lock = self.origin_lock(&domain_name);
        let _guard = lock.lock().await;

        // Re-read the config under the lock. A connect that raced ahead of us
        // - a timed-out client's first attempt, still downloading when our
        // retry slipped past the pre-lock guard with no origin on file yet -
        // may have persisted its origin while we queued here. Answer a
        // now-matching origin idempotently instead of downloading the whole
        // repo again, and a now-conflicting one with the same conflict the
        // pre-lock guard raises. The locked helper skips the lock we hold, and
        // no progress stage fires: an idempotent return reports none, exactly
        // like the pre-lock guard's.
        if let Ok(entry) = self.domain_entry(&domain_name)
            && let Some(origin_cfg) = &entry.origin
        {
            if Self::origin_matches_request(&entry, origin_cfg, repo, path, branch, folder) {
                return self
                    .origin_already_connected_locked(&domain_name, &entry)
                    .await;
            }
            return Err(EngineError::Conflict(format!(
                "domain '{domain_name}' is already connected to {}; pass a domain name to connect this origin under a different one",
                origin_cfg.repo
            )));
        }

        let adopts_registered = existing.is_some();
        let (root, adopted_entry) = match existing {
            Some((r, entry)) => (r, Some(entry)),
            None => (
                match folder {
                    Some(f) => crystalline_core::config::expand_tilde(f),
                    None => {
                        let domains_root = self.config.read().unwrap().domains_root();
                        origin::default_domain_folder(&domains_root, &domain_name)
                    }
                },
                None,
            ),
        };
        let provider = self.resolve_origin_provider()?;
        // No branch named: ask the forge which branch the repository calls its
        // default and record that, so the entry says what it tracks. Never a
        // silent `main`: a repository whose default is `trunk` would track a
        // branch that does not exist. A failed lookup refuses the add.
        let branch_name = match branch {
            Some(b) => b.to_string(),
            None => provider
                .default_branch(repo)
                .await
                .inspect_err(|e| self.drop_github_credential_on_auth(e))
                .map_err(|e| default_branch_refusal(repo, e))?,
        };
        let spec = OriginSpec {
            repo: repo.to_string(),
            subpath: path.map(str::to_string),
            branch: branch_name.clone(),
        };
        let state_dir = self.origin_state_dir(&domain_name)?;
        progress_at(1, &format!("downloading {repo}"));
        let report = ops::subscribe(provider.as_ref(), &spec, &root, &state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;
        progress_at(
            2,
            &format!(
                "downloaded {} engrams, registering the domain",
                report.engrams
            ),
        );

        // Register the domain and persist, mirroring `configure`'s file-then-
        // effective write-lock-first pattern so a concurrent read never observes
        // a half-applied config and no env value bakes into the saved file.
        {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard);
            // Adopting a registered domain keeps the decisions already made
            // about it: whether its artifacts are provisioned, and whether it
            // reviews changes. Read from the file when it holds the entry,
            // else from the entry this call adopted.
            let (provision, review) = file
                .domains
                .get(&domain_name)
                .or(adopted_entry.as_ref())
                .map(|e| (e.provision, e.review))
                .unwrap_or((None, None));
            file.domains.insert(
                domain_name.clone(),
                DomainEntry {
                    kind: CoreDomainKind::File,
                    path: Some(root.clone()),
                    origin: Some(OriginConfig {
                        repo: repo.to_string(),
                        path: path.map(str::to_string),
                        branch: Some(branch_name.clone()),
                        poll_secs: None,
                    }),
                    provision,
                    review,
                },
            );
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }

        // Tell a running daemon's watcher to start watching the new root; it
        // also runs its own catch-up sync and embed once the watch is armed.
        // This engine's own sync just below runs regardless, so the domain is
        // searchable immediately even outside a daemon (a standalone CLI
        // command, or a race with the watcher's async catch-up); sync is
        // checksum idempotent, so the watcher repeating it moments later is a
        // harmless no-op. An adopted registered domain is already watched.
        if !adopts_registered && let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(domain_name.clone(), root.clone()));
        }

        progress_at(3, "indexing for search");
        self.sync(Some(&domain_name)).await?;
        // Embedding a whole freshly connected repo can outlast any client
        // timeout, so a daemon or in-process MCP server runs it on the embed
        // worker; without a worker (standalone one-shot commands, tests) the
        // inline pass keeps the old behavior, and is a no-op anyway whenever
        // no provider is loaded.
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after connecting '{domain_name}' failed: {e}");
        }

        progress_at(4, "connected");
        Ok(json!({
            "domain": domain_name,
            "root": root.display().to_string(),
            "engrams": report.engrams,
            "base_commit": report.base_commit,
            "adopted": report.adopted || adopts_registered,
            "files_added": report.files_written,
            "local_changes": report.local_changes,
        }))
    }

    /// Whether a registered domain's origin matches this connect request
    /// exactly, so a retry answers idempotently instead of re-connecting.
    /// GitHub treats owner/name case insensitively, so the repo compares that
    /// way; the subpath compares exactly, an absent requested branch matches
    /// any stored one, and an absent stored branch means main; an omitted
    /// folder always matches, a given one must resolve to the registered root.
    /// Shared by the pre-lock guard and the re-read under the lock so both
    /// sites judge a match identically.
    fn origin_matches_request(
        entry: &DomainEntry,
        origin_cfg: &OriginConfig,
        repo: &str,
        path: Option<&str>,
        branch: Option<&str>,
        folder: Option<&str>,
    ) -> bool {
        let same_repo = origin_cfg.repo.eq_ignore_ascii_case(repo);
        let same_path = origin_cfg.path.as_deref() == path;
        // No branch asked for matches whatever the entry tracks: a connect
        // without one records the repository default it resolved, and a retry
        // of that connect is the same request. A stored entry with no branch
        // still means main, since it was written under that rule.
        let same_branch = match branch {
            None => true,
            Some(b) => origin_cfg.branch() == b,
        };
        let same_folder = match (folder, entry.file_path()) {
            (None, _) => true,
            (Some(f), Some(r)) => crystalline_core::config::expand_tilde(f) == r,
            (Some(_), None) => false,
        };
        same_repo && same_path && same_branch && same_folder
    }

    /// The response for a connect retry that matches the existing
    /// connection: the same shape `origin_add` returns, marked
    /// `already_connected`, read under the domain's origin lock.
    async fn origin_already_connected(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        self.origin_already_connected_locked(name, entry).await
    }

    /// [`origin_already_connected`](Self::origin_already_connected)'s body,
    /// assuming the caller already holds the domain's origin lock. The re-read
    /// inside `origin_add_with_progress` calls this directly: the origin lock
    /// is a non-reentrant tokio mutex, so re-acquiring it there would
    /// deadlock.
    async fn origin_already_connected_locked(
        &self,
        name: &str,
        entry: &DomainEntry,
    ) -> Result<Value> {
        let root = entry.file_path().unwrap_or_default();
        let state_dir = self.origin_state_dir(name)?;
        let base_commit = crystalline_remote::state::OriginState::load(&state_dir)?
            .map(|s| s.base_commit)
            .unwrap_or_default();
        let engrams = {
            let store = self.store.lock().await;
            store
                .domain_stats()
                .await
                .unwrap_or_default()
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.engrams)
                .unwrap_or(0)
        };
        Ok(json!({
            "domain": name,
            "root": root.display().to_string(),
            "engrams": engrams,
            "base_commit": base_commit,
            "already_connected": true,
        }))
    }

    /// Brings one origin-connected domain (or every one, when `domain` is
    /// `None`) up to date with its origin. Errors when a named domain is not
    /// registered or has no origin; one domain failing (offline, revoked)
    /// never aborts the others, each per-domain failure is collected into the
    /// `errors` array instead. Allowed on a read-only instance: a pull is a
    /// derived-truth update like sync, not a user-authored content write.
    pub async fn origin_update(
        &self,
        domain: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        let hidden = self.hidden_for(scope).await?;
        let targets = self.origin_targets(domain, &hidden)?;

        let mut domains = Vec::new();
        let mut errors = Vec::new();
        for (name, entry) in targets {
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => domains.push(v),
                Err(e) => errors.push(json!({ "domain": name, "error": e.to_string() })),
            }
        }
        Ok(json!({ "domains": domains, "errors": errors }))
    }

    /// Pulls and syncs one domain, under its origin lock. The per-domain body
    /// behind `origin_update`'s aggregate loop.
    pub(super) async fn origin_update_one(&self, name: &str, entry: &DomainEntry) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;

        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;

        // An env-defined team domain with no origin state yet bootstraps itself
        // on first contact: the zero-config read-only node's first pull is a
        // subscribe, not an update. This is gated on the domain being
        // env-defined so a non-env domain with missing state still fails exactly
        // as before (it was never fully connected). Bootstrapping is a
        // derived-truth pull, so it is allowed on a read-only instance. The
        // env check comes first so ordinary domains skip the state read on
        // every poll tick.
        if self.overlay.env_domain(name).is_some()
            && crystalline_remote::state::OriginState::load(&state_dir)
                .ok()
                .flatten()
                .is_none()
        {
            return self
                .bootstrap_env_origin(name, &spec, &root, &state_dir)
                .await;
        }

        let provider = self.resolve_origin_provider()?;
        let report = ops::pull(provider.as_ref(), &spec, &root, &state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;

        self.sync(Some(name)).await?;
        // The pull advanced the base, so every draft standing over this domain
        // is asked whether the folder has caught up with it. After the sync,
        // not before: the pass reads the base snapshot for content but the rest
        // of this call expects a domain whose rows are current. A failure here
        // is a warning rather than a failed update - the pull has already
        // landed on disk, and the next pull runs the whole pass again.
        if !report.up_to_date
            && let Err(e) = self.converge_pulled_overlays(name, &report.applied).await
        {
            tracing::warn!("converging the drafts in '{name}' after updating failed: {e}");
        }
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after updating '{name}' failed: {e}");
        }

        // `ops::pull` already saved the post-pull state to `state_dir`; reload
        // it fresh so each transition's url and title can be joined in for
        // the caller. A reload failure only degrades the proposal entries to
        // number and status (see `origin::proposal_transitions_json`), it
        // never fails an update that has already landed on disk.
        let state = crystalline_remote::state::OriginState::load(&state_dir)
            .ok()
            .flatten();
        let proposals = origin::proposal_transitions_json(&report.proposals, state.as_ref());
        // Every still-open proposal rides along in full, review feedback
        // included: an update is where a pull refreshes it, so this is the
        // channel an agent reads reviewer comments from without a second call.
        let open_proposals: Vec<Value> = state
            .as_ref()
            .map(|s| {
                s.proposals
                    .iter()
                    .filter(|p| p.status == crystalline_remote::state::ProposalStatus::Open)
                    .map(|p| serde_json::to_value(p).expect("a proposal serializes"))
                    .collect()
            })
            .unwrap_or_default();
        let mut v = origin::pull_report_json(name, &report, proposals);
        v["open_proposals"] = Value::Array(open_proposals);
        Ok(v)
    }

    /// Bootstraps an env-defined team domain on its first contact with GitHub:
    /// creates the root, runs the same [`ops::subscribe`] `origin_add` uses
    /// (minus the config write, since an env domain is never persisted), then
    /// syncs and best-effort embeds. Called under the domain's origin lock by
    /// [`Engine::origin_update_one`]. The report is shaped like a normal update
    /// (`up_to_date`, `applied`, `merged`, `conflicts`, `proposals`) so
    /// `print_origin_update` and the poller's outcome handling keep working
    /// unchanged, plus `bootstrapped: true` and the subscribe facts (`engrams`,
    /// `base_commit`) a bootstrapped line reads from.
    async fn bootstrap_env_origin(
        &self,
        name: &str,
        spec: &OriginSpec,
        root: &Path,
        state_dir: &Path,
    ) -> Result<Value> {
        let provider = self.resolve_origin_provider()?;
        // notify refuses to watch a missing directory; the daemon pre-creates
        // env-domain roots at startup, but a subscribe run outside that path
        // (an on-demand `origin update`, a poll tick) creates it here too.
        std::fs::create_dir_all(root).map_err(|e| {
            EngineError::Internal(format!(
                "could not create the domain root {}: {e}",
                root.display()
            ))
        })?;
        let report = ops::subscribe(provider.as_ref(), spec, root, state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))?;

        // Tell a running daemon's watcher to start watching the freshly
        // bootstrapped root, the same signal `origin_add` sends.
        if let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(name.to_string(), root.to_path_buf()));
        }

        self.sync(Some(name)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after bootstrapping '{name}' failed: {e}");
        }

        Ok(json!({
            "domain": name,
            "bootstrapped": true,
            "up_to_date": false,
            "applied": [],
            "merged": [],
            "conflicts": [],
            "proposals": [],
            "skipped_large": report.skipped_large,
            "re_baselined": false,
            "engrams": report.engrams,
            "base_commit": report.base_commit,
        }))
    }

    /// Bootstraps every env-defined team domain that carries an origin but
    /// has no local origin state yet, bringing each up through
    /// [`Engine::origin_update_one`] so bootstrapping and a plain background
    /// pull stay exactly one code path. Called once from the daemon's startup
    /// task. A missing GitHub connection is not a failure - the background
    /// poller retries the moment a connection lands - so `NotConnected` only
    /// logs an info line; any other per-domain error is logged and never
    /// aborts startup. When env-origin domains exist while collaboration is
    /// off, one warning tells the operator to turn it on.
    pub async fn bootstrap_env_origins(&self) {
        let targets: Vec<(String, DomainEntry)> = self
            .overlay
            .env_domains()
            .filter(|(_, env)| env.entry.origin.is_some())
            .map(|(name, env)| (name.clone(), env.entry.clone()))
            .collect();
        if targets.is_empty() {
            return;
        }
        if !self.config.read().unwrap().github_enabled() {
            tracing::warn!(
                "env-defined team domains are configured but GitHub collaboration is off; set CRYSTALLINE_GITHUB_ENABLED=true to let them bootstrap"
            );
            return;
        }

        for (name, entry) in targets {
            let Ok(state_dir) = self.origin_state_dir(&name) else {
                continue;
            };
            // Already bootstrapped in an earlier run: nothing to do here, the
            // poller keeps it up to date from now on.
            let has_state = crystalline_remote::state::OriginState::load(&state_dir)
                .ok()
                .flatten()
                .is_some();
            if has_state {
                continue;
            }
            match self.origin_update_one(&name, &entry).await {
                Ok(v) => {
                    tracing::info!(
                        "bootstrapped env-defined team domain '{name}' ({} engram(s) at {})",
                        v["engrams"].as_u64().unwrap_or(0),
                        v["base_commit"].as_str().unwrap_or("")
                    );
                }
                Err(EngineError::Remote(RemoteError::NotConnected)) => {
                    tracing::info!(
                        "env-defined team domain '{name}' is waiting for a GitHub connection; the poller retries automatically"
                    );
                }
                Err(e) => {
                    tracing::warn!("could not bootstrap env-defined team domain '{name}': {e}");
                }
            }
        }
    }

    /// Reports where one origin-connected domain (or every one, when
    /// `domain` is `None`) stands relative to its origin, plus this
    /// machine's GitHub connection. Never hard-fails just because the
    /// machine is offline or has no saved connection: each domain's `behind`
    /// is `None` and the connection block reports `connected: false` rather
    /// than erroring. One domain's genuine failure (corrupt state, a missing
    /// filesystem root) never aborts the others: it is collected into the
    /// `errors` array instead, mirroring `origin_update`. Allowed on a
    /// read-only instance (a pure read).
    ///
    /// `detail` names the unshared work instead of only counting it: each
    /// domain entry then carries a `detail` block grouping the changed paths
    /// by kind (see [`origin::local_change_detail`]). It is opt-in because it
    /// costs a second walk of every domain's working tree, so `local_changes`
    /// stays the bare count for every caller that only wants to know whether
    /// there is anything to share.
    ///
    /// `diff` goes one step further and puts both sides of every unshared file
    /// in the detail block, under `diff`: the team's copy and this machine's,
    /// so a caller can say what changed before sharing or discarding it. It
    /// needs a domain, because reading every side of every domain at once is a
    /// walk nobody asked for, and it implies `detail`, since a diff with no
    /// file list beside it would be half an answer.
    pub async fn origin_status(
        &self,
        domain: Option<&str>,
        detail: bool,
        diff: bool,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if diff && domain.is_none() {
            return Err(EngineError::Invalid("diff needs a domain".to_string()));
        }
        let detail = detail || diff;
        let hidden = self.hidden_for(scope).await?;
        let targets = self.origin_targets(domain, &hidden)?;
        let connection = self.origin_status_connection().await?;

        let actor = crate::scope::overlay_actor(scope);
        // Whose changes a `diff` answers: the acting identity, the way every
        // other surface of this feature resolves one.
        let diff_actor = diff.then(|| match scope {
            crate::scope::Scope::Unrestricted => ShareActor::Owner,
            crate::scope::Scope::User { account, .. } => ShareActor::Account(account.clone()),
            crate::scope::Scope::Anonymous => ShareActor::HttpAgent,
        });
        let mut domains = Vec::new();
        let mut errors = Vec::new();
        for (name, entry) in targets {
            // Read here rather than inside the per-domain body, and that is not
            // tidiness: the body runs under this domain's origin lock, and
            // taking the store lock inside it would invent a lock pair that
            // exists nowhere else in the engine. The counts are a read of rows
            // nobody else in this call touches, so taking them first costs
            // nothing and orders nothing.
            //
            // Only for a domain that reviews changes, and only because the body
            // reports these keys for no other kind: a domain taking changes
            // directly would pay a store lock and two queries per status call
            // for an answer nothing reads, and the instance-wide `/sync`
            // overview asks this of every team domain at once.
            let (view, converged) = if entry.is_overlay() {
                // Whoever owns the domain sees who else is drafting in it. One
                // comparison covers the whole rule: an instance admin owns
                // every domain, a private domain's owner owns theirs, and
                // nobody else ever reaches `Own`.
                //
                // A membership that cannot be read is answered "no" rather than
                // propagated, and both halves of that are deliberate. A
                // permission question with no answer is not a yes, and this
                // function is documented never to fail a whole report over one
                // domain - a `?` here would abort every other domain's status
                // over one unreadable acl row.
                let everyone = match self.domain_right(scope, &name).await {
                    Ok(right) => right >= crate::scope::DomainRight::Own,
                    Err(e) => {
                        tracing::warn!(
                            domain = %name,
                            error = format!("{e:#}"),
                            "who holds domain '{name}' could not be read, so its status says \
                             nothing about who else is drafting there"
                        );
                        false
                    }
                };
                (
                    crate::review::DraftView::new(
                        self.overlay_counts_by_actor(&name).await,
                        actor.clone(),
                        everyone,
                    ),
                    // Read here for the reason the counts are: a plain
                    // in-memory read of what the last pull recorded, which
                    // orders nothing and cannot fail.
                    self.converged_json(&name, actor.as_deref(), everyone),
                )
            } else {
                // Never read: the per-domain body reports the draft keys only
                // for a reviewing domain, which this is not.
                (
                    crate::review::DraftView::new(None, actor.clone(), false),
                    None,
                )
            };
            // Read out here for the reason the counts above are, and it is the
            // same reason: the per-domain body runs under that domain's origin
            // lock, and a reviewing domain's change list takes the store lock,
            // which would be exactly the lock pair this loop exists to keep
            // out of the body. The sides are read under no lock at all, which
            // is the promise `team_local_changes` already makes of every
            // offline read on this path.
            let diff_block = match diff_actor.as_ref() {
                Some(who) => match self.local_change_sides(&name, who).await {
                    Ok(sides) => Some(sides),
                    Err(e) => {
                        errors.push(json!({ "domain": name, "error": e.to_string() }));
                        continue;
                    }
                },
                None => None,
            };
            match self
                .origin_status_one(
                    &name,
                    &entry,
                    detail,
                    diff_block.as_ref(),
                    &view,
                    converged.as_ref(),
                )
                .await
            {
                Ok(v) => domains.push(v),
                Err(e) => errors.push(json!({ "domain": name, "error": e.to_string() })),
            }
        }
        Ok(json!({ "connection": connection, "domains": domains, "errors": errors }))
    }

    /// Reports one domain's status, under its origin lock. The per-domain
    /// body behind `origin_status`'s aggregate loop.
    ///
    /// A live probe is best-effort in two layers: no connection, or a
    /// provider that fails to build, degrades straight to `probe: None`
    /// (unchanged from before). When a provider was resolved but the probe
    /// call itself fails for a transport reason - offline, rate limited, an
    /// expired connection, see [`origin::is_probe_transport_error`] - the
    /// same domain is retried once with no probe at all, so the
    /// offline-capable report still comes back; the probe's own error
    /// message rides along verbatim as `probe_error` instead of aborting
    /// the domain. Any other failure (corrupt local state, and so on) is a
    /// genuine per-domain error, propagated to the caller's `errors` array.
    ///
    /// One thing this read is not allowed to do is settle an owed stack link.
    /// [`ops::status`] can pay that debt off with `create_stack`/`extend_stack`,
    /// which are forge WRITES, and the credential it would spend is the probe's
    /// own: the instance one, since a status carries no actor. In instance mode
    /// that is the credential every write goes out on anyway and the settlement
    /// runs exactly as it always has; in personal mode it would be the one
    /// instance-credential write the wave promises never happens, so permission
    /// is withheld and the debt stays recorded until the next share, amend or
    /// withdrawal pays it off on the acting identity's own credential.
    ///
    /// `detail` is threaded through both arms on purpose. The retry arm is the
    /// offline one, and offline is exactly when a caller cannot look the change
    /// list up anywhere else, so a status that degrades to local state still
    /// names what is unshared rather than dropping the one answer it can still
    /// give from the working tree alone.
    async fn origin_status_one(
        &self,
        name: &str,
        entry: &DomainEntry,
        detail: bool,
        diff: Option<&Value>,
        drafts: &crate::review::DraftView,
        converged: Option<&Value>,
    ) -> Result<Value> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for(name, entry)?;
        // A probe is best-effort: no connection, or a provider that fails to
        // build, must never turn a status call into a hard failure.
        let probe = self.resolve_origin_provider().ok();
        let settle_owed_link = {
            let config = self.config.read().unwrap();
            config.github_stacks() && config.github_share_identity() == ShareIdentityMode::Instance
        };
        let change_detail = || {
            let mut block = detail
                .then(|| origin::local_change_detail(&root, &state_dir))
                .flatten();
            // Both sides ride inside the same block the paths do, so a caller
            // that asked for them reads one thing rather than two. In a
            // reviewing domain the block's own buckets name the working tree's
            // out-of-band files and this is what names the acting actor's
            // drafts, which is the only list a discard there can act on.
            if let (Some(block), Some(sides)) = (block.as_mut(), diff) {
                block["diff"] = sides.clone();
            }
            block
        };
        // In review mode every legitimate change joins its author's draft, so
        // anything the working tree holds that the origin does not got there
        // some other way: an editor, a script, a restored backup. It is
        // reported rather than blocked - the folder belongs to whoever holds
        // the machine - and the key is absent on a domain that takes changes
        // directly, where a local change is ordinary unshared work and
        // `local_changes` already says so.
        // Emitted for every reviewing domain, empty when nothing is known to be
        // unshared - the opposite of the rule `detail` follows beside it, and
        // deliberately. `detail` is absent when the walk never happened because
        // "nothing unshared" and "this could not be told" must not render the
        // same; here the key's presence is what says the domain reviews at all,
        // so making it absent for a domain that has never been pulled would say
        // "this domain takes changes directly", which is a different and wrong
        // thing.
        let out_of_band = entry.is_overlay().then(|| {
            origin::unshared_work(&root, &state_dir)
                .map(|work| work.paths)
                .unwrap_or_default()
        });
        let sharing = crystalline_core::sharing_at(&root);
        let with_out_of_band = |mut value: Value| {
            if let Some(object) = value.as_object_mut() {
                // Read off the folder like the change detail is, so every
                // status surface says which kind of domain it is looking at.
                object.insert("sharing".to_string(), json!(sharing.as_str()));
            }
            if let Some(paths) = &out_of_band
                && let Some(object) = value.as_object_mut()
            {
                object.insert("out_of_band".to_string(), json!(paths));
                // Under the same condition and for the same reason the block
                // above gives: on a reviewing domain every caller is told what
                // they are holding, and whoever owns the domain is told who
                // else is holding anything. On a domain that takes changes
                // directly neither key appears at all - `my_drafts: 0` there
                // would say "you are holding nothing here", which reads as
                // "you could be", and nobody can draft in a domain that is not
                // reviewing.
                object.insert("my_drafts".to_string(), drafts.mine());
                if let Some(everyone) = drafts.everyone() {
                    object.insert("drafts".to_string(), everyone);
                }
                // What the last pull's convergence did here, absent until a
                // pull has converged something or flagged a conflict for this
                // domain - read off the durable record beside the journal, so
                // a restart does not turn "nothing has looked yet" into
                // "nothing converged". Absent rather than zeroed, for the
                // reason `behind` is null when nothing probed it: the two are
                // different answers.
                if let Some(converged) = converged {
                    object.insert("converged".to_string(), converged.clone());
                }
            }
            value
        };
        match ops::status(&spec, &root, &state_dir, probe.as_deref(), settle_owed_link).await {
            Ok(report) => Ok(with_out_of_band(origin::status_report_json(
                name,
                &report,
                None,
                change_detail(),
            ))),
            Err(e) if probe.is_some() && origin::is_probe_transport_error(&e) => {
                // AuthExpired is one of the transport errors this arm catches
                // (see `origin::is_probe_transport_error`), so a probe that
                // failed because the token was revoked drops the cached
                // credential here too; the retry below runs probe-free, so
                // status still comes back offline.
                self.drop_github_credential_on_auth(&e);
                let report = ops::status(&spec, &root, &state_dir, None, settle_owed_link).await?;
                Ok(with_out_of_band(origin::status_report_json(
                    name,
                    &report,
                    Some(e.to_string()),
                    change_detail(),
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Runs one scheduling pass of the background origin poller: checks
    /// whether collaboration is enabled, connected and not paused for a
    /// shared rate limit, then brings every due origin-connected domain up
    /// to date via [`Engine::origin_update_one`], the same per-domain pull
    /// an on-demand `origin_update` runs, under the same per-domain lock, so
    /// a poll tick and a concurrent on-demand update on the same domain
    /// never interleave. This method never talks to GitHub itself; it only
    /// decides which domains are due and delegates the actual pull, so
    /// polling and on-demand updating stay exactly one code path.
    ///
    /// `now` drives every due/not-due decision and `wall_now` is its
    /// wall-clock mirror, recorded alongside every reschedule so
    /// `status_report`'s offline `origins` block can show `next_due` without
    /// ever touching an `Instant` (which carries no epoch and cannot be
    /// serialized). Passing both in, rather than reading `Instant::now()`
    /// and `Utc::now()` here, is what lets a test drive several ticks
    /// deterministically with no real waiting.
    ///
    /// A tick does nothing when collaboration is off (so enabling it later
    /// starts polling on the very next tick, no restart needed), when the
    /// shared rate-limit pause has not yet elapsed or when no GitHub token
    /// is on file (so a `connect` lands and the next tick picks it up
    /// automatically; a debug line notes this at most once an hour). A
    /// domain hitting `RemoteError::RateLimited` pauses every domain until
    /// the reported reset (defaulting an hour out when GitHub reports none)
    /// and ends the tick immediately, since GitHub rate limits are
    /// per-token, not per-repository. Any other per-domain failure (offline,
    /// a revoked token, a corrupt state directory) is recorded quietly and
    /// never stops the tick from moving on to the next due domain.
    pub async fn origin_poll_tick(&self, now: Instant, wall_now: DateTime<Utc>) {
        if !self.config.read().unwrap().github_enabled() {
            return;
        }
        if let Some(until) = self.origin_poller.rate_limited_until() {
            if wall_now < until {
                return;
            }
            self.origin_poller.set_rate_limited_until(None);
        }
        if !self.origin_connection_offline().0 {
            if self.origin_poller.should_log_no_token(now) {
                tracing::debug!(
                    "origin poll: no GitHub connection yet; waiting for connect to resume polling"
                );
            }
            return;
        }
        // The poller is the machine itself rather than a caller, so nothing is
        // subtracted: it polls every origin this daemon hosts.
        let Ok(targets) = self.origin_targets(None, &HashSet::new()) else {
            return;
        };
        let github_poll_secs = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.poll_secs);

        for (name, entry) in targets {
            if !self.origin_poller.is_due(&name, now) {
                continue;
            }
            let domain_poll_secs = entry.origin.as_ref().and_then(|o| o.poll_secs);
            let interval_secs = poller::effective_interval_secs(domain_poll_secs, github_poll_secs);
            let tick = self.origin_poller.next_tick();
            let jitter = poller::jittered_interval(interval_secs, &name, tick);
            let jitter_chrono =
                Duration::from_std(jitter).unwrap_or(Duration::seconds(interval_secs as i64));
            self.origin_poller
                .schedule(&name, now + jitter, wall_now + jitter_chrono);

            match self.origin_update_one(&name, &entry).await {
                Ok(v) => {
                    let up_to_date = v["up_to_date"].as_bool().unwrap_or(false);
                    let applied = v["applied"].as_array().map(Vec::len).unwrap_or(0);
                    let conflict_paths: Vec<&str> = v["conflicts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["path"].as_str())
                        .collect();
                    // A share proposal can transition (merged, declined) with
                    // no file in this domain changing at all, so it needs its
                    // own info line even when the pull otherwise reports
                    // `up_to_date`: `PullReport::proposals` (see
                    // `crystalline_remote::ops::settle_up_to_date`) is
                    // refreshed on every pull regardless of whether the
                    // branch itself moved.
                    let proposal_lines: Vec<String> = v["proposals"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| {
                            let number = p["number"].as_u64().unwrap_or(0);
                            let status = p["status"].as_str().unwrap_or("?");
                            format!("#{number} {status}")
                        })
                        .collect();
                    if v["bootstrapped"].as_bool().unwrap_or(false) {
                        tracing::info!(
                            "origin poll: bootstrapped '{name}' ({} engram(s))",
                            v["engrams"].as_u64().unwrap_or(0)
                        );
                    } else if !conflict_paths.is_empty() {
                        tracing::info!(
                            "origin poll: '{name}' has new conflict(s): {}",
                            conflict_paths.join(", ")
                        );
                    } else if !proposal_lines.is_empty() {
                        tracing::info!(
                            "origin poll: '{name}' proposal update: {}",
                            proposal_lines.join(", ")
                        );
                    } else if !up_to_date {
                        tracing::info!("origin poll: '{name}' applied {applied} file(s)");
                    } else {
                        tracing::debug!("origin poll: '{name}' is up to date");
                    }
                    let outcome = if up_to_date {
                        poller::DomainPollOutcome::UpToDate
                    } else {
                        poller::DomainPollOutcome::Applied {
                            applied,
                            conflicts: conflict_paths.len(),
                        }
                    };
                    self.origin_poller.record_result(&name, outcome);
                }
                Err(EngineError::Remote(RemoteError::RateLimited { reset })) => {
                    let until = reset.unwrap_or_else(|| wall_now + Duration::hours(1));
                    tracing::warn!(
                        "origin poll: GitHub is rate limiting this machine; pausing every domain until {until}"
                    );
                    self.origin_poller.set_rate_limited_until(Some(until));
                    return;
                }
                Err(e) => {
                    tracing::debug!("origin poll: '{name}' failed: {e}");
                    self.origin_poller
                        .record_result(&name, poller::DomainPollOutcome::Error(e.to_string()));
                }
            }
        }
    }

    /// This machine's GitHub connection for `status_report`'s offline
    /// `origins` block: `(connected, token_store)`. Unlike
    /// `origin_connection_json` (used by the live `origin_status` operation,
    /// which reflects an injected test provider's own identity as always
    /// connected), this never special-cases an injected provider: it is a
    /// plain token-store lookup, exactly the same check the poller itself
    /// makes before spending a tick on any domain, so the two never
    /// disagree about whether this machine is connected.
    fn origin_connection_offline(&self) -> (bool, Option<&'static str>) {
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        match self.github_credential(host.as_deref()) {
            Ok((store, Some(_))) => (true, Some(store.kind())),
            _ => (false, None),
        }
    }

    /// Builds `status_report`'s `origins` block entirely offline: this
    /// machine's GitHub connection, the poller's shared rate-limit pause
    /// and, per origin-connected domain, its repo, branch, proposal and
    /// conflict counts and local change count from a probe-free
    /// `ops::status` call (the same state-only read `origin_status` itself
    /// falls back to when a live probe fails), plus the poller's own
    /// schedule and last result for that domain. Every read here is local:
    /// the token store and each domain's saved origin state, never a GitHub
    /// call, so `status` never blocks on the network even when
    /// collaboration is on.
    pub(super) async fn origins_status_block(&self) -> Value {
        let (connected, token_store) = self.origin_connection_offline();
        let rate_limit_wait_until = self.origin_poller.rate_limited_until();
        // `status`'s own block, which the CLI and the control socket read: the
        // machine owner, so nothing is subtracted.
        let targets = self
            .origin_targets(None, &HashSet::new())
            .unwrap_or_default();

        let mut domains = Vec::new();
        for (name, entry) in targets {
            let Ok((spec, root, state_dir)) = self.origin_spec_for(&name, &entry) else {
                continue;
            };
            // No provider, so nothing here could settle an owed stack link
            // anyway; withholding the permission says so at the call rather
            // than leaving it to be inferred from the `None` beside it.
            let Ok(report) = ops::status(&spec, &root, &state_dir, None, false).await else {
                continue;
            };
            let next_due = self.origin_poller.next_due_at(&name);
            let last_result = self.origin_poller.last_result(&name);
            let mut entry =
                origin::origin_poll_status_json(&name, &report, next_due, last_result.as_ref());
            entry["sharing"] = json!(crystalline_core::sharing_at(&root).as_str());
            domains.push(entry);
        }

        json!({
            "enabled": true,
            "connected": connected,
            "token_store": token_store,
            "rate_limit_wait_until": rate_limit_wait_until,
            "domains": domains,
        })
    }

    /// Whose drafts a share of `domain` is of, or `None` when the domain takes
    /// changes directly and a share is the folder walk it always was.
    ///
    /// Asked FIRST by both share verbs, ahead of the credential and ahead of
    /// every forge call: an agent with no identity holds no draft, so there is
    /// nothing for a share to be of whatever this instance's GitHub
    /// configuration says, and the refusal that teaches the way in is the one a
    /// write already gets. Resolving a credential before saying so would answer
    /// a question about the forge to a caller whose problem is that nobody knows
    /// whose work this is.
    fn overlay_share_identity(&self, domain: &str, actor: &ShareActor) -> Result<Option<String>> {
        if !self.reviews_changes(domain) {
            return Ok(None);
        }
        share_staging::overlay_share_actor(actor).map(Some)
    }

    /// The tree a share or a preview of `domain` runs against, or `None` when
    /// `drafting` is `None` and the folder on disk is the answer it always was.
    ///
    /// The single seam review mode adds to sharing, so the share and the preview
    /// behind its confirmation question cannot describe two different things.
    /// Two steps, in this order:
    ///
    /// 1. a pull of the REAL folder, which is the pull `ops::propose` would run
    ///    for itself. It has to happen against the folder rather than the staged
    ///    tree: a pull advances the base snapshot as it applies upstream work,
    ///    so a pull into staging would leave the team's folder behind its own
    ///    base, and no later pull would bring it back. Run here, it lands where
    ///    it belongs;
    /// 2. the staged tree itself ([`Engine::stage_overlay_share`]), and with it
    ///    the commit the folder now stands at. The caller hands that to
    ///    [`crate::share_staging::PinnedHead`], which is what turns "the inline
    ///    pull has nothing left to do" from likely into true.
    ///
    /// `origin` is what [`Engine::origin_spec_for_domain`] resolved, borrowed
    /// whole: the spec, the domain's folder and its origin state directory.
    pub(super) async fn overlay_share_tree(
        &self,
        domain: &str,
        drafting: Option<&str>,
        provider: &dyn Provider,
        origin: (&OriginSpec, &Path, &Path),
        acting: Option<&str>,
    ) -> Result<Option<PreparedShare>> {
        let Some(who) = drafting else {
            return Ok(None);
        };
        let (spec, root, state_dir) = origin;
        let report = ops::pull(provider, spec, root, state_dir)
            .await
            .inspect_err(|e| self.drop_github_credential_on_auth(e))
            .map_err(|e| enrich_write_error(e, acting, &spec.repo))?;
        // This pull advances the base like any other, so the same convergence
        // runs on it - and it runs BEFORE the tree is staged, so a draft the
        // team has already merged is not proposed straight back at them.
        //
        // The sync in front of it is what makes the two call sites one rule
        // rather than two. A rename finishes through `write_overlay_entry`,
        // whose address check reads index rows; against rows that still
        // describe the pre-pull folder the old base row would still hold the
        // draft's address and the destination write would be refused, so the
        // same rename would come out a move here and a divergence there. The
        // scan is incremental, and the share's own tail syncs again after the
        // proposal, where a second pass over unchanged files costs a walk.
        if !report.up_to_date {
            if let Err(e) = self.sync(Some(domain)).await {
                tracing::warn!("indexing what the pull before sharing '{domain}' applied: {e}");
            }
            if let Err(e) = self.converge_pulled_overlays(domain, &report.applied).await {
                tracing::warn!("converging the drafts in '{domain}' before sharing failed: {e}");
            }
        }
        Ok(Some(
            self.stage_overlay_share(domain, who, state_dir).await?,
        ))
    }

    /// Reads what one actor's share is made of - the base snapshot the pull just
    /// settled and that actor's own index rows - and hands both to
    /// [`crate::share_staging::build`].
    ///
    /// Thin on purpose: the filesystem work and its lifetime live in
    /// [`crate::share_staging`], and what belongs here is which two things are
    /// read and with which lookup. **The rows, never the journal beside them**:
    /// the journal is the durable mirror a rebuilt index is restored from, and a
    /// mirror that had fallen behind would quietly change what a share carries.
    /// And **the read-only id lookup**, never an upserting one: a share of a
    /// domain this index has never been told about holds no drafts, and asking
    /// must not register one.
    async fn stage_overlay_share(
        &self,
        domain: &str,
        actor: &str,
        state_dir: &Path,
    ) -> Result<PreparedShare> {
        let state = crystalline_remote::state::OriginState::load(state_dir)?.ok_or_else(|| {
            EngineError::Conflict(format!(
                "domain '{domain}' has no origin state; add the domain from its origin first"
            ))
        })?;
        let view = DomainView::for_actor(self, domain, &HashSet::new(), actor)?;
        Ok(PreparedShare {
            staging: view.materialise(state_dir, &state.files).await?,
            pinned: state.base_commit,
        })
    }

    /// Proposes one domain's local changes as a pull request against its
    /// origin, under its origin lock.
    ///
    /// Refuses with `github.enabled`'s message when collaboration is off,
    /// and with `EngineError::ReadOnly` on a read-only instance (a share
    /// publishes content, exactly what read-only mode protects). When
    /// `ops::propose` refuses because conflicts are still pending, this
    /// degrades that refusal into a `conflicts_pending` outcome carrying the
    /// actual conflict paths (reloaded from the domain's now-current state,
    /// durable on disk since the inline pull inside `propose` already
    /// persisted them) rather than the bare count `RemoteError` alone
    /// carries, so a caller never needs to make a second round trip to learn
    /// what needs resolving. The share itself never touches local files, but
    /// the pull it opens with does, so it ends with the same sync and embed
    /// tail `origin_update_one` runs (see
    /// `Engine::index_what_the_share_pull_applied`).
    ///
    /// `proposal` names an open layer to amend instead of letting the share
    /// pick its own target; `None` is the ordinary call.
    ///
    /// `files` narrows what the share carries to those domain-relative paths,
    /// the generated listings of their folders riding along; `None` is the
    /// whole unshared delta. A path that is not among the domain's unshared
    /// changes refuses the share by name (see
    /// `crystalline_remote::ops::ShareOptions::files`).
    ///
    /// `actor` is who the share runs as, which decides the credential the
    /// forge writes go out on (see [`Engine::resolve_share_provider`]); it is
    /// inert while `github.share_identity` is `instance`, the default.
    ///
    /// The login that credential was connected as is recorded on the proposal
    /// this share creates or rewrites, in both modes
    /// (`crystalline_remote::state::Proposal::author_login`), so a chain whose
    /// layers belong to different people can say so.
    pub async fn origin_share(
        &self,
        domain: &str,
        title: Option<&str>,
        description: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // The policy, read off the REAL folder and never off the tree the
        // share detects in: in review mode that tree holds the actor's own
        // draft of the MANIFEST, and a draft must not switch the review step
        // off for its own author.
        let sharing = crystalline_core::sharing_at(&root);
        let drafting = self.overlay_share_identity(domain, &actor)?;
        if let (Some(who), true) = (drafting.as_deref(), stacks_allowed) {
            self.refuse_open_proposal_while_reviewing(domain, &state_dir, who)?;
        }
        let (provider, login) = self.resolve_share_provider(&actor)?;
        let acting = self.personal_write_login(login.as_deref());
        // In review mode the share is of the actor's own drafts, which are in
        // the index and on nobody's disk, so the tree `ops` detects against is
        // staged here. `None` is a domain that takes changes directly, whose
        // share is the folder walk it always was.
        let staging = self
            .overlay_share_tree(
                domain,
                drafting.as_deref(),
                provider.as_ref(),
                (&spec, &root, &state_dir),
                acting.as_deref(),
            )
            .await?;
        // The share runs against the staged tree, and the provider it runs with
        // holds the inline pull to the commit that tree was staged over: a merge
        // landing in staging would be deleted with it while the base snapshot
        // advanced past it, and nothing would ever put it back.
        let pinned = staging.as_ref().map(|prepared| {
            share_staging::PinnedHead::new(provider.as_ref(), prepared.pinned.clone())
        });
        let share_provider: &dyn Provider = match &pinned {
            Some(pinned) => pinned,
            None => provider.as_ref(),
        };
        let detect_in = staging
            .as_ref()
            .map_or(root.as_path(), |prepared| prepared.staging.root());
        match ops::propose(
            share_provider,
            &spec,
            detect_in,
            domain,
            &state_dir,
            ops::ShareOptions {
                title,
                description,
                proposal,
                stacks_allowed,
                // Who the proposal record names, in either identity mode: the
                // credential this share actually went out on. `None` only when
                // that credential carries no login (the environment token).
                author_login: login.as_deref(),
                files,
                sharing,
            },
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        {
            Ok(outcome) => {
                let mut receipt = origin::propose_outcome_json(&outcome);
                // A reviewing domain's direct commit: the folder is written
                // from the base copies the commit advanced, those paths are
                // indexed, and the convergence pass a merged proposal's pull
                // would run runs now - the next pull finds head == base and
                // would never run it.
                if let (Some(_), ops::ProposeOutcome::Committed(report)) =
                    (drafting.as_deref(), &outcome)
                {
                    let paths: Vec<String> = report
                        .added
                        .iter()
                        .chain(&report.updated)
                        .chain(&report.deleted)
                        .cloned()
                        .collect();
                    // Nothing here may turn a landed commit into an error:
                    // the branch already moved and no retry can take it back,
                    // so a local IO or store failure is warned about and the
                    // `committed` receipt stands. The reviewed folder then
                    // differs from its base copies, which `origin_status`
                    // counts as local changes and `discard_changes` restores.
                    // A pull does not heal it on its own: after the failure
                    // the head equals the base, so the pull is up to date and
                    // runs no convergence; the drafts fold once a later
                    // upstream write touches those paths, or when the person
                    // discards them.
                    if let Err(e) = ops::materialise_base_paths(&root, &state_dir, &paths) {
                        tracing::warn!("writing the direct commit's files in '{domain}': {e}");
                    }
                    if let Err(e) = self.sync_paths(domain, paths.clone()).await {
                        tracing::warn!("indexing the direct commit's files in '{domain}': {e}");
                    }
                    match self.converge_pulled_overlays(domain, &paths).await {
                        Ok(folded) => receipt["drafts_folded"] = json!(folded.cleared),
                        Err(e) => {
                            tracing::warn!("folding the shared drafts in '{domain}': {e}");
                            // The one key that says the commit landed and the
                            // drafts behind it did not fold, in place of the
                            // count a fold that ran would have carried.
                            receipt["fold_error"] = json!(e.to_string());
                        }
                    }
                }
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
                // Whose the proposal is, in the sense review mode means it. The
                // forge record cannot say (see
                // `Engine::refuse_open_proposal_while_reviewing`), so it is
                // recorded here, beside the drafts it carried.
                if let Some(who) = drafting.as_deref()
                    && let Some(number) = proposal_number_of(&receipt)
                {
                    self.record_proposal_actor(domain, number, Some(who));
                }
                Ok(receipt)
            }
            Err(RemoteError::ConflictsPending { count }) => {
                // The conflicts refusal is the loudest case for syncing: the
                // pull ran, applied everything that merged cleanly and only
                // then refused, so this shape carries the most unindexed work
                // of any share outcome.
                self.index_what_the_share_pull_applied(domain, "sharing")
                    .await;
                let conflicts = crystalline_remote::state::OriginState::load(&state_dir)
                    .ok()
                    .flatten()
                    .map(|s| s.conflicts)
                    .unwrap_or_default();
                Ok(json!({
                    "outcome": "conflicts_pending",
                    "count": count,
                    "conflicts": conflicts,
                }))
            }
            Err(e) => Err(enrich_write_error(e, acting.as_deref(), &spec.repo).into()),
        }
    }

    /// Refuse a share that would stack a layer on an open one while the domain
    /// reviews changes - in the words of whose that open proposal is.
    ///
    /// Two things a stacked layer needs are not there in review mode. A layer
    /// is detected against the CHAIN TIP, which may be another actor's open
    /// layer - a member would be proposing a change on top of work they never
    /// wrote and may not even be shown. And amending a layer replays the layers
    /// above it from the working tree, which in a reviewing domain holds no
    /// layer's content at all: the folder says what the team reviewed, and
    /// every layer lives in somebody's overlay.
    ///
    /// **So a reviewing domain carries one proposal at a time, and that costs
    /// two different refusals rather than one.** The proposal is this actor's
    /// own, and withdrawing it is a thing they may do
    /// ([`REVIEW_NO_STACKING`]); or it is somebody else's, and it is not
    /// theirs to withdraw, so the honest answer names whose it is and what to
    /// wait for ([`REVIEW_PROPOSAL_IS_ANOTHERS`]). One refusal for both would
    /// tell a member who has never shared anything to "share a fresh proposal
    /// instead", which is precisely what they were doing.
    ///
    /// **Whose it is comes from the journal's own record**, not from the forge
    /// record's `author_login`: that names the GitHub account a share's
    /// credential was connected as, and in the default instance identity mode
    /// every actor's share goes out on one login. A proposal opened before this
    /// was recorded, or by another machine, has no owner here and reads as
    /// somebody else's - the safe way round, since it never tells one actor to
    /// withdraw a proposal that is not theirs.
    ///
    /// The refusal stays narrow: it needs a forge that actually serves stacks
    /// (probed once and recorded) and a layer already open, so the first share
    /// of a domain, and every share on a forge that stacks nothing, goes
    /// through exactly as it did.
    pub(super) fn refuse_open_proposal_while_reviewing(
        &self,
        domain: &str,
        state_dir: &Path,
        actor: &str,
    ) -> Result<()> {
        let Some(state) = crystalline_remote::state::OriginState::load(state_dir)
            .ok()
            .flatten()
            .filter(|state| state.stacks_available == Some(true))
        else {
            return Ok(());
        };
        let Some(open) = state
            .proposals
            .iter()
            .find(|p| p.status == crystalline_remote::state::ProposalStatus::Open)
        else {
            return Ok(());
        };
        let record = self
            .journal_state_dir()
            .map(|dir| crate::overlay_journal::journal_record(&dir, domain))
            .unwrap_or_default();
        let owner = record.proposals.get(&open.number.to_string());
        if owner.is_some_and(|who| who == actor) {
            return Err(EngineError::Refused(REVIEW_NO_STACKING.to_string()));
        }
        let named = owner.map_or("another member", String::as_str);
        Err(EngineError::Refused(
            REVIEW_PROPOSAL_IS_ANOTHERS.replace("{actor}", named),
        ))
    }

    /// Indexes whatever the pull inside a share or a preview wrote to the
    /// working tree.
    ///
    /// Both `ops::propose` and `ops::propose_preview` pull first - freshness is
    /// part of proposing honestly - and a pull applies upstream files. Without
    /// this, those files sit on disk unsearchable until the poller's next tick
    /// happens to sync them, which is `origin_update_one`'s bug with a
    /// different call in front of it. Run unconditionally rather than only when
    /// something looks applied: the sync is incremental, so a pull that changed
    /// nothing costs a cheap no-op scan, and there is no cheaper signal here
    /// that is also correct (a preview reports the share's plan, not the pull's
    /// effect).
    ///
    /// Best effort in the same sense `origin_update_one`'s embed tail is: a
    /// failure is logged, never turned into a failed share whose proposal is
    /// already open on the forge.
    async fn index_what_the_share_pull_applied(&self, domain: &str, verb: &str) {
        if let Err(e) = self.sync(Some(domain)).await {
            tracing::warn!("syncing '{domain}' after {verb} failed: {e}");
            return;
        }
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after {verb} '{domain}' failed: {e}");
        }
    }

    /// Previews what a share of one domain would do, under its origin lock,
    /// without making a single provider write.
    ///
    /// The same three gates `origin_share` applies apply here: a preview runs
    /// the real pull first (freshness is part of previewing honestly), so it
    /// writes the working tree and is refused on a read-only instance exactly
    /// as a share is - and, for the same reason, it ends with the same sync and
    /// embed tail (see `Engine::index_what_the_share_pull_applied`).
    ///
    /// It carries the share's own credential resolution too, `actor` and all:
    /// a preview that resolved a different identity than the share would could
    /// promise a plan this instance then refuses to perform.
    ///
    /// `credential` is the one place that resolution bends, and only for the
    /// personal-token refusal (see [`PreviewCredential`]): a caller that asks
    /// for [`PreviewCredential::ReadScopeFallback`] gets the plan computed on
    /// the instance credential when the acting identity has connected none of
    /// its own, which is the browser's case - the checkbox list a person picks
    /// files in is fed by this call, so refusing it would make connecting a
    /// hoop in front of an unknown. Every other refusal stands for both
    /// callers, the acting login is `None` on that path (nothing personal was
    /// resolved to name), and the share itself still refuses.
    pub async fn origin_share_preview(
        &self,
        domain: &str,
        title: Option<&str>,
        proposal: Option<u64>,
        files: Option<&[String]>,
        actor: ShareActor,
        credential: PreviewCredential,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // Read exactly where the share reads it, and for the same reason: off
        // the folder the team reviewed, never off the tree the preview detects
        // in (see `Engine::origin_share`).
        let sharing = crystalline_core::sharing_at(&root);
        let drafting = self.overlay_share_identity(domain, &actor)?;
        if let (Some(who), true) = (drafting.as_deref(), stacks_allowed) {
            // The preview carries the share's own gates, so nobody is asked to
            // confirm a share this instance would then refuse.
            self.refuse_open_proposal_while_reviewing(domain, &state_dir, who)?;
        }
        let (provider, login) = match self.resolve_share_provider(&actor) {
            Ok(resolved) => resolved,
            Err(e)
                if credential == PreviewCredential::ReadScopeFallback
                    && is_personal_token_missing(&e) =>
            {
                (self.resolve_origin_provider()?, None)
            }
            Err(e) => return Err(e),
        };
        let acting = self.personal_write_login(login.as_deref());
        // The share's own staging, for the share's own reason: a preview that
        // planned against the folder would promise a share of files nobody
        // drafted (see `Engine::overlay_share_tree`).
        let staging = self
            .overlay_share_tree(
                domain,
                drafting.as_deref(),
                provider.as_ref(),
                (&spec, &root, &state_dir),
                acting.as_deref(),
            )
            .await?;
        // Pinned exactly as the share pins it, and for the same reason: a
        // preview's own inline pull would merge into the staged tree too.
        let pinned = staging.as_ref().map(|prepared| {
            share_staging::PinnedHead::new(provider.as_ref(), prepared.pinned.clone())
        });
        let preview_provider: &dyn Provider = match &pinned {
            Some(pinned) => pinned,
            None => provider.as_ref(),
        };
        let detect_in = staging
            .as_ref()
            .map_or(root.as_path(), |prepared| prepared.staging.root());
        let plan = ops::propose_preview(
            preview_provider,
            &spec,
            detect_in,
            domain,
            &state_dir,
            ops::ShareOptions {
                title,
                description: None,
                proposal,
                stacks_allowed,
                // Carried for the same reason the provider is: a preview
                // resolves exactly what the share would. It records nothing.
                author_login: login.as_deref(),
                files,
                sharing,
            },
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        .map_err(|e| enrich_write_error(e, acting.as_deref(), &spec.repo))?;
        self.index_what_the_share_pull_applied(domain, "previewing a share")
            .await;
        // Provenance preselection is a guess at whose work a mixed delta holds,
        // and in review mode there is nothing left to guess: every path in the
        // plan is one of this actor's own drafts. So the plan is served without
        // it rather than with a column answering a question nobody asked.
        let provenance = drafting.is_none().then_some(root.as_path());
        let mut plan = origin::share_plan_json(&plan, provenance);
        // The repository beside the branch, so a confirmation question can say
        // where a commit goes without a second lookup.
        plan["repo"] = json!(spec.repo);
        Ok(plan)
    }

    /// Previews which proposal a withdrawal would take out, without touching
    /// the forge at all.
    ///
    /// A pure local read: the offline status path ([`ops::status`] with no
    /// probe) reports this domain's open and declined proposals off origin
    /// state, and [`origin::withdraw_plan_json`] resolves the target out of
    /// that exactly as [`ops::withdraw`] would, refusing with the same
    /// teaching errors when no single target can be named. Nothing is written
    /// and no provider call is made, which is what lets an eliciting client
    /// ask its user before a pull request is closed.
    ///
    /// It still carries the withdrawal's own gates, all of them and in the
    /// same order - collaboration off, read-only, an unregistered domain, and
    /// a provider this instance cannot build - rather than only the read's, so
    /// a user is never asked to confirm a withdrawal this instance would
    /// refuse to perform. The provider is resolved and dropped: an instance
    /// with no credential on file has to fail in round one, where the failure
    /// is still the answer to the call, rather than after the user has said
    /// yes to a question.
    ///
    /// The provider it resolves and drops is the withdrawal's own, `actor`
    /// included, so an instance that shares personally refuses here - before
    /// the question - when the acting identity has no connection of its own.
    pub async fn origin_withdraw_preview(
        &self,
        domain: &str,
        proposal: Option<u64>,
        revert: bool,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // The withdrawal's own identity gate, in round one: on a reviewing
        // domain a revert puts one actor's overlay back, and nobody in
        // particular has none to put back.
        self.overlay_share_identity(domain, &actor)?;
        let (_provider, _login) = self.resolve_share_provider(&actor)?;
        // Probe-free, so no forge call of any kind: the settlement permission
        // is withheld for the same reason the provider was dropped.
        let report = ops::status(&spec, &root, &state_dir, None, false).await?;
        Ok(origin::withdraw_plan_json(
            &report,
            proposal,
            revert,
            stacks_allowed,
        )?)
    }

    /// Withdraws a share proposal for one domain: closes its pull request on
    /// the forge, best-effort deletes its branch, optionally restores the
    /// shared files (`revert`) and records it as withdrawn. Under the
    /// domain's origin lock; syncs and embeds afterward only when files
    /// moved. Refuses when collaboration is off and on a read-only instance.
    ///
    /// `actor` decides the credential the close and the branch delete go out
    /// on, exactly as it does for a share.
    pub async fn origin_withdraw(
        &self,
        domain: &str,
        proposal: Option<u64>,
        revert: bool,
        actor: ShareActor,
    ) -> Result<Value> {
        let stacks_allowed = {
            let config = self.config.read().unwrap();
            if !config.github_enabled() {
                return Err(RemoteError::NotEnabled.into());
            }
            config.github_stacks()
        };
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (spec, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // Whose withdrawal this is, on a domain that reviews changes - and the
        // refusal an agent with no identity gets, in the words every draft verb
        // refuses in. `None` is a domain that takes changes directly, where a
        // revert is the working-tree restore it always was.
        let drafting = self.overlay_share_identity(domain, &actor)?;
        // What the chain holds BEFORE the withdrawal, so an overlay revert can
        // find the record it is undoing. Read here and not back off the saved
        // state afterwards: the ordinary path settles the record into
        // `history`, which `OriginState::push_history` caps at twenty, so a
        // busy domain can evict the very proposal this call just withdrew - and
        // a revert that then found nothing would quietly restore nothing while
        // the receipt said the withdrawal had worked.
        let open_before: Vec<crystalline_remote::state::Proposal> = if drafting.is_some() && revert
        {
            crystalline_remote::state::OriginState::load(&state_dir)?
                .map(|state| state.proposals)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let (provider, login) = self.resolve_share_provider(&actor)?;
        let acting = self.personal_write_login(login.as_deref());
        let mut report = ops::withdraw(
            provider.as_ref(),
            &spec,
            &root,
            &state_dir,
            proposal,
            // A revert of a reviewing domain never reaches the folder: what
            // the withdrawal undoes is the actor's own overlay, and the folder
            // on disk is what the team reviewed and nobody withdrew.
            revert && drafting.is_none(),
            stacks_allowed,
        )
        .await
        .inspect_err(|e| self.drop_github_credential_on_auth(e))
        .map_err(|e| enrich_write_error(e, acting.as_deref(), &spec.repo))?;

        if drafting.is_some() {
            // The proposal is gone from the chain, so whose it was is no longer
            // a question anything asks.
            self.record_proposal_actor(domain, report.number, None);
        }
        if let (Some(who), true) = (drafting.as_deref(), revert) {
            self.revert_into_overlay(domain, who, &open_before, &mut report)
                .await?;
            // Nothing on disk moved, so there is nothing to sync: the overlay
            // writes nudged the embedder themselves.
            return Ok(origin::withdraw_report_json(&report));
        }

        if !report.restored.is_empty() || !report.deleted.is_empty() {
            self.sync(Some(domain)).await?;
            if !self.request_embed()
                && let Err(e) = self.embed_pending().await
            {
                tracing::warn!(
                    "embedding after withdrawing proposal #{} for '{domain}' failed: {e}",
                    report.number
                );
            }
        }
        Ok(origin::withdraw_report_json(&report))
    }

    /// Puts one actor's own view back the way it stood before the withdrawn
    /// proposal was shared: every path that proposal carried stops being their
    /// draft.
    ///
    /// The undo is a clear and never a write, and that follows from what a
    /// review-mode share is. A share of a reviewing domain is made ENTIRELY of
    /// the acting actor's overlay entries, so the state before it was "this
    /// actor held nothing at these paths" - a proposed addition, a proposed
    /// rewrite and a proposed deletion all go back to the same thing, which is
    /// the base showing through again.
    ///
    /// **It reverts the ACTING actor's overlay, whosever the proposal was.**
    /// Nothing in this task restricts a withdrawal to the proposal's author, so
    /// an actor withdrawing somebody else's proposal finds none of its paths in
    /// their own overlay and reverts nothing - an empty `restored` and
    /// `deleted` beside a withdrawal that did happen. That is the honest
    /// outcome of the two rules meeting (a revert is per actor, a withdrawal is
    /// not) and not a silent failure of either; who may withdraw whose proposal
    /// is a question the program has not answered yet.
    ///
    /// **A draft edited since it was shared is never touched**, the rule
    /// [`ops::withdraw`]'s own revert keeps for a file: the recorded digest is
    /// what says whether what stands here is still what was proposed, and
    /// anything else is newer work. Such a path is named in `skipped_diverged`
    /// exactly as the folder path would be.
    ///
    /// The one case a revert would have to WRITE rather than clear - a path a
    /// lower layer of a stack added and this layer changed, whose pre-share
    /// content is that layer's blob - cannot arise here, because stacking on an
    /// open layer is refused outright while a domain reviews
    /// ([`Engine::origin_share`]). If stacks ever reach review mode, this is
    /// the half that has to learn to write.
    async fn revert_into_overlay(
        &self,
        domain: &str,
        actor: &str,
        open_before: &[crystalline_remote::state::Proposal],
        report: &mut crystalline_remote::ops::WithdrawReport,
    ) -> Result<()> {
        // The record as it stood before the withdrawal resolved its own target,
        // matched on the number the withdrawal reports.
        let Some(withdrawn) = open_before.iter().find(|p| p.number == report.number) else {
            return Ok(());
        };
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let Some(domain_id) = domain_id else {
            return Ok(());
        };
        for file in &withdrawn.files {
            let held = {
                let store = self.store.lock().await;
                store.overlay_entry(domain_id, actor, &file.path).await?
            };
            let Some(held) = held else {
                continue;
            };
            // What was proposed, against what this actor holds now. A
            // tombstone proposed a deletion and carries no content of its own,
            // so its digest is the absence the proposal recorded.
            let unchanged = match file.sha256.as_deref() {
                Some(proposed) => {
                    !held.tombstone && sha256_hex(held.content.as_bytes()) == proposed
                }
                None => held.tombstone,
            };
            if !unchanged {
                report.skipped_diverged.push(file.path.clone());
                continue;
            }
            DomainView::for_actor(self, domain, &HashSet::new(), actor)?
                .drop(domain_id, &file.path)
                .await?;
            match file.change {
                // A page only this actor had goes away with the proposal.
                crystalline_remote::state::ProposedChange::Added => {
                    report.deleted.push(file.path.clone());
                }
                // A rewrite or a deletion of a page the team has: the team's
                // own version shows through again.
                _ => report.restored.push(file.path.clone()),
            }
        }
        Ok(())
    }

    /// What one actor holds in a reviewing domain, rows and files, at the
    /// spelling and in the order the change list reports: `(path, kind,
    /// current bytes)` with `None` bytes for a tombstone, sorted by path.
    ///
    /// The kind is the folder's answer: `deleted` for a tombstone, `modified`
    /// where the folder holds the path, `added` where it does not. That is
    /// exactly the set a review-mode share stages
    /// ([`crate::share_staging`]), so the list and the share agree.
    async fn overlay_local_changes(
        &self,
        domain: &str,
        root: &Path,
        actor: &str,
    ) -> Result<Vec<(String, &'static str, Option<Vec<u8>>)>> {
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let mut out: Vec<(String, &'static str, Option<Vec<u8>>)> = Vec::new();
        if let Some(domain_id) = domain_id {
            let rows = {
                let store = self.store.lock().await;
                store.overlay_entries(domain_id, actor).await?
            };
            for row in rows {
                if !origin::takes_part_in_local_change(&row.path) {
                    continue;
                }
                let kind = if row.tombstone {
                    "deleted"
                } else if root.join(&row.path).is_file() {
                    "modified"
                } else {
                    "added"
                };
                let bytes = (!row.tombstone).then(|| row.content.into_bytes());
                out.push((row.path, kind, bytes));
            }
        }
        let state_dir = self.journal_state_dir()?;
        for entry in crate::overlay_files::entries(&state_dir, domain, actor).entries {
            if !origin::takes_part_in_local_change(&entry.path) {
                continue;
            }
            let kind = if entry.tombstone {
                "deleted"
            } else if root.join(&entry.path).is_file() {
                "modified"
            } else {
                "added"
            };
            let bytes = if entry.tombstone {
                None
            } else {
                crate::overlay_files::read(&state_dir, domain, actor, &entry.path).map_err(
                    |source| EngineError::Io {
                        path: entry.path.clone(),
                        source,
                    },
                )?
            };
            out.push((entry.path, kind, bytes));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// The folder's own bytes at a review-mode draft's path, or `None` where
    /// the folder holds nothing there. Only ever asked for a path the actor's
    /// overlay already names, which is what keeps the join off the raw
    /// request: every such path was validated by the verb that wrote it.
    fn folder_side(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(root.join(path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(EngineError::Io {
                path: path.to_string(),
                source,
            }),
        }
    }

    /// The team-domain walk every method below shares: the origin state, the
    /// base it detects against and the detected delta. Under no lock; a read
    /// a beat stale is the promise every offline read here makes.
    pub(super) fn team_local_changes(&self, domain: &str) -> Result<TeamChanges> {
        #[cfg(any(test, feature = "testing"))]
        self.detection_walks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let state = crystalline_remote::state::OriginState::load(&state_dir)?.ok_or_else(|| {
            EngineError::Invalid(format!("domain '{domain}' has no origin state"))
        })?;
        let base = ops::unshared_base(&state);
        let local = crystalline_remote::changes::detect_local_changes(&root, &base)?;
        Ok((root, state_dir, base, local))
    }

    /// The domain's own refusal for a caller that asked about local changes
    /// where there are none to have: a domain with no team origin shares
    /// nothing, so nothing of it is unshared.
    fn local_changes_need_an_origin(&self, domain: &str) -> Result<()> {
        if !self.domain_has_origin(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain '{domain}' has no team origin; only a team domain has local changes"
            )));
        }
        Ok(())
    }

    /// Every unshared change of `domain`, offline: `{ domain, mode, changes,
    /// skipped_large }`. A team domain answers the working tree against
    /// `unshared_base`, the comparison a share makes; a reviewing domain
    /// answers the acting actor's own drafts against the folder, and an
    /// anonymous HTTP agent, who holds no draft there, an empty list. Never
    /// pulls, never probes, never resolves a provider.
    pub async fn local_changes(&self, domain: &str, actor: &ShareActor) -> Result<Value> {
        self.local_changes_listing(domain, actor, false).await
    }

    /// The same envelope as [`Engine::local_changes`] with both sides of every
    /// change inlined: each entry is what [`Engine::local_change`] answers for
    /// that path, uncapped, and in the same order.
    ///
    /// One detection walk for the whole envelope, which is the point of it:
    /// asking [`Engine::local_change`] per listed path would re-run the walk
    /// (a read and hash of every file in the domain, or a reviewing domain's
    /// overlay read) once per change, so a domain with a hundred unshared
    /// changes paid for a hundred walks to answer one call.
    #[doc(hidden)]
    pub async fn local_changes_detailed(&self, domain: &str, actor: &ShareActor) -> Result<Value> {
        self.local_changes_listing(domain, actor, true).await
    }

    /// The body both listings share. `sides` decides only what each entry
    /// carries: the summary row, or that row with both texts on it.
    async fn local_changes_listing(
        &self,
        domain: &str,
        actor: &ShareActor,
        sides: bool,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        self.local_changes_need_an_origin(domain)?;
        let entry = |path: &str, kind: &str, base: Option<&[u8]>, current: Option<&[u8]>| {
            if sides {
                // Uncapped, because a caller deciding what to discard reads
                // the whole file rather than a preview of it.
                let mut value = origin::change_detail_json(path, kind, base, current, None);
                value["domain"] = json!(domain);
                value
            } else {
                origin::change_entry_json(path, kind, base, current)
            }
        };
        if self.reviews_changes(domain) {
            let Ok(who) = share_staging::overlay_share_actor(actor) else {
                return Ok(
                    json!({ "domain": domain, "mode": "review", "changes": [], "skipped_large": [] }),
                );
            };
            let (_, root, _) = self.origin_spec_for_domain(domain)?;
            let mut changes = Vec::new();
            for (path, kind, current) in self.overlay_local_changes(domain, &root, &who).await? {
                let base = Self::folder_side(&root, &path)?;
                changes.push(entry(&path, kind, base.as_deref(), current.as_deref()));
            }
            return Ok(
                json!({ "domain": domain, "mode": "review", "changes": changes, "skipped_large": [] }),
            );
        }
        let (root, state_dir, _, local) = self.team_local_changes(domain)?;
        let mut changes = Vec::new();
        for change in local.substantive() {
            let Some(found) = ops::local_change_sides(&root, &state_dir, &local, change.path())?
            else {
                continue;
            };
            changes.push(entry(
                change.path(),
                change_kind(change),
                found.base.as_deref(),
                found.current.as_deref(),
            ));
        }
        let skipped: Vec<Value> = local
            .skipped_large
            .iter()
            .map(|(path, size)| json!({ "path": path, "size": size }))
            .collect();
        Ok(
            json!({ "domain": domain, "mode": "team", "changes": changes, "skipped_large": skipped }),
        )
    }

    /// Both sides of every unshared change of `domain`, in the order
    /// [`Engine::local_changes`] reports them: what `origin_status`'s `diff`
    /// block carries: the changes array of
    /// [`Engine::local_changes_detailed`]'s envelope, so the whole block costs
    /// the one walk that listing makes.
    async fn local_change_sides(&self, domain: &str, actor: &ShareActor) -> Result<Value> {
        let mut listed = self.local_changes_detailed(domain, actor).await?;
        Ok(listed["changes"].take())
    }

    /// Both sides of one unshared change, or `NotFound` in the words
    /// `select_share_files` refuses an unknown path in. `cap` bounds a text
    /// side; `None` answers every text.
    pub async fn local_change(
        &self,
        domain: &str,
        path: &str,
        actor: &ShareActor,
        cap: Option<usize>,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        self.local_changes_need_an_origin(domain)?;
        let not_a_change = || {
            EngineError::NotFound(format!(
                "not among this domain's unshared changes: {path}; take the paths from the change list, which names every file that differs"
            ))
        };
        if self.reviews_changes(domain) {
            let who = share_staging::overlay_share_actor(actor)?;
            let (_, root, _) = self.origin_spec_for_domain(domain)?;
            let found = self
                .overlay_local_changes(domain, &root, &who)
                .await?
                .into_iter()
                .find(|(p, _, _)| p == path)
                .ok_or_else(not_a_change)?;
            let (path, kind, current) = found;
            let base = Self::folder_side(&root, &path)?;
            let mut value =
                origin::change_detail_json(&path, kind, base.as_deref(), current.as_deref(), cap);
            value["domain"] = json!(domain);
            return Ok(value);
        }
        let (root, state_dir, _, local) = self.team_local_changes(domain)?;
        let sides =
            ops::local_change_sides(&root, &state_dir, &local, path)?.ok_or_else(not_a_change)?;
        let mut value = origin::change_detail_json(
            sides.change.path(),
            change_kind(&sides.change),
            sides.base.as_deref(),
            sides.current.as_deref(),
            cap,
        );
        value["domain"] = json!(domain);
        Ok(value)
    }

    /// Put the named paths back the way the team has them: `{ domain,
    /// restored, deleted, cleared: [{ path, kind }], refused: [{ path, reason }],
    /// reindexed }`. Under the domain's origin lock like a withdrawal, so it
    /// never interleaves with a share's pull; refuses on a read-only instance.
    /// A team domain restores from the base copy and re-indexes exactly the
    /// touched paths; a reviewing domain clears the acting actor's own drafts
    /// through [`DomainView::drop`] and `overlay_files::clear` and moves
    /// nothing in the folder. One refused path never stops the others.
    pub async fn discard_local_changes(
        &self,
        domain: &str,
        targets: &[DiscardTarget],
        actor: &ShareActor,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.local_changes_need_an_origin(domain)?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let drafting = self.overlay_share_identity(domain, actor)?;
        if let Some(who) = drafting {
            return self.discard_into_overlay(domain, &who, targets).await;
        }
        let (root, state_dir, base, local) = self.team_local_changes(domain)?;
        let targets = fill_unguarded(targets, &local);
        let report = ops::discard_local_files(&root, &state_dir, &base, &local, &targets)?;
        let touched: Vec<String> = report
            .restored
            .iter()
            .chain(report.deleted.iter())
            .map(|p| local.disk_path(p).to_string())
            .collect();
        let reindexed = touched.len();
        if !touched.is_empty() {
            // The targeted pass the import already takes: exactly these paths,
            // which is also what rebuilds a folder's listing beside a discarded
            // engram in a domain that shares its listings.
            self.sync_paths(domain, touched).await?;
            if !self.request_embed()
                && let Err(e) = self.embed_pending().await
            {
                tracing::warn!("embedding after discarding changes in '{domain}' failed: {e}");
            }
        }
        let refused: Vec<Value> = report
            .refused
            .iter()
            .map(|(path, reason)| json!({ "path": path, "reason": reason.code() }))
            .collect();
        Ok(json!({
            "domain": domain,
            "restored": report.restored,
            "deleted": report.deleted,
            "cleared": [],
            "refused": refused,
            "reindexed": reindexed,
        }))
    }

    /// The review-mode half of [`Engine::discard_local_changes`]: a clear and
    /// never a write, for the reason [`Engine::revert_into_overlay`] gives.
    /// A draft somebody has open in a live editor is refused rather than
    /// closed under them, since the room's own save would write it straight
    /// back.
    async fn discard_into_overlay(
        &self,
        domain: &str,
        actor: &str,
        targets: &[DiscardTarget],
    ) -> Result<Value> {
        let (_, root, _) = self.origin_spec_for_domain(domain)?;
        let state_dir = self.journal_state_dir()?;
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        let mut cleared: Vec<Value> = Vec::new();
        let mut refused: Vec<Value> = Vec::new();
        for target in targets {
            let path = target
                .path
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();
            // A generated listing is never a change of this feature, so it is
            // never a path a discard can name, whatever the MANIFEST says
            // about sharing listings. The team arm gets this from
            // `resolve_local_change`; here it is the same rule written out,
            // rather than something that happens to hold because no verb
            // writes such a row today.
            if !origin::takes_part_in_local_change(&path) {
                refused.push(json!({ "path": path, "reason": "unknown_path" }));
                continue;
            }
            let row = match domain_id {
                Some(domain_id) => {
                    let store = self.store.lock().await;
                    store.overlay_entry(domain_id, actor, &path).await?
                }
                None => None,
            };
            if let (Some(domain_id), Some(held)) = (domain_id, row) {
                // No digest is "discard what the list showed me", the way an
                // agent calling without `expected` means it; a digest is the
                // guard that newer work is never dropped.
                // The digest is of the row's own content, the way
                // [`Engine::revert_into_overlay`] takes it and the way the
                // change list reports it: a row's stamp column carries
                // whatever the write recorded there, which is a fact about a
                // file the draft may not have.
                let unchanged = match target.sha256.as_deref() {
                    Some(expected) => {
                        !held.tombstone && sha256_hex(held.content.as_bytes()) == expected
                    }
                    None => true,
                };
                if !unchanged {
                    refused.push(json!({ "path": path, "reason": "changed_since" }));
                    continue;
                }
                let open = match self.collab_rooms() {
                    Some(rooms) => {
                        rooms
                            .has_live_room(domain, &held.permalink, Some(actor))
                            .await
                    }
                    None => false,
                };
                if open {
                    refused.push(json!({ "path": path, "reason": "open_in_editor" }));
                    continue;
                }
                let kind = if held.tombstone {
                    "deleted"
                } else if root.join(&path).is_file() {
                    "modified"
                } else {
                    "added"
                };
                DomainView::for_actor(self, domain, &HashSet::new(), actor)?
                    .drop(domain_id, &path)
                    .await?;
                cleared.push(json!({ "path": path, "kind": kind }));
                continue;
            }
            let io = |source: std::io::Error| EngineError::Io {
                path: path.clone(),
                source,
            };
            // A path the files overlay will not even address holds nothing
            // there, which is the same answer as an empty folder: the caller
            // named something this domain has no change at. A refusal to
            // address is not a filesystem failure, so it is answered rather
            // than raised.
            let held = match crate::overlay_files::held(&state_dir, domain, actor, &path) {
                Ok(held) => held,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
                    refused.push(json!({ "path": path, "reason": "unknown_path" }));
                    continue;
                }
                Err(e) => return Err(io(e)),
            };
            match held {
                crate::overlay_files::Held::Nothing => {
                    refused.push(json!({ "path": path, "reason": "unknown_path" }));
                }
                crate::overlay_files::Held::Bytes => {
                    let bytes = crate::overlay_files::read(&state_dir, domain, actor, &path)
                        .map_err(io)?
                        .unwrap_or_default();
                    if target
                        .sha256
                        .as_deref()
                        .is_some_and(|expected| expected != sha256_hex(&bytes))
                    {
                        refused.push(json!({ "path": path, "reason": "changed_since" }));
                        continue;
                    }
                    crate::overlay_files::clear(&state_dir, domain, actor, &path).map_err(io)?;
                    let kind = if root.join(&path).is_file() {
                        "modified"
                    } else {
                        "added"
                    };
                    cleared.push(json!({ "path": path, "kind": kind }));
                }
                crate::overlay_files::Held::Tombstone => {
                    // A deletion's current side is absence, so a caller that
                    // looked at bytes was looking at something else.
                    if target.sha256.is_some() {
                        refused.push(json!({ "path": path, "reason": "changed_since" }));
                        continue;
                    }
                    crate::overlay_files::clear(&state_dir, domain, actor, &path).map_err(io)?;
                    cleared.push(json!({ "path": path, "kind": "deleted" }));
                }
            }
        }
        Ok(json!({
            "domain": domain,
            "restored": [],
            "deleted": [],
            "cleared": cleared,
            "refused": refused,
            "reindexed": 0,
        }))
    }

    /// One conflict's full detail: both recorded sides plus the current local
    /// content, addressed by id or by path (exactly one must be given; if
    /// both arrive the id wins and the path is ignored, never mixed, so an id
    /// lookup can never be answered by some other conflict that happens to
    /// match the path). Neither is `EngineError::Invalid` rather than a
    /// misleading not-found. Sides are returned as UTF-8 strings; a side that
    /// exists but is not UTF-8 comes back null with `note` saying so. A pure
    /// read: no gate beyond the domain being registered with an origin, no
    /// lock needed.
    pub async fn origin_conflict_detail(
        &self,
        domain: &str,
        id: Option<&str>,
        path: Option<&str>,
    ) -> Result<Value> {
        if id.is_none() && path.is_none() {
            return Err(EngineError::Invalid(
                "origin_conflict_detail needs an id or a path".to_string(),
            ));
        }
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        let state = crystalline_remote::state::OriginState::load(&state_dir)?.ok_or_else(|| {
            EngineError::Invalid(format!("domain '{domain}' has no origin state"))
        })?;
        let conflict = state
            .conflicts
            .iter()
            .find(|c| match (id, path) {
                (Some(id), _) => c.id == id,
                (None, Some(path)) => c.path == path,
                (None, None) => false,
            })
            .cloned()
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no open conflict {} for '{domain}'",
                    id.or(path).unwrap_or("(none named)")
                ))
            })?;
        let (base, upstream) =
            crystalline_remote::state::read_conflict_files(&state_dir, &conflict.id)?;
        let local_path = root.join(&conflict.path);
        let local = match std::fs::read(&local_path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(EngineError::Io {
                    path: local_path.display().to_string(),
                    source: e,
                });
            }
        };
        let mut note: Option<String> = None;
        let base_v = utf8_side(base, "base", &mut note);
        let local_v = utf8_side(local, "local", &mut note);
        let upstream_v = utf8_side(upstream, "upstream", &mut note);
        Ok(json!({
            "id": conflict.id,
            "path": conflict.path,
            "kind": conflict.kind,
            "detected_at": conflict.detected_at,
            "base": base_v,
            "local": local_v,
            "upstream": upstream_v,
            "note": note,
        }))
    }

    /// Resolves one recorded conflict for one domain, under its origin lock,
    /// then syncs the domain (and embeds) since resolving writes the
    /// working tree.
    ///
    /// `keep` is `"mine"` or `"theirs"`; exactly one of `keep` or `content`
    /// must be supplied (see [`origin::resolution_from`]). Refuses with
    /// `github.enabled`'s message when collaboration is off, and with
    /// `EngineError::ReadOnly` on a read-only instance.
    ///
    /// `actor` makes no provider call of its own - resolving writes this
    /// machine and reaches the forge later, on the next share, under whoever
    /// performs that. What it decides is WHOSE the resolution is: on a domain
    /// that reviews changes the conflict being settled is one actor's draft
    /// standing against a base that moved under it, so the resolution joins
    /// that actor's overlay and never the folder the team reviewed. An agent
    /// with no identity is refused there in the words every draft verb refuses
    /// in; on a domain that takes changes directly the identity changes
    /// nothing, exactly as before.
    pub async fn origin_resolve(
        &self,
        domain: &str,
        path: &str,
        keep: Option<&str>,
        content: Option<&[u8]>,
        actor: ShareActor,
    ) -> Result<Value> {
        if !self.config.read().unwrap().github_enabled() {
            return Err(RemoteError::NotEnabled.into());
        }
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // An attachment is not an engram and this verb settles engrams. The
        // path would be normalized to `<path>.md` on the next line and then
        // miss everything, so the refusal comes first and says what to do
        // instead: the bytes are settled by writing them again or by deleting
        // them, which is the same pair of verbs that put them there.
        if is_assets_reserved(path) {
            return Err(EngineError::Invalid(format!(
                "'{path}' is an attachment, and a conflict resolution settles an engram's \
                 markdown; upload the file again to keep your version or delete it to take the \
                 one the team has"
            )));
        }
        let resolution = origin::resolution_from(keep, content)?;
        let lock = self.origin_lock_registered(domain)?;
        let _guard = lock.lock().await;
        let (_, root, state_dir) = self.origin_spec_for_domain(domain)?;
        // One spelling from here down. A caller who names `notes/plan` and a
        // caller who names `notes/plan.md` mean one engram, and the draft
        // lookup, the recorded-conflict lookup and `ops::resolve` have to be
        // asked about the same one or a missing suffix would turn a resolution
        // into a refusal.
        let path = &normalize_md(path);
        if let Some(who) = self.overlay_share_identity(domain, &actor)? {
            let held = {
                let store = self.store.lock().await;
                match store.domain_id(domain).await? {
                    Some(domain_id) => {
                        Some((domain_id, store.overlay_entry(domain_id, &who, path).await?))
                    }
                    None => None,
                }
            };
            match held {
                Some((domain_id, Some(_))) => {
                    return self
                        .resolve_in_overlay(domain, domain_id, &who, path, resolution)
                        .await;
                }
                // Nothing of this caller's stands here. A conflict the pull
                // recorded is the FOLDER's and never anybody's draft: it got
                // there because the reviewed folder was edited out of band and
                // upstream then changed the same file, and settling it is what
                // puts the folder back level with its own base. So it is
                // settled the way it always was, on the folder - review mode is
                // not a reason to leave a conflict standing in the team's own
                // files with no verb that can reach it. The overlay rows are
                // untouched by that path: a sync writes the base dimension and
                // a draft is somebody else's row entirely.
                _ if !self.folder_conflict_at(&state_dir, path)? => {
                    return Err(EngineError::NotFound(format!(
                        "you are not drafting '{path}' in domain '{domain}', and no conflict \
                         stands there either. This domain reviews changes, so what a resolution \
                         settles is your own draft: draft the change first, then settle it"
                    )));
                }
                _ => {}
            }
        }
        let report = ops::resolve(&root, &state_dir, path, resolution)?;

        self.sync(Some(domain)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after resolving a conflict for '{domain}' failed: {e}");
        }

        Ok(json!({
            "resolved": report.resolved,
            "remaining": report.remaining,
        }))
    }

    /// Whether the domain's own origin state records a conflict at `path`: one
    /// the pull put in the folder every actor shares, which no draft of
    /// anybody's is or ever was.
    fn folder_conflict_at(&self, state_dir: &Path, path: &str) -> Result<bool> {
        Ok(crystalline_remote::state::OriginState::load(state_dir)?
            .is_some_and(|state| state.conflicts.iter().any(|c| c.path == path)))
    }

    /// Settles one actor's conflict inside their own overlay: the draft that
    /// was standing against a base that moved under it.
    ///
    /// Three answers and one shape. Keeping the team's version ENDS the draft -
    /// what the folder says is what this actor reads at that path again - and
    /// merged content becomes their new draft, written through the one overlay
    /// writer so it carries the address rule every draft carries. Keeping their
    /// own changes nothing on purpose: the draft as it stands IS the answer,
    /// and the conflict was never a record for a write to clear, only the last
    /// pull's reading of a draft against a folder that had moved. All three
    /// settle the conflict, so all three take it out of what
    /// [`Engine::converged_json`] reports.
    ///
    /// The domain's own conflict records are deliberately untouched: those are
    /// [`ops::resolve`]'s, they belong to the folder every actor shares, and
    /// clearing one from inside a draft would settle it on everybody's behalf.
    ///
    /// **No recorded conflict is required**, and that is a widening worth
    /// naming: any path this actor is drafting can be resolved, whether or not
    /// a pull ever flagged it. Requiring the record would make a conflict
    /// unsettleable whenever the record could not be read, and the arms are
    /// what a draft's author may already do to their own draft anyway - the
    /// merged arm is `write_engram` by another name, and the `theirs` arm is
    /// their own deletion of their own draft.
    async fn resolve_in_overlay(
        &self,
        domain: &str,
        domain_id: DomainId,
        actor: &str,
        path: &str,
        resolution: ops::Resolution<'_>,
    ) -> Result<Value> {
        let view = DomainView::for_actor(self, domain, &HashSet::new(), actor)?;
        match resolution {
            ops::Resolution::Mine => {}
            ops::Resolution::Theirs => {
                view.drop(domain_id, path).await?;
            }
            ops::Resolution::Merged(bytes) => {
                let text = std::str::from_utf8(bytes).map_err(|_| {
                    EngineError::Invalid(
                        "the merged content is not valid UTF-8, so it is not an engram".to_string(),
                    )
                })?;
                view.write(domain_id, path, text).await?;
            }
        }
        let remaining = self.settle_convergence(domain, actor, path);
        Ok(json!({
            "resolved": path,
            "remaining": remaining,
            // The receipt says where the resolution landed: in this actor's
            // draft, not in the folder the team reviewed.
            "draft": true,
        }))
    }

    /// Resolves a single domain's `OriginSpec`, root and state directory for
    /// `origin_share`, `origin_withdraw` and `origin_resolve`: each a
    /// single-domain operation unlike `origin_update`/`origin_status`'s
    /// optional "every domain" mode. Errors with `UnknownDomain` when
    /// unregistered, and with the same "has no origin" message
    /// `origin_spec_for` raises when registered but not origin-connected.
    pub(super) fn origin_spec_for_domain(
        &self,
        domain: &str,
    ) -> Result<(OriginSpec, PathBuf, PathBuf)> {
        let entry = self.domain_entry(domain)?;
        self.origin_spec_for(domain, &entry)
    }

    /// The domains `origin_update`/`origin_status` operate on: the one named
    /// (erroring if it is not registered or has no origin) or every
    /// registered domain with an origin, mirroring `sync_targets`'s
    /// config-then-discovered layering.
    ///
    /// `hidden` is the caller's own set of domains it may not see, and it binds
    /// both arms. A named one is refused exactly as an unregistered one, with
    /// the registered list in the error filtered to what this caller may see.
    /// The unnamed arm - "every domain with an origin" - drops them, which is
    /// the whole of what makes the aggregate form safe: without it a stranger
    /// asking for the standing of "every shared domain" is handed a private
    /// team domain's name, its open proposals and its conflicts, and
    /// `origin_update` additionally pulls into it. Every machine-owner caller
    /// (the CLI, the control socket, the poller, the status block) passes an
    /// empty set, which is the answer they would resolve to anyway.
    fn origin_targets(
        &self,
        domain: Option<&str>,
        hidden: &HashSet<String>,
    ) -> Result<Vec<(String, DomainEntry)>> {
        match domain {
            Some(name) => {
                let entry = self.domain_entry_scoped(name, hidden)?;
                if entry.origin.is_none() {
                    return Err(EngineError::Invalid(format!(
                        "domain '{name}' has no origin; connect it with `crystalline domain add --origin`"
                    )));
                }
                Ok(vec![(name.to_string(), entry)])
            }
            None => {
                let mut out: Vec<(String, DomainEntry)> = Vec::new();
                let config = self.config.read().unwrap();
                for (name, entry) in &config.domains {
                    if entry.origin.is_some() && !hidden.contains(name) {
                        out.push((name.clone(), entry.clone()));
                    }
                }
                for (name, entry) in self.discovered_domains.read().unwrap().iter() {
                    if config.domains.contains_key(name) {
                        continue;
                    }
                    if entry.origin.is_some() && !hidden.contains(name) {
                        out.push((name.clone(), entry.clone()));
                    }
                }
                Ok(out)
            }
        }
    }

    /// The `OriginSpec`, domain root and origin state directory for a
    /// registered domain's origin.
    fn origin_spec_for(
        &self,
        name: &str,
        entry: &DomainEntry,
    ) -> Result<(OriginSpec, PathBuf, PathBuf)> {
        let origin_cfg = entry
            .origin
            .as_ref()
            .ok_or_else(|| EngineError::Invalid(format!("domain '{name}' has no origin")))?;
        let root = entry.file_path().ok_or_else(|| {
            EngineError::Invalid(format!(
                "domain '{name}' has no filesystem root to sync an origin into"
            ))
        })?;
        let state_dir = self.origin_state_dir(name)?;
        let spec = OriginSpec {
            repo: origin_cfg.repo.clone(),
            subpath: origin_cfg.path.clone(),
            branch: origin_cfg.branch().to_string(),
        };
        Ok((spec, root, state_dir))
    }

    /// [`Engine::origin_lock`] for the single-domain operations whose next step
    /// requires a registered domain: the name is checked first, so a lock entry
    /// is never created for a name that is about to fail anyway. The error is
    /// exactly the `UnknownDomain` [`Engine::origin_spec_for_domain`] would
    /// raise a line later, so a registered name behaves identically and an
    /// unregistered one answers the same, only without leaving an entry behind.
    pub(super) fn origin_lock_registered(
        &self,
        domain: &str,
    ) -> Result<Arc<tokio::sync::Mutex<()>>> {
        self.domain_entry(domain)?;
        Ok(self.origin_lock(domain))
    }

    /// The per-domain lock serializing origin operations for one domain
    /// name, created lazily on first use. Callers that already hold a
    /// `DomainEntry`, and `origin_add` (whose domain may not be registered
    /// yet), use this directly; every other single-domain caller goes through
    /// [`Engine::origin_lock_registered`].
    pub(super) fn origin_lock(&self, domain: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.origin_locks.lock().unwrap();
        locks
            .entry(domain.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The per-file lock every content write holds across its whole
    /// read-decide-write, created lazily on first use and keyed by the file's
    /// canonical path.
    ///
    /// What it closes is a time-of-check-to-time-of-use race, not a partial
    /// write. Each of the file-domain writes looks at the world and then acts
    /// on what it saw, and between those two steps another writer fits:
    ///
    /// - [`Engine::save_engram`], [`Engine::save_manifest`] and
    ///   [`Engine::delete_engram`] read the file, hash it and compare that
    ///   against the caller's `expected_checksum`. Unlocked, two saves of one
    ///   engram arriving together (two browser tabs, or one tab whose autosave
    ///   overlaps a manual save) both read the same text, both find their token
    ///   fresh and both write. One author's version then wins on disk while the
    ///   other is told the save succeeded, which is precisely the outcome
    ///   `If-Match` exists to prevent.
    /// - [`Engine::write_engram_as`] checks that the permalink is free and then
    ///   creates the file. Two creates of one title would both find it free and
    ///   both write, and the second would answer 201 over the first's body
    ///   rather than the 409 that says it was already taken.
    /// - [`Engine::edit_engram_as`] and [`Engine::retire_engram_as`] serialize
    ///   their read-modify-write under the lock: each reads the file, applies
    ///   its operation to that text and writes the result. Two edits, or an
    ///   agent's edit racing a browser's save, would each compute from a
    ///   version the other has already replaced, and the last write would
    ///   silently drop the other's change; under the lock the second one reads
    ///   the first's result and applies to that instead. `edit_engram_as`
    ///   additionally compares an `expected_checksum` there when one is
    ///   supplied (see [`crate::params::EditParams`]), refusing a stale edit
    ///   rather than merely serializing it. A retirement takes its successor's
    ///   lock too, for the reciprocal line it appends there, but never at the
    ///   same time as its target's.
    ///
    /// Held across the whole sequence, the second caller sees the first's bytes
    /// and either refuses or builds on them.
    ///
    /// Keyed by the file's own identity rather than by `domain/permalink`, so
    /// two domains registered over one root, or over two spellings of one path,
    /// still serialize on the file itself: the key is
    /// [`canonicalize`](std::fs::canonicalize)d where the filesystem can
    /// resolve it, which covers symlinks and `..` segments, and falls back to
    /// the path as given for a file that does not exist yet - a create and a
    /// save of one engram therefore share a key only once the file is there,
    /// which is exactly when both are reading it. Taken before the store lock,
    /// always, so the two never invert; virtual domains take neither, since
    /// their compare-and-swap happens inside a single database statement.
    ///
    /// The map is never pruned, like [`Engine::origin_locks`]: an entry is a
    /// path string and an `Arc`, and the set of files a process ever writes is
    /// bounded by the installation.
    ///
    /// **In-process only.** Two Crystalline processes over one domain root are
    /// not held apart by this; the host-lock machinery governs that.
    pub(super) fn write_lock(&self, abs: &Path) -> Arc<tokio::sync::Mutex<()>> {
        // Computed before the map-wide guard is taken: `canonicalize` is a
        // blocking stat, and every other file's lookup would otherwise queue
        // behind it while it resolves this one's path.
        let key = lock_key(abs);
        let mut locks = self.write_locks.lock().unwrap();
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The one lock a draft is written under, for every verb that writes one.
    ///
    /// A draft has no file in the domain's folder - that is the whole of review
    /// mode - so the base file's lock holds nothing apart from it: in a
    /// reviewing domain nobody writes the folder at all, and two writers of one
    /// draft that took it would be two writers holding a mutex neither of them
    /// contends. The draft's own mirror in the overlay journal IS the file this
    /// write produces, so its path is the key, and every arm that writes a
    /// draft takes it: the capture, the edit, the save, the delete and both
    /// halves of a move.
    ///
    /// What the lock is held across is a read-modify-write with no compare
    /// behind it. A capture replaces the whole row; an edit reads the draft,
    /// applies its operation and writes the result; a save compares a checksum
    /// it read a moment ago. Each store write is atomic on its own, which is
    /// exactly why a lost update here is invisible - the capture's receipt says
    /// it landed and the edit that resumed with the older text quietly replaces
    /// it. Serializing them makes the loser read the winner's bytes and either
    /// refuse (a stale checksum, a taken permalink) or build on them.
    ///
    /// Keyed through [`crate::overlay_journal::entry_path`] rather than by
    /// hand, so the lock and the mirror can never come to name two different
    /// places, and taken before the store lock like every other holder. See
    /// [`Engine::write_lock`], whose map this shares: a base file's path and a
    /// draft mirror's path are different keys in one map, so no cycle is formed
    /// and a domain that takes changes directly is untouched by this.
    ///
    /// Keyed on the PATH, so two captures by one actor at two paths that both
    /// claim one permalink are not serialized against each other and both
    /// land; the conflict target is `(domain_id, path, actor)`, so no store
    /// constraint backstops it. The direct-mode file arm has the same hole
    /// under the same key, so this is parity, not a regression (whole-branch
    /// re-review, 2026-09-17).
    pub(super) fn draft_lock(
        &self,
        domain: &str,
        actor: &str,
        path: &str,
    ) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let state_dir = self.journal_state_dir()?;
        let mirror = crate::overlay_journal::entry_path(&state_dir, domain, actor, path)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        Ok(self.write_lock(&mirror))
    }

    /// The base directory per-domain origin state lives under: the test
    /// override, or the real state directory.
    fn origins_base_dir(&self) -> Result<PathBuf> {
        match &self.origins_dir_override {
            Some(p) => Ok(p.clone()),
            None => crystalline_core::config::origins_state_dir()
                .map_err(|e| EngineError::Internal(e.to_string())),
        }
    }

    /// One domain's origin state directory (base snapshot, conflict records,
    /// `state.json`).
    ///
    /// `pub(crate)` rather than private: [`crate::nudge::memo_key`] keys its
    /// share-walk memo on this path beside a domain's folder, because two
    /// domain names can register the same folder under different origins
    /// directories, and the memoized answer depends on which one.
    pub(crate) fn origin_state_dir(&self, domain: &str) -> Result<PathBuf> {
        Ok(self.origins_base_dir()?.join(domain))
    }

    /// The state directory the overlay journal lives under: the test override,
    /// or the real one. `<state_dir>/overlays/<domain>/<actor>/<path>` is the
    /// journal's own layout, which [`crate::overlay_journal`] owns.
    /// **This resolver performs no I/O, and that is load bearing.** Its failure
    /// means "this process knows no path at all", a fact about the environment,
    /// which is why [`Engine::overlay_domain_files`] may read it as nothing
    /// there is rather than as something it cannot see: nothing can ever have
    /// been written through a resolver that answers no path. A `create_dir_all`
    /// or a `canonicalize` added here would turn a real permission failure on
    /// the state root into that same answer, and the silent omission that arm
    /// is safe from today would reopen. Every directory failure is detected
    /// strictly after this call, inside `overlay_files::by_actor`.
    pub(crate) fn journal_state_dir(&self) -> Result<PathBuf> {
        match &self.state_dir_override {
            Some(p) => Ok(p.clone()),
            // **Under the test seam this refuses instead of falling back**, and
            // it is the one resolver in this file that does. Every other one
            // reaching a real machine path costs a test a read; this one is
            // reached by `journal_remove_domain`, which is
            // `std::fs::remove_dir_all` under `<state_dir>/overlays/<domain>`.
            // A fixture that forgot [`Engine::with_state_dir`] would delete a
            // developer's own drafts by domain name, silently (the sweep is
            // best effort) and unrecoverably (the journal is the only copy of a
            // draft a rebuild cannot make again) - and would read their real
            // journal into its test index on the way. An audit of the fixtures
            // is not enough; the resolver has to say no.
            #[cfg(any(test, feature = "testing"))]
            None => Err(EngineError::Internal(
                "this engine was built without a state directory, so it reaches no overlay \
                 journal: a test that touches drafts, a removal or a sync must say where the \
                 journal lives with `Engine::with_state_dir`"
                    .to_string(),
            )),
            #[cfg(not(any(test, feature = "testing")))]
            None => crystalline_core::config::state_dir()
                .map_err(|e| EngineError::Internal(e.to_string())),
        }
    }

    /// Resolves the provider an origin operation runs its GitHub calls
    /// through: the injected test provider when one is set, or a fresh
    /// `GitHubProvider` built from the current config and the cached GitHub
    /// token (read from the OS keychain at most once per process, see
    /// [`Engine::github_credential`]). A `connect` earlier this same process
    /// is picked up without a restart - the connect refreshes the cache - and
    /// a machine that has not connected yet is never cached, so a later
    /// connect is seen too. Errors with `RemoteError::NotConnected` when no
    /// token has been saved and no test provider is injected.
    fn resolve_origin_provider(&self) -> Result<Arc<dyn Provider>> {
        if let Some(p) = &self.origin_provider_override {
            return Ok(p.clone());
        }
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        let (_store, token) = self.github_credential(host.as_deref())?;
        let token = token.ok_or(RemoteError::NotConnected)?;
        Ok(Arc::new(GitHubProvider::new(
            api_url,
            Some(token.access_token),
        )))
    }

    /// Resolves the provider a WRITE verb runs its GitHub calls through, plus
    /// the login it acts as (the acting `StoredToken.user`, for a proposal's
    /// recorded author).
    ///
    /// The read side is [`Engine::resolve_origin_provider`] and never moves.
    /// This one splits by `github.share_identity`, read LIVE on every call
    /// rather than snapshotted at start: the setting is `startup_effective:
    /// false`, so a mode flipped through `configure` is honoured by the very
    /// next share with no restart.
    ///
    /// - `instance` (the default): byte for byte what the read side does, the
    ///   one instance credential, whoever the actor is.
    /// - `personal`: the actor's own credential -
    ///   [`ShareActor::Owner`] the fixed `owner` slot, [`ShareActor::Account`]
    ///   that account's, [`ShareActor::HttpAgent`] the account
    ///   `github.agent_identity` names. No personal token on file refuses with
    ///   a teaching text; there is no fallback to the instance credential, by
    ///   design (spec section 6).
    ///
    /// The test provider override short-circuits BOTH modes: an injected mock
    /// has no credential behind it to read a login off, so the login it acts as
    /// is whatever [`Engine::with_origin_provider_login`] supplied beside it -
    /// `None` unless a test asked for one, which is the same answer as a
    /// credential that names nobody.
    pub(super) fn resolve_share_provider(
        &self,
        actor: &ShareActor,
    ) -> Result<(Arc<dyn Provider>, Option<String>)> {
        if let Some(p) = &self.origin_provider_override {
            return Ok((p.clone(), self.origin_provider_override_login.clone()));
        }
        let (api_url, token) = self.resolve_share_credential(actor)?;
        let login = token.user_display().map(str::to_string);
        Ok((
            Arc::new(GitHubProvider::new(api_url, Some(token.access_token))),
            login,
        ))
    }

    /// The credential half of [`Engine::resolve_share_provider`]: the api url
    /// and the token a write goes out on, before an HTTP client exists.
    ///
    /// Split out because this is where every decision lives - the mode, the
    /// actor mapping, the two refusals - while building the client is
    /// mechanical, and because a `reqwest` client build loads the platform
    /// trust store, which is slow enough to be worth keeping out of the tests
    /// that exercise this matrix.
    pub(super) fn resolve_share_credential(
        &self,
        actor: &ShareActor,
    ) -> Result<(Option<String>, StoredToken)> {
        let (api_url, mode, agent_identity) = {
            let config = self.config.read().unwrap();
            (
                config.github.as_ref().and_then(|g| g.api_url.clone()),
                config.github_share_identity(),
                config.github_agent_identity().map(str::to_string),
            )
        };
        let identity = match mode {
            ShareIdentityMode::Instance => TokenIdentity::Instance,
            ShareIdentityMode::Personal => {
                TokenIdentity::Personal(self.acting_identity_name(actor, agent_identity)?)
            }
        };
        let host = origin::token_host(api_url.as_deref());
        let (_store, token) = self.github_credential_for(&identity, host.as_deref())?;
        let token = match token {
            Some(token) => token,
            // The two absences are different failures and read differently: an
            // instance with no credential at all is simply not connected, while
            // an instance that shares personally and holds no token for THIS
            // identity is connected and still refusing, which is the case that
            // needs teaching.
            None if identity == TokenIdentity::Instance => {
                return Err(RemoteError::NotConnected.into());
            }
            None => return Err(RemoteError::Refused(PERSONAL_TOKEN_MISSING.to_string()).into()),
        };
        Ok((api_url, token))
    }

    /// The login a write failure is enriched in the name of: the acting login
    /// when this instance shares personally, `None` otherwise.
    ///
    /// Instance-token failures keep today's texts (spec section 8), so the
    /// teaching in [`enrich_write_error`] must not fire for them - and the mode
    /// is read live here for the same reason it is read live in
    /// [`Engine::resolve_share_provider`], one call after it, so the two agree
    /// about which credential the call in flight actually used.
    fn personal_write_login(&self, login: Option<&str>) -> Option<String> {
        match self.config.read().unwrap().github_share_identity() {
            ShareIdentityMode::Personal => login.map(str::to_string),
            ShareIdentityMode::Instance => None,
        }
    }

    /// The personal identity name a write runs under, in personal mode.
    ///
    /// The machine owner has no account to be, so it gets the one fixed local
    /// name; an account is itself, whether it signed in to Fluid or
    /// authenticated an MCP session; an unauthenticated HTTP-MCP agent is
    /// whoever `github.agent_identity` names, or a refusal that says which
    /// setting to write.
    fn acting_identity_name(
        &self,
        actor: &ShareActor,
        agent_identity: Option<String>,
    ) -> Result<String> {
        let name = match actor {
            ShareActor::Owner => OWNER_IDENTITY_NAME.to_string(),
            ShareActor::Account(name) => name.clone(),
            ShareActor::HttpAgent => agent_identity.ok_or_else(|| {
                EngineError::Remote(RemoteError::Refused(AGENT_IDENTITY_UNSET.to_string()))
            })?,
        };
        // Normalization belongs to the layer that mints these names - the auth
        // store (`crate::rest::auth_store`) trims and lowercases an account name
        // before it is ever stored, and the settings layer holds
        // `github.agent_identity` to the same shape - so the engine asserts the
        // invariant rather than quietly re-normalizing and hiding a surface that
        // stopped honouring it. The token store refuses a malformed name anyway;
        // this is the earlier, louder signal in a debug build.
        debug_assert_eq!(
            name,
            name.trim().to_lowercase(),
            "account names reach the engine already trimmed and lowercased"
        );
        Ok(name)
    }

    /// This machine's GitHub connection, for `origin_status`: `{ connected,
    /// user, token_store }`. With an injected test provider, reflects the
    /// mock's own identity instead of the real token store, so origin tests
    /// never touch the OS keychain or a real credential file. `user` renders
    /// as JSON `null` rather than an empty string for the environment token
    /// store, whose synthesized identity has no login attached (see
    /// `StoredToken::user_display`).
    pub(super) async fn origin_connection_json(&self) -> Result<Value> {
        if let Some(provider) = &self.origin_provider_override {
            let user = provider.current_user().await.ok();
            return Ok(json!({ "connected": true, "user": user, "token_store": "file" }));
        }
        let api_url = self
            .config
            .read()
            .unwrap()
            .github
            .as_ref()
            .and_then(|g| g.api_url.clone());
        let host = origin::token_host(api_url.as_deref());
        let (store, token) = self.github_credential(host.as_deref())?;
        Ok(json!({
            "connected": token.is_some(),
            "user": token.as_ref().and_then(|t| t.user_display()),
            "token_store": store.kind(),
        }))
    }

    /// How many of one team domain's unshared substantive changes `account`
    /// last wrote, by the changed file's own `generated.by` line.
    ///
    /// `None` for a domain with no origin state to compare against, which is
    /// the same answer a domain with no origin gets: nothing is known to be
    /// unshared, so nothing is known to be anybody's. Never an error - this
    /// enriches a report, and a report survives an unreadable working tree.
    ///
    /// Last-writer provenance, never authorship: it says which actor wrote the
    /// revision on disk, not who the knowledge belongs to.
    ///
    /// Under the domain's [`Engine::origin_lock`], for the reason
    /// [`Engine::share_facts`] gives: a status read that races a share would
    /// otherwise compare a half-written pair and attribute a delta nobody made.
    pub async fn owned_local_changes(&self, domain: &str, account: &str) -> Option<u64> {
        let lock = self.origin_lock(domain);
        let _guard = lock.lock().await;
        let (_spec, root, state_dir) = self.origin_spec_for_domain(domain).ok()?;
        let work = origin::unshared_work(&root, &state_dir)?;
        Some(work.owned_by(&root, &format!("human:{account}")))
    }

    /// [`Engine::origin_connection_json`] plus the two facts a caller needs to
    /// know WHICH credential a share of theirs would go out on: the mode
    /// (`share_identity`, always) and, in personal mode, the machine owner's
    /// own connection (`owner_identity`, absent in instance mode because there
    /// is no personal slot in play).
    ///
    /// Only `origin_status` is enriched, not [`Engine::origin_connection_json`]
    /// itself: the settings screen and the `configure` snapshot report the
    /// instance connection, and the personal identities they care about are the
    /// SESSION's, served by `/me/github-identity`.
    ///
    /// Two slots, because two callers resolve two different credentials.
    /// `owner_identity` is the machine OWNER's, which is what a CLI or
    /// stdio-MCP share resolves ([`Engine::acting_identity_name`]).
    /// `agent_identity` is the one an HTTP-MCP peer's share runs as
    /// (`github.agent_identity`), and it is reported on exactly the same terms:
    /// personal mode only, and only when that setting names an account at all,
    /// because an unset agent slot is not a connection somebody has failed to
    /// make - it is a deployment that has no HTTP agent sharing on it. Both
    /// carry `{ account, connected, user }`, and neither is a claim about who
    /// the reader is: a caller reads the slot it would share on.
    ///
    /// A credential that cannot be resolved reports as not connected rather
    /// than failing the whole status read: this is a report, and every other
    /// line of it survives an unreadable credential store.
    ///
    /// The cost, stated rather than hidden: only a PRESENT token is cached
    /// ([`Engine::github_credential_for`]), so an instance in personal mode
    /// whose owner has connected nothing pays one credential-store read per
    /// `origin status` - deliberate, since it is also what lets a standalone
    /// `crystalline connect github --personal` be seen without a restart.
    async fn origin_status_connection(&self) -> Result<Value> {
        let mut connection = self.origin_connection_json().await?;
        let (mode, agent) = {
            let config = self.config.read().unwrap();
            (
                config.github_share_identity(),
                config.github_agent_identity().map(str::to_string),
            )
        };
        connection["share_identity"] = json!(mode.as_str());
        if mode == ShareIdentityMode::Personal {
            connection["owner_identity"] =
                self.personal_slot_json(OWNER_IDENTITY_NAME, &connection);
            // Only where an HTTP agent has a slot at all: an absent
            // `github.agent_identity` means no share ever runs as one, and a
            // slot reported for it would read as a connection somebody forgot
            // to make.
            if let Some(agent) = agent.as_deref() {
                connection["agent_identity"] = self.personal_slot_json(agent, &connection);
            }
        }
        Ok(connection)
    }

    /// One personal identity slot for the status connection block:
    /// `{ account, connected, user }`, read from the credential store for
    /// `account`.
    ///
    /// A credential that cannot be resolved reports as not connected rather
    /// than failing the whole status read.
    ///
    /// With an injected test provider the store is never touched at all -
    /// reading it would reach the machine's real keychain from a test - and the
    /// login is deliberately null rather than the mock's: that login belongs to
    /// the injected INSTANCE provider, and reporting it here would invent a
    /// personal connection nobody made, the one thing this must never do, since
    /// the whole point of the slot is to say whether a share can go out at all.
    fn personal_slot_json(&self, account: &str, connection: &Value) -> Value {
        let (connected, user) = if self.origin_provider_override.is_some() {
            (
                connection["connected"].as_bool().unwrap_or(false),
                Value::Null,
            )
        } else {
            let identity = TokenIdentity::Personal(account.to_string());
            let host = self.github_token_host();
            let token = self
                .github_credential_for(&identity, host.as_deref())
                .ok()
                .and_then(|(_store, token)| token);
            (
                token.is_some(),
                json!(token.as_ref().and_then(|t| t.user_display())),
            )
        };
        json!({
            "account": account,
            "connected": connected,
            "user": user,
        })
    }

    /// The INSTANCE GitHub token store for `host` and the token it holds -
    /// [`Engine::github_credential_for`] with the instance identity - reading
    /// the OS keychain at most once per process. Every read verb and every
    /// connection-status surface goes through here; only a write in personal
    /// mode addresses another identity. The environment token wins first
    /// (`CRYSTALLINE_GITHUB_TOKEN`, via `self.overlay`; keyring-free and never
    /// cached, so unsetting it is picked up live); then a cached present-token
    /// for this host; then the resolved store - the test file override (see
    /// [`Engine::with_token_store_dir`], a plain file that never touches the
    /// real OS keychain), or the real `TokenStore::resolve_and_load`, whose
    /// single `get_password` both picks the backend and loads the token. Only
    /// a present token is cached: a `None` stays live so a later `connect`
    /// (here, or from a standalone CLI writing the same keychain item) is seen
    /// on the next call without a restart. Replaces the old per-operation
    /// resolve-then-load double read that turned every origin op into two
    /// keychain touches.
    ///
    /// The environment wins over the test override too, so a poller or connect
    /// test can prove the env token is actually what gets used even when a
    /// token directory is also wired up.
    pub(super) fn github_credential(
        &self,
        host: Option<&str>,
    ) -> Result<(TokenStore, Option<StoredToken>)> {
        self.github_credential_for(&TokenIdentity::Instance, host)
    }

    /// [`Engine::github_credential`] for any identity: the instance credential
    /// or one person's personal one, cached per identity and host so two
    /// identities can never be served the same client.
    ///
    /// The environment token is instance-only and stays that way. One process
    /// serves everybody who reaches it, so a single `CRYSTALLINE_GITHUB_TOKEN`
    /// cannot mean alice's token for one request and bob's for the next; a
    /// personal identity resolves through the keyring or the file store, even on
    /// a machine that sets the variable.
    pub(super) fn github_credential_for(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<(TokenStore, Option<StoredToken>)> {
        if *identity == TokenIdentity::Instance
            && let Some(token) = self.overlay.github_token()
        {
            let store = TokenStore::env(token, host);
            let stored = store.load()?;
            return Ok((store, stored));
        }
        let key = credential_cache_key(identity, host);
        // The std mutex is held across the keychain read on a cache miss on
        // purpose: the critical section never awaits, and single-flighting the
        // first touch under the lock collapses N concurrent first reads (a
        // daemon resolving several team domains at once) into a single keychain
        // prompt instead of a race of N. Every later call is a cache hit and
        // never reaches the read.
        let mut cache = self.github_tokens.lock().unwrap();
        if let Some(cached) = cache.get(&key) {
            return Ok((cached.store.clone(), Some(cached.token.clone())));
        }
        let (store, token) = match &self.token_store_dir_override {
            Some(dir) => {
                let store = TokenStore::file_fallback_for(identity, dir)?;
                let token = store.load()?;
                (store, token)
            }
            None => {
                let base = self.origins_base_dir()?;
                TokenStore::resolve_and_load_for(identity, host, &base)?
            }
        };
        if let Some(token) = &token {
            cache.insert(
                key,
                CachedGithub {
                    store: store.clone(),
                    token: token.clone(),
                },
            );
        }
        Ok((store, token))
    }

    /// The plan a connect flow saves the INSTANCE credential through.
    pub(super) fn github_save_plan(&self, host: Option<&str>) -> Result<TokenSavePlan> {
        self.github_save_plan_for(&TokenIdentity::Instance, host)
    }

    /// The plan a connect flow saves its token through: the test file override
    /// or a real `save_resolving`, plus a handle to the token cache to refresh
    /// after the write. `host` is the token host this connect targets, captured
    /// by value so the device-flow task can own the plan across the spawn.
    ///
    /// `identity` decides both halves of where the token lands - the keyring
    /// account (or the fallback file name) and the cache slot the write
    /// refreshes - so a person's connect can never overwrite the machine's
    /// credential, nor leave the machine's client cached under a name it no
    /// longer belongs to.
    pub(super) fn github_save_plan_for(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<TokenSavePlan> {
        let target = match &self.token_store_dir_override {
            // The same derivation `github_credential_for` reads back through,
            // rather than a second spelling of the file name here.
            Some(dir) => SaveTarget::File(
                TokenStore::file_fallback_for(identity, dir).map_err(EngineError::Remote)?,
            ),
            None => SaveTarget::Resolve {
                fallback_dir: self.origins_base_dir()?,
            },
        };
        Ok(TokenSavePlan {
            identity: identity.clone(),
            host: host.map(str::to_string),
            target,
            cache: Arc::clone(&self.github_tokens),
        })
    }

    /// Clears the whole GitHub token cache when `e` is
    /// [`RemoteError::AuthExpired`] - the mapped GitHub 401, see
    /// `crystalline_remote::github` - so a token rotated or revoked out from
    /// under a long-running daemon is dropped and the next `github_credential`
    /// re-reads from the keychain or file, picking up a standalone CLI connect
    /// that wrote a fresh token while the daemon ran. Coarse on purpose:
    /// clearing every entry (up to one per identity per host now) avoids
    /// threading the offending host through every provider-op call site and
    /// costs only one extra keychain read per host on the next touch.
    fn drop_github_credential_on_auth(&self, e: &RemoteError) {
        if matches!(e, RemoteError::AuthExpired) {
            self.github_tokens.lock().unwrap().clear();
        }
    }
}
