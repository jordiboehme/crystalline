use super::*;

impl Engine {
    // --- sync / reindex (ctl + CLI) ------------------------------------------

    /// Put every draft the overlay journal mirrors for one domain back into the
    /// index, answering with how many rows were written.
    ///
    /// An overlay entry is primary data that no file on disk describes, so an
    /// index that lost its rows - a `reindex --wipe`, a database restored from
    /// an older copy, a fresh index file - cannot rebuild them by walking the
    /// domain. The journal under the state directory is the copy that can, and
    /// this is where it is read back. It runs in the sync domain pass, so the
    /// first sync after a rebuild is what heals the drafts; on every other sync
    /// it finds every row already there and writes nothing.
    ///
    /// **Store rows win.** An actor already holding a row at a path keeps it,
    /// so a restore only ever fills a gap and a live draft is never overwritten
    /// by an older mirror of itself.
    ///
    /// **Refused for a domain nobody registers**, which is the half that keeps
    /// the two removal paths honest: both of them sweep the journal, but a
    /// mirror that outlived its sweep (an interrupted removal, a folder no
    /// process could delete) must not be able to resurrect drafts for a domain
    /// that no longer exists. Resolved through
    /// [`Engine::registered_domain_names`], the same set collection keys on.
    pub async fn restore_overlays(&self, domain: &str) -> Result<u64> {
        if !self.registered_domain_names().contains(domain) {
            return Err(EngineError::UnknownDomain {
                domain: domain.to_string(),
                registered: self.known_domain_names(),
            });
        }
        let state_dir = self.journal_state_dir()?;
        // **A domain that reviews nothing takes nothing back.** The mirror can
        // outlive the mode - a fold that failed halfway, a config key edited
        // out, an environment variable unset - and this pass runs on every sync
        // of every domain, so without this the leftover rows would be written
        // back into the index for ever rather than merely left behind once. The
        // bytes stay on disk: they are somebody's work, and putting the domain
        // back into review mode is what brings them back. Resolved after the
        // state directory, so an engine that can reach no journal still learns
        // that first.
        if !self.reviews_changes(domain) {
            return Ok(0);
        }
        // A domain with nothing mirrored never reaches the store: the sync pass
        // calls this for every domain on every pass, and resolving a domain id
        // is a write.
        let counts = crate::overlay_journal::journal_counts(&state_dir, domain);
        if counts.total == 0 && !counts.unreadable {
            return Ok(0);
        }
        let entry = self.domain_entry(domain)?;
        let kind = if entry.is_virtual() {
            DomainKind::Virtual
        } else {
            DomainKind::File
        };
        let path = entry.file_path();
        let path_str = path.as_ref().map(|p| p.to_string_lossy());
        let store = self.store.lock().await;
        let id = store
            .upsert_domain(domain, path_str.as_deref(), kind)
            .await?;
        let restored = crate::overlay_journal::restore_into(
            &*store,
            &state_dir,
            domain,
            id,
            &self.chunk_params,
        )
        .await?;
        Ok(restored)
    }

    /// [`Engine::restore_overlays`] as the sync pass runs it: best effort, and
    /// never a reason to fail the sync it rides on. A draft that could not be
    /// restored is a draft the next sync tries again for; a sync that refused
    /// because of one would leave the base rows unindexed too.
    async fn restore_overlays_quietly(&self, domain: &str) {
        match self.restore_overlays(domain).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                domain = domain,
                restored = n,
                "restored {n} mirrored draft(s) into '{domain}' from the overlay journal"
            ),
            Err(e) => tracing::warn!(
                domain = domain,
                error = format!("{e:#}"),
                "the overlay journal for '{domain}' could not be restored; the drafts it \
                 mirrors stay out of the index until the next sync"
            ),
        }
    }

    /// Sync one or all registered domains, returning per-domain reports.
    pub async fn sync(&self, only: Option<&str>) -> Result<Value> {
        self.sync_take_over(only, false).await
    }

    /// Sync like [`Engine::sync`], but with an explicit host-takeover flag for the
    /// `sync --take-over` and `serve --take-over` migration paths. In
    /// collaboration mode (a non-empty instance id) each file domain is claimed
    /// before syncing: an acquired domain syncs, a domain held by another live
    /// instance is skipped on a full sync (`only` is `None`) and refused on a
    /// named one and `take_over` forces the claim. Outside collaboration mode
    /// (standalone, single-instance) nothing is claimed and every target syncs.
    pub async fn sync_take_over(&self, only: Option<&str>, take_over: bool) -> Result<Value> {
        let _activity = ActivityState::begin(&self.activity, "sync", only);
        let targets = self.sync_targets(only)?;
        let collab = !self.instance_id.is_empty();
        // Each domain this run applied, paired with the report its apply
        // produced, for the final cross-domain resolution pass. A domain that
        // was skipped (hosted elsewhere) or failed to scan wrote nothing and is
        // not in the list at all.
        let mut applied: Vec<(DomainId, SyncReport)> = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();
        // Two short store-lock windows per domain with the scan in between, so the
        // walk-and-hash pass of a large domain no longer blocks every concurrent
        // read behind the mutex. The first window claims the host, resolves the
        // domain id and snapshots its stamps; the second applies transactionally
        // with the TOCTOU guards. The claim stays live across the lock-free scan:
        // the heartbeat timer renews it on its own task (30 s cadence, 90 s stale
        // threshold), and unlike the old single-lock sync that scan no longer holds
        // the store lock the timer needs, so a long scan cannot starve the
        // heartbeat into staleness. The apply window is bounded db work, so no
        // extra renew before it is needed.
        for (name, root) in &targets {
            let (domain, snapshot) = {
                let store = self.store.lock().await;
                if collab {
                    match self.claim_file_host(&*store, name, root, take_over).await? {
                        HostClaim::Acquired => {}
                        HostClaim::HeldByOther(host) => {
                            if only.is_some() {
                                return Err(EngineError::Conflict(host_refusal(name, &host)));
                            }
                            tracing::info!(
                                "domain '{name}' is hosted by instance {} (last heartbeat {}); serving it read-from-database only",
                                host.instance_id,
                                host.heartbeat_at
                            );
                            skipped.push(json!({
                                "domain": name,
                                "hosted_by": host.instance_id,
                                "heartbeat_at": host.heartbeat_at,
                            }));
                            continue;
                        }
                    }
                }
                let domain = store
                    .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                let snapshot = store.file_stamps(domain).await?;
                (domain, snapshot)
            };
            let scan = match scan_domain(name, root, snapshot, &self.chunk_params, false).await {
                Ok(scan) => scan,
                Err(e) if only.is_none() => {
                    // One denied domain must not block the rest of the
                    // sweep; its error is carried in the result and the
                    // daemon log, and the next sync retries it.
                    tracing::warn!("sync of '{name}' skipped: {e}");
                    failed.push(json!({ "domain": name, "error": e.to_string() }));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let report = {
                let store = self.store.lock().await;
                apply_scan(&*store, domain, scan)
                    .await
                    .map_err(|e| EngineError::Internal(format!("sync of '{name}' failed: {e}")))?
            };
            // Files changed under us, so the generated index files follow.
            if changed_anything(&report) {
                self.refresh_index_files(name).await;
            }
            // The files are in; the drafts no file describes come back from the
            // journal. A no-op on every sync but the first one after a rebuild.
            self.restore_overlays_quietly(name).await;
            applied.push((domain, report));
        }
        // Every domain of this run is in now, so the references that pointed
        // forward into a domain the loop had not reached yet can resolve. A
        // single-domain run is a no-op inside the pass.
        {
            let store = self.store.lock().await;
            resolve_forward_refs(&*store, &mut applied)
                .await
                .map_err(|e| {
                    EngineError::Internal(format!("resolving forward references failed: {e}"))
                })?;
        }
        let reports: Vec<SyncReport> = applied.into_iter().map(|(_, report)| report).collect();
        Ok(json!({
            "reports": serde_json::to_value(&reports).unwrap_or(Value::Null),
            "skipped": skipped,
            "failed": failed,
        }))
    }

    /// Sync only the given relative paths of one file domain: the targeted path
    /// the daemon's watcher takes for a small debounced batch instead of a full
    /// rescan, so a one-file edit in a large domain costs one stat and one hash,
    /// not a walk of every entry.
    ///
    /// The two-lock-window shape mirrors [`Engine::sync_take_over`]'s per-domain
    /// body - claim the host, snapshot the stamps and release the lock, run the
    /// lock-free path scan, then re-lock to apply through the same [`apply_scan`]
    /// with its TOCTOU guards - so a targeted pass never holds the store mutex
    /// across the scan either. The watcher, the archive import and a discard of
    /// local changes call this; it is intentionally not exposed over MCP or the
    /// control socket, where a full sync is always wanted. A domain hosted by
    /// another live instance in collaboration mode is skipped silently, exactly
    /// as the watcher's full-sync path skips it today, so a non-host never
    /// writes the host's rows. A missed or mis-targeted event is caught by the
    /// full fallback, the startup sync or a manual sync, so the targeted pass
    /// only has to be convergent, never perfect.
    pub async fn sync_paths(&self, name: &str, paths: Vec<String>) -> Result<SyncReport> {
        let ContentSource::File { root } = self.content_source(name)? else {
            // A virtual domain has no files on disk; there is nothing to scan.
            return Ok(SyncReport {
                domain: name.to_string(),
                ..SyncReport::default()
            });
        };
        let collab = !self.instance_id.is_empty();
        let (domain, snapshot) = {
            let store = self.store.lock().await;
            if collab {
                match self.claim_file_host(&*store, name, &root, false).await? {
                    HostClaim::Acquired => {}
                    HostClaim::HeldByOther(host) => {
                        tracing::info!(
                            "targeted sync skipped: domain '{name}' is hosted by instance {}",
                            host.instance_id
                        );
                        return Ok(SyncReport {
                            domain: name.to_string(),
                            ..SyncReport::default()
                        });
                    }
                }
            }
            let domain = store
                .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                .await?;
            let snapshot = store.file_stamps(domain).await?;
            (domain, snapshot)
        };
        let scan = scan_paths(name, &root, snapshot, paths, &self.chunk_params).await;
        let report = {
            let store = self.store.lock().await;
            apply_scan(&*store, domain, scan).await.map_err(|e| {
                EngineError::Internal(format!("targeted sync of '{name}' failed: {e}"))
            })?
        };
        // An out-of-band edit the watcher caught changes what the folder's
        // generated index should say.
        if changed_anything(&report) {
            self.refresh_index_files(name).await;
        }
        Ok(report)
    }

    /// Reindex all file domains. `full` re-reads, re-parses and re-upserts every
    /// file rather than only the ones whose modification time or size moved, and
    /// destroys nothing on the way: each domain serves its previous complete
    /// rows until its own rebuild commits, files gone from disk are pruned as a
    /// sync prunes them, and a chunk whose text is unchanged keeps its
    /// embedding. Virtual-domain rows are never touched at all - they have no
    /// files to rebuild from. In collaboration mode a domain hosted by another
    /// live instance is left untouched, so a non-host never rebuilds the host's
    /// rows out from under it.
    ///
    /// The true wipe is not here: it needs the index file to itself, which the
    /// daemon is holding, so it lives on the daemonless
    /// `crystalline reindex --wipe`.
    ///
    /// The loop is [`crystalline_index::reindex_domains`], shared with the
    /// daemonless `crystalline reindex`: this side supplies only what is the
    /// daemon's own business, the host claim before a domain is touched and the
    /// generated index files after one changed.
    pub async fn reindex(&self, full: bool) -> Result<Value> {
        let _activity = ActivityState::begin(&self.activity, "reindex", None);
        let targets = self.sync_targets(None)?;
        let hooks = DaemonReindexHooks {
            engine: self,
            collab: !self.instance_id.is_empty(),
        };
        let reports = reindex_domains(
            &*self.store,
            &targets,
            &self.chunk_params,
            full.then_some(RebuildKind::Full),
            &hooks,
        )
        .await?;
        Ok(json!({
            "full": full,
            "reports": serde_json::to_value(&reports).unwrap_or(Value::Null),
        }))
    }

    /// The file domains to sync, as `(name, root)` pairs. Virtual domains have
    /// no files, so they are skipped everywhere sync and reindex walk domains; a
    /// named sync of a virtual domain is a clean no-op.
    pub(super) fn sync_targets(&self, only: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
        match only {
            Some(name) => match self.content_source(name)? {
                ContentSource::File { root } => Ok(vec![(name.to_string(), root)]),
                ContentSource::Virtual => Ok(Vec::new()),
            },
            None => {
                let mut targets: Vec<(String, PathBuf)> = Vec::new();
                let config = self.config.read().unwrap();
                for (name, entry) in &config.domains {
                    if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                        targets.push((name.clone(), root));
                    }
                }
                // A domain registered after startup and already resolved once
                // (e.g. by a named `ctl sync`) rides along on a full sync too.
                for (name, entry) in self.discovered_domains.read().unwrap().iter() {
                    if config.domains.contains_key(name) {
                        continue;
                    }
                    if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                        targets.push((name.clone(), root));
                    }
                }
                Ok(targets)
            }
        }
    }

    /// The recorded file stamps of one or every registered file domain, keyed
    /// by domain name and then by domain-relative path: what a caller
    /// compares the files on disk against to tell an indexed file from one
    /// the index has never seen. `crystalline doctor`'s orphan and unindexed
    /// checks read it over ctl, since this daemon holds the index file itself
    /// and a second opener would only collide with it.
    ///
    /// A store read, not a pure one: it upserts each domain row exactly as
    /// [`Engine::sync_take_over`] does, because stamps are keyed by domain id
    /// and a domain nobody has synced yet has no row to read. Nothing about
    /// what this daemon watches, syncs or caches changes here (see
    /// [`Engine::diagnostic_file_domains`]).
    ///
    /// Each entry carries the engram's mtime, size and checksum, more than a
    /// presence check needs, because a stamp is what the index records: a
    /// caller comparing content rather than presence should not need a second
    /// verb for it.
    pub async fn domain_file_stamps(&self, only: Option<&str>) -> Result<Value> {
        let targets = self.diagnostic_file_domains(only)?;
        let mut domains = serde_json::Map::new();
        let store = self.store.lock().await;
        for (name, root) in &targets {
            let domain = store
                .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                .await?;
            let stamps = store.file_stamps(domain).await?;
            domains.insert(
                name.clone(),
                serde_json::to_value(&stamps).unwrap_or(Value::Null),
            );
        }
        drop(store);
        Ok(json!({ "domains": Value::Object(domains) }))
    }

    /// Diagnostics for ctl `status`: per-domain stats, embedding coverage and the
    /// active full-text mode.
    pub async fn status_report(&self) -> Result<Value> {
        let store = self.store.lock().await;
        let info = store.store_info().await?;
        let stats = store.domain_stats().await?;
        let coverage = store.embedding_coverage().await?;
        drop(store);
        let active_embedded = coverage.embedded_for(&self.model_id);
        // Annotate each domain with its ownership relative to this instance so an
        // operator sees at a glance which domains this daemon hosts in a shared
        // database and which it serves read-from-database. `hosted_here` is true
        // only for a file domain whose host lock this instance holds.
        let domains: Vec<Value> = stats
            .iter()
            .map(|s| {
                let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
                if let Value::Object(map) = &mut v {
                    let hosted_here = !self.instance_id.is_empty()
                        && s.host_instance_id.as_deref() == Some(self.instance_id.as_str());
                    map.insert("hosted_here".to_string(), json!(hosted_here));
                }
                v
            })
            .collect();
        let registered: Vec<String> = self
            .config
            .read()
            .unwrap()
            .domains
            .keys()
            .cloned()
            .collect();
        let mut activity = self.activity.lock().unwrap().snapshot_json();
        if let Value::Object(map) = &mut activity {
            map.insert(
                "embedding_backlog".to_string(),
                json!(coverage.backlog_for(&self.model_id)),
            );
        }
        let mut result = json!({
            "fts_mode": info.fts_mode,
            "schema_version": info.schema_version,
            "db_path": info.db_path,
            "db_size": info.db_size,
            "instance_id": if self.instance_id.is_empty() { Value::Null } else { json!(self.instance_id) },
            "registered": registered,
            "domains": serde_json::to_value(&domains).unwrap_or(Value::Null),
            "embeddings": {
                "active_model": self.model_id,
                "provider": self.provider().is_some(),
                "embedded_chunks": active_embedded,
                "total_chunks": coverage.total_chunks,
                "hybrid_available": coverage.has_active_embeddings(&self.model_id),
            },
            "activity": activity,
        });
        let pruned: Vec<Value> = self
            .model_cache_pruned
            .read()
            .unwrap()
            .iter()
            .map(|(repo, bytes)| json!({ "repo": repo, "bytes": bytes }))
            .collect();
        if !pruned.is_empty()
            && let Some(emb) = result.get_mut("embeddings").and_then(Value::as_object_mut)
        {
            emb.insert("pruned_model_cache".to_string(), Value::Array(pruned));
        }
        // Omitted entirely while collaboration is off, so pre-feature output
        // stays byte-stable for an install that never touches GitHub.
        if self.config.read().unwrap().github_enabled()
            && let Value::Object(map) = &mut result
        {
            map.insert("origins".to_string(), self.origins_status_block().await);
        }
        Ok(result)
    }

    /// Chunks awaiting embedding for the active model: the figure `status_report`
    /// exposes as `embedding_backlog`. Reads the cached coverage snapshot, so it
    /// is cheap enough for the daemon's self-heal tick to poll; no per-chunk
    /// scan.
    pub async fn embedding_backlog(&self) -> Result<usize> {
        let coverage = {
            let store = self.store.lock().await;
            store.embedding_coverage().await?
        };
        Ok(coverage.backlog_for(&self.model_id))
    }

    /// Record what the model-cache prune removed, for `ctl status`. Called once
    /// per start, right after the active model has loaded.
    pub fn record_model_cache_prune(&self, removed: Vec<(String, u64)>) {
        *self.model_cache_pruned.write().unwrap() = removed;
    }

    /// The repository the model cache is pruned down to, or `None` when this
    /// instance must not prune weights at all.
    ///
    /// Three conditions, and every one of them has to hold. The instance is
    /// writable: a read-only instance serves a database and a model cache it
    /// does not own, and deleting another install's weights is not its
    /// business. The configured provider is the local one: a remote config may
    /// legitimately name one of the table's models by its repository id,
    /// because that is what the endpoint serving it calls it, and that string
    /// says nothing about which weights this disk needs. And the active model
    /// is one the table knows, so there is a repository to keep; a model this
    /// build does not know keeps everything, since nothing is deleted on a
    /// guess.
    fn model_cache_keep(&self) -> Option<&'static str> {
        if self.read_only {
            return None;
        }
        let local = match self.config.read().unwrap().embeddings.as_ref() {
            Some(e) => e.provider.trim() == "local",
            // No embeddings block is the local provider on the default model.
            None => true,
        };
        if !local {
            return None;
        }
        crystalline_index::local_model(&self.model_id).map(|m| m.repo)
    }

    /// Prune the model cache down to the active model's weights, recording what
    /// went so `ctl status` can report it.
    ///
    /// The daemon calls this once per start and only after the active model has
    /// LOADED, never before: a failed download must not be the reason the only
    /// working weights are deleted. The keep list is a slice because the
    /// contradiction scorer adds its own model id to it; until then it holds
    /// one entry. Every failure is logged and swallowed, because an unpruned
    /// cache costs disk and nothing else.
    pub async fn prune_model_cache(&self, models_dir: PathBuf) {
        let Some(keep) = self.model_cache_keep() else {
            return;
        };
        let removed = tokio::task::spawn_blocking(move || {
            crystalline_index::prune_model_cache(&models_dir, &[keep])
        })
        .await;
        match removed {
            Ok(Ok(removed)) if !removed.is_empty() => {
                let bytes: u64 = removed.iter().map(|(_, b)| b).sum();
                tracing::info!(
                    models = removed.len(),
                    bytes,
                    "pruned unused embedding models from the cache"
                );
                self.record_model_cache_prune(removed);
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => tracing::warn!("could not prune the model cache: {err}"),
            Err(err) => tracing::warn!("the model cache prune task failed: {err}"),
        }
    }

    /// How many prune statements [`Engine::prune_stale_embeddings_if_complete`]
    /// has sent to the store since this engine was built.
    ///
    /// The seam exists because the prune's cost is the statement, not its
    /// result: at full coverage it clears nothing whether it runs or not, so
    /// "it was skipped" is invisible in every observable the engine otherwise
    /// has. Nothing in the daemon, the CLI or the MCP surface reads this.
    #[cfg(any(test, feature = "testing"))]
    pub fn prune_statements_issued(&self) -> u64 {
        self.prune_statements
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many times a team domain's change list has walked a folder since
    /// this engine was built: every call of [`Engine::team_local_changes`],
    /// which reads and hashes every file of the domain it is asked about.
    ///
    /// The seam exists because the cost is invisible in the answer: a diff
    /// built from one walk and a diff built from one walk per changed file are
    /// the same JSON, so only a count tells the two apart. Read as a delta
    /// around the call under test, since building a fixture walks too. Nothing
    /// in the daemon, the CLI or the MCP surface reads this.
    #[cfg(any(test, feature = "testing"))]
    pub fn detection_walks(&self) -> u64 {
        self.detection_walks
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Clear the vectors of every model but the active one, but only once the
    /// active model covers every chunk in the index.
    ///
    /// `None` says the condition was not met and nothing was touched; `Some(n)`
    /// says the prune ran and cleared `n` chunks. The gate is the whole point:
    /// below full coverage the chunks a pass has not reached yet still carry
    /// the previous model's vectors, and those are what search and the
    /// neighbours answer from until it does.
    pub async fn prune_stale_embeddings_if_complete(&self) -> Result<Option<usize>> {
        let store = self.store.lock().await;
        let coverage = store.embedding_coverage().await?;
        if coverage.total_chunks == 0
            || coverage.embedded_for(&self.model_id) < coverage.total_chunks
        {
            return Ok(None);
        }
        // Under the gate above every chunk already carries the active model, so
        // a snapshot showing one embedding group and no chunk this model does
        // not account for has already proved the statement would match nothing.
        // Skip it there: a pass runs on every write and the statement is a scan
        // of the chunk table. What survives this is the index the snapshot
        // cannot vouch for, a second group or an embedded chunk outside the
        // model's count, and that one still runs.
        if coverage.models.len() <= 1
            && coverage.embedded_chunks == coverage.embedded_for(&self.model_id)
        {
            return Ok(Some(0));
        }
        #[cfg(any(test, feature = "testing"))]
        self.prune_statements
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pruned = store.prune_embeddings_except(&self.model_id).await?;
        drop(store);
        if pruned > 0 {
            tracing::info!(
                model = %self.model_id,
                pruned,
                "cleared the vectors of a model this install no longer uses"
            );
        }
        Ok(Some(pruned))
    }

    /// Best-effort WAL checkpoint: reclaims disk after a burst of writes (a
    /// bulk embed pass, daemon shutdown) by merging the WAL back into the main
    /// db file and truncating it. The engine already bounds WAL growth on its
    /// own (a passive checkpoint fires past a hardcoded un-backfilled-frame
    /// threshold, see the PRAGMA probe comment on `TursoStore::build`), so
    /// this call is disk hygiene, never growth control - callers must not
    /// depend on it for correctness. Errors are logged and swallowed: never
    /// let a checkpoint block or fail the caller. A no-op on Postgres via the
    /// `Store::checkpoint_wal` trait default.
    pub async fn checkpoint_wal(&self) {
        let store = self.store.lock().await;
        if let Err(e) = store.checkpoint_wal().await {
            tracing::warn!("WAL checkpoint failed: {e}");
        }
    }

    /// The domain-id set this instance should embed, or `None` for "all domains".
    /// Outside collaboration mode it is `None` (embed everything). In
    /// collaboration mode it is the file domains this instance hosts plus every
    /// virtual domain (whose single source of truth is the shared database, so
    /// every instance is jointly responsible for keeping them embedded). An empty
    /// set is returned as `Some([])`, which the store treats as "nothing to do".
    async fn embed_scope(&self, store: &dyn Store) -> Result<Option<Vec<DomainId>>> {
        if self.instance_id.is_empty() {
            return Ok(None);
        }
        let mut ids: Vec<DomainId> = self.hosted.read().unwrap().values().copied().collect();
        let mut virtuals: Vec<String> = self
            .config
            .read()
            .unwrap()
            .domains
            .iter()
            .filter(|(_, e)| e.is_virtual())
            .map(|(n, _)| n.clone())
            .collect();
        for (name, entry) in self.discovered_domains.read().unwrap().iter() {
            if entry.is_virtual() && !self.config.read().unwrap().domains.contains_key(name) {
                virtuals.push(name.clone());
            }
        }
        for name in virtuals {
            let id = store
                .upsert_domain(&name, None, DomainKind::Virtual)
                .await?;
            ids.push(id);
        }
        ids.sort_by_key(|d| d.0);
        ids.dedup_by_key(|d| d.0);
        Ok(Some(ids))
    }

    /// Embed outstanding chunks for the active model in bounded batches, locking
    /// the store only to pull jobs and to store vectors so long embeds do not
    /// block searches. Returns the number of chunks embedded, which is `0` both
    /// when there was nothing to embed and when another pass was already
    /// walking the backlog; [`Self::embed_pending_outcome`] tells those apart
    /// and is what a caller reporting a count to a person wants.
    pub async fn embed_pending(&self) -> Result<usize> {
        self.embed_pending_with_page(EMBED_PAGE_SIZE).await
    }

    /// [`Self::embed_pending`], saying which of the two things happened rather
    /// than folding a turned-away request into a zero.
    pub async fn embed_pending_outcome(&self) -> Result<EmbedOutcome> {
        self.embed_pass_with_page(EMBED_PAGE_SIZE).await
    }

    /// [`Self::embed_pending`] with an explicit backlog page size. Production
    /// callers take [`EMBED_PAGE_SIZE`] through the wrapper; the parameter lets
    /// a test drive several pages over a small corpus.
    pub async fn embed_pending_with_page(&self, page_size: usize) -> Result<usize> {
        Ok(self.embed_pass_with_page(page_size).await?.embedded())
    }

    /// The pass itself, reporting its outcome.
    ///
    /// A batch the provider rejects is logged and skipped, not fatal: its chunks
    /// keep no embedding and stay in the backlog, visible in `status`, for a
    /// later pass, so one poisoned batch cannot starve the whole queue. Only
    /// store errors abort the pass.
    ///
    /// One pass runs at a time. A caller that arrives while another pass is
    /// walking the backlog is turned away at once instead of walking it a
    /// second time with its own cursor: two passes do not share a backlog, they
    /// shadow each other. Nothing is dropped by that - the running pass is told
    /// to walk again, and a walk starts at the head of the backlog, so it picks
    /// up whatever the second caller had just written.
    async fn embed_pass_with_page(&self, page_size: usize) -> Result<EmbedOutcome> {
        if self.provider().is_none() {
            return Ok(EmbedOutcome::Embedded {
                chunks: 0,
                pruned: 0,
            });
        }
        let Some(mut pass) = EmbedPass::claim(&self.embed_gate) else {
            tracing::debug!("an embed pass is already running; it walks the backlog again");
            return Ok(EmbedOutcome::AlreadyRunning);
        };
        let page_size = page_size.max(1);
        let mut embedded = 0usize;
        let mut activity: Option<ActivityGuard> = None;
        loop {
            embedded += self.embed_one_walk(page_size, &mut activity).await?;
            if !pass.walk_again() {
                break;
            }
        }
        // A completed pass is the moment the previous model's vectors stop
        // being the answer to anything: the active model now covers every
        // chunk, so what is left of another model is dead weight. Never fatal -
        // a failure here costs disk, not correctness.
        let pruned = match self.prune_stale_embeddings_if_complete().await {
            Ok(Some(n)) => n,
            Ok(None) => 0,
            Err(e) => {
                tracing::warn!("could not prune stale embeddings after an embed pass: {e}");
                0
            }
        };
        Ok(EmbedOutcome::Embedded {
            chunks: embedded,
            pruned,
        })
    }

    /// One walk of the backlog, head to tail, for [`Self::embed_pass_with_page`].
    /// The activity is the caller's so a pass that walks twice stays one
    /// operation in `status`.
    async fn embed_one_walk(
        &self,
        page_size: usize,
        activity: &mut Option<ActivityGuard>,
    ) -> Result<usize> {
        let Some(provider) = self.provider() else {
            return Ok(0);
        };
        let model = self.model_id.clone();
        // In collaboration mode the scan is scoped to the file domains this
        // instance hosts plus all virtual domains, so a non-host does not
        // wastefully re-embed a chunk another instance owns; standalone it
        // embeds everything. The scope holds for the whole walk and is read
        // again for the next one, so a domain this instance took over while the
        // pass ran is covered by it.
        let scope = {
            let store = self.store.lock().await;
            self.embed_scope(&*store).await?
        };
        // The backlog is walked one keyset page at a time so a large first index
        // never holds every chunk's text at once. The store lock is held only to
        // pull a page and to write vectors, never across the embed call.
        let mut embedded = 0usize;
        let mut cursor: Option<(i64, i64)> = None;
        loop {
            let mut jobs = {
                let store = self.store.lock().await;
                store
                    .chunks_needing_embedding(&model, scope.as_deref(), page_size, cursor)
                    .await?
            };
            if jobs.is_empty() {
                break;
            }
            // A short page is the last one. The cursor is taken from the store's
            // ordering, before the length sort reorders the page.
            let last_page = jobs.len() < page_size;
            cursor = jobs.last().map(|j| (j.engram_id, j.seq));
            // Length-sort so batches pay for their longest member once instead
            // of padding every short chunk out to whatever long one happened to
            // land in the same batch.
            order_jobs_for_batching(&mut jobs);
            if activity.is_none() {
                *activity = Some(ActivityState::begin(&self.activity, "embed", None));
            }
            for batch in jobs.chunks(EMBED_BATCH) {
                let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();
                // A batch the provider cannot handle is logged and skipped: its
                // chunks keep no embedding, so they stay in the backlog for a
                // later pass instead of starving every batch behind them.
                let vectors = match provider.embed(&texts).await {
                    Ok(v) if v.len() == batch.len() => v,
                    Ok(v) => {
                        tracing::warn!(
                            chunks = ?batch.iter().map(|j| j.chunk_id).collect::<Vec<_>>(),
                            "skipping an embed batch: the provider returned {} vectors for {} inputs",
                            v.len(),
                            batch.len()
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            chunks = ?batch.iter().map(|j| j.chunk_id).collect::<Vec<_>>(),
                            "skipping an embed batch the provider rejected: {e}"
                        );
                        continue;
                    }
                };
                let rows: Vec<crystalline_index::EmbeddingRow> = batch
                    .iter()
                    .zip(vectors)
                    .map(|(job, embedding)| crystalline_index::EmbeddingRow {
                        chunk_id: job.chunk_id,
                        dims: embedding.len(),
                        embedding,
                    })
                    .collect();
                let store = self.store.lock().await;
                store.store_embeddings(&rows, &model).await?;
                embedded += batch.len();
            }
            if last_page {
                break;
            }
        }
        Ok(embedded)
    }

    /// Whether an embedding pass is walking the backlog right now. Cheap: one
    /// flag behind the single-flight gate, no store round trip. A periodic
    /// trigger reads it so it does not chain a fresh full-backlog walk onto the
    /// end of every long pass, since "the backlog is non-empty" stays true for
    /// the whole life of one.
    pub fn embed_in_flight(&self) -> bool {
        self.embed_gate.lock().unwrap().running
    }

    /// Schedules a background embedding pass when a worker is wired,
    /// returning whether it was scheduled; callers run an inline pass when
    /// it was not. "Scheduled" is all it reports: the signal is queued, and
    /// whether the pass then runs, coalesces into a running one or finds the
    /// backlog already drained is the worker's business, never the caller's.
    pub fn request_embed(&self) -> bool {
        match &self.embed_tx {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// Ask the embed worker to pick up what a write just chunked, without
    /// waiting for it. A write never embeds inline: the point of the worker is
    /// that a write returns at the speed of the disk, and a virtual domain -
    /// never watched - would otherwise sit unembedded until the self-heal
    /// tick. With no worker wired this is a no-op, as [`Engine::request_embed`]
    /// already is.
    pub(crate) fn nudge_embed(&self) {
        let _ = self.request_embed();
    }
}
