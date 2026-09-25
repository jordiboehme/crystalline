use super::*;

impl Engine {
    // --- recent --------------------------------------------------------------

    /// Recent engrams within a timeframe, from the domains `scope` may read.
    ///
    /// The visibility filter is pushed into the query rather than applied to
    /// what comes back: the row limit is enforced in SQL, so dropping rows
    /// afterwards would quietly shorten a scoped caller's answer instead of
    /// filling it with the next visible engram.
    pub async fn recent_activity(
        &self,
        p: &RecentParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let timeframe = p.timeframe.clone().unwrap_or_else(|| "7d".to_string());
        let hidden = self.hidden_for(scope).await?;
        let domains = match self.scoped_domains(&p.domains, &hidden).await? {
            ScopedDomains::AsAsked => Some(p.domains.clone()).filter(|d| !d.is_empty()),
            ScopedDomains::Only(domains) => Some(domains),
            ScopedDomains::Nothing => {
                return Ok(json!({
                    "timeframe": timeframe,
                    "count": 0,
                    "engrams": Value::Array(Vec::new()),
                }));
            }
        };
        let filter = RecentFilter {
            domains,
            after: timeframe_cutoff(&timeframe),
            engram_types: Some(p.types.clone()).filter(|t| !t.is_empty()),
            limit: 50,
        };
        let mut items = {
            let store = self.store.lock().await;
            store.recent(&filter).await?
        };
        // This reader's own drafts, folded in after the screen above exactly as
        // they are on a browse: the domains `hidden_for` already allowed, and
        // only then whose drafts they are.
        self.fold_recent_drafts(&filter, &hidden, scope, &mut items)
            .await?;
        Ok(json!({
            "timeframe": timeframe,
            "count": items.len(),
            "engrams": serde_json::to_value(&items).unwrap_or(Value::Null),
        }))
    }

    /// Fold every reviewing domain's drafts into a recency listing.
    ///
    /// The cross-domain half: which domains this reader may see, which of them
    /// the filter allows, and the one re-sort and re-cut over the whole folded
    /// list. What each domain's own drafts do to the page is
    /// [`DomainView::recent_into`], asked once per domain.
    async fn fold_recent_drafts(
        &self,
        filter: &RecentFilter,
        hidden: &HashSet<String>,
        scope: &crate::scope::Scope,
        items: &mut Vec<EngramSummary>,
    ) -> Result<()> {
        let reviewed: Vec<DomainView<'_>> = self
            .registered_domain_names()
            .into_iter()
            .filter(|name| !hidden.contains(name))
            .filter(|name| {
                filter
                    .domains
                    .as_ref()
                    .is_none_or(|only| only.iter().any(|d| d == name))
            })
            .filter_map(|name| DomainView::for_read(self, &name, hidden, scope).ok())
            .filter(|view| view.actor().is_some())
            .collect();
        if reviewed.is_empty() {
            return Ok(());
        }
        for view in reviewed {
            view.recent_into(filter, items).await?;
        }
        // The statement's own order, re-applied over the folded list.
        items.sort_by(|a, b| {
            b.recorded_at
                .cmp(&a.recorded_at)
                .then_with(|| a.permalink.cmp(&b.permalink))
        });
        let limit = if filter.limit == 0 { 20 } else { filter.limit };
        items.truncate(limit);
        Ok(())
    }

    // --- list domains --------------------------------------------------------

    /// List registered domains with counts and optional routing bullets. A file
    /// domain reports its path and reads routing bullets from its `MANIFEST.md`
    /// on disk; a virtual domain reports a null path, its kind and reads routing
    /// bullets from its MANIFEST engram in the database.
    ///
    /// With `include_routing` the response also carries a top-level `behavior`
    /// array: the same rules the onboarding block renders, from
    /// [`crystalline_core::behavior_bullets`]. Remote clients never show the
    /// model the initialize instructions, so this one call is their whole
    /// onboarding - the routing lines and the rules that govern them together.
    ///
    /// A domain `scope` may not see is absent from the listing, not marked as
    /// withheld: this is the index a caller routes by, and a name in it is the
    /// whole of what a private domain keeps.
    ///
    /// Each row it does keep carries `private`, so a client that draws a badge
    /// reads it off the listing rather than asking after every domain in it.
    /// That is not the same fact as the one above and it is not a leak of it:
    /// a caller who may not see a domain never gets a row for it to read.
    ///
    /// The rows come back sorted by name, case-insensitively, so the sidebar
    /// and the CLI inherit one order rather than settling it three times.
    /// Registration order is what the config map preserves and it is
    /// meaningless to anybody reading the listing.
    ///
    /// The routing prompt sorts the same way and by the same comparison, in
    /// [`crystalline_core::generate_prompt_unscoped`], rather than through this
    /// call: it builds its block from the config directly. The two are the same
    /// index seen twice - once at session start, once when an agent asks again
    /// mid-session - so they agree.
    pub async fn list_domains(
        &self,
        p: &ListDomainsParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let (private, hidden) = self.visibility_for(scope).await?;
        let store = self.store.lock().await;
        let stats = store.domain_stats().await.unwrap_or_default();
        drop(store);

        let mut out = Vec::new();
        // Every registration this instance has, not the startup snapshot
        // alone: a domain `domain add` wrote into the config file after this
        // daemon started is registered, and a listing that left it out while
        // named reads and search served it is the gap issue #70 reported.
        // Cloned out from behind the locks before any `.await` below.
        let domains = self.registered_domain_entries();
        // Sorted by name rather than left in registration order, which is what
        // the map preserves and what a reader scanning a sidebar has no use
        // for. Case-insensitive first, so capitalization never sorts a domain
        // away from its neighbours, then exact as the tie-break so two names
        // differing only in case have one settled order.
        let mut listed: Vec<_> = domains
            .iter()
            .filter(|(name, _)| !hidden.contains(*name))
            .collect();
        listed.sort_by_cached_key(|(name, _)| (name.to_lowercase(), (*name).clone()));
        for (name, entry) in listed {
            let source = self.source_of(entry);
            let s = stats.iter().find(|d| &d.name == name);
            let mut obj = json!({
                "name": name,
                "kind": if entry.is_virtual() { "virtual" } else { "file" },
                "path": entry.file_path().map(|r| r.display().to_string()),
                "engrams": s.map(|d| d.engrams),
                "observations": s.map(|d| d.observations),
                "relations": s.map(|d| d.relations),
                "last_sync": s.and_then(|d| d.last_sync.clone()),
                // Whether this domain is private, so a client badges the row it
                // already has instead of asking after each one. Every domain a
                // caller may not read was dropped above, so this only ever says
                // "private" about a domain that caller can already see.
                //
                // False on an installation with no accounts database, which is
                // every domain on it: privacy is a membership record, and a
                // machine with no accounts has none.
                "private": private.contains(name),
                // Whether this domain reviews changes before they land, so a
                // client says which way a write in it will go rather than
                // finding out from the receipt. Absent as `null` on a domain
                // that takes changes directly, which is how a domain starts
                // out.
                "review": entry.is_overlay().then_some("overlay"),
            });
            // What THIS caller is holding in a domain that reviews changes, so
            // a screen can say "you have work waiting here" off the listing it
            // already reads. It rides here rather than only on the domain's
            // sync status because that status is gated with the share verbs: a
            // plain member of a reviewing domain could not reach their own
            // count, which is a fact about them rather than about the team.
            // Absent on a domain that takes changes directly, exactly as
            // `review` is and for the same reason; null when the index could
            // not be counted, which is not the same as holding nothing.
            if entry.is_overlay() {
                obj["my_drafts"] = crate::review::DraftView::new(
                    self.overlay_counts_by_actor(name).await,
                    crate::scope::overlay_actor(scope),
                    false,
                )
                .mine();
            }
            // In a shared database a file domain names its current host so an
            // agent and an operator see who syncs what; `hosted_here` is true when
            // this instance holds the lock.
            if let Some(host) = s.and_then(|d| d.host_instance_id.clone()) {
                let hosted_here = !self.instance_id.is_empty() && host == self.instance_id;
                obj["host"] = json!({
                    "instance_id": host,
                    "heartbeat_at": s.and_then(|d| d.host_heartbeat_at.clone()),
                    "hosted_here": hosted_here,
                });
            }
            if p.include_routing {
                let bullets = match &source {
                    ContentSource::File { root } => routing_bullets(root),
                    ContentSource::Virtual => self.virtual_routing_bullets_for(name).await,
                };
                obj["when_to_use"] = json!(bullets);
            }
            out.push(obj);
        }
        if p.include_routing {
            return Ok(json!({
                "behavior": crystalline_core::behavior_bullets(self.read_only()),
                "domains": out,
            }));
        }
        Ok(json!({ "domains": out }))
    }

    /// One domain's MANIFEST markdown, read through the same source its routing
    /// bullets are read through: a file domain's `MANIFEST.md` on disk, a
    /// virtual domain's MANIFEST engram in the database.
    ///
    /// The source, not a reduction of it: a client that renders or edits a
    /// manifest needs the frontmatter and every section, not the routing
    /// bullets [`Engine::list_domains`] already extracts. An unregistered domain
    /// errors with the registered set named, like every other verb; a domain
    /// that carries no MANIFEST yet is a `NotFound`, since a manifest is what
    /// routes an agent to a domain at all rather than an optional extra.
    pub async fn manifest_markdown(&self, domain: &str) -> Result<String> {
        match self.content_source(domain)? {
            ContentSource::File { root } => {
                let path = root.join("MANIFEST.md");
                match std::fs::read_to_string(&path) {
                    Ok(source) => Ok(source),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Err(EngineError::NotFound(format!(
                            "domain '{domain}' has no MANIFEST.md at {}",
                            path.display()
                        )))
                    }
                    Err(source) => Err(EngineError::Io {
                        path: path.display().to_string(),
                        source,
                    }),
                }
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let content = match store.find_engram(domain, "manifest").await? {
                    Some(d) => store.engram_content(d.domain_id, &d.path).await?,
                    None => None,
                };
                content.ok_or_else(|| {
                    EngineError::NotFound(format!("domain '{domain}' has no MANIFEST engram yet"))
                })
            }
        }
    }

    /// Save a domain's MANIFEST markdown verbatim, guarded by the checksum of
    /// the version the caller read - the manifest counterpart of
    /// [`Engine::save_engram`], through the same `expected_checksum` seam and
    /// the same "stale edit" wording on both domain kinds.
    ///
    /// `refresh_routing_cache` runs unconditionally afterwards, on both file
    /// and virtual domains, even though the cache it fills
    /// (`Engine::routing_virtual`) only ever holds virtual-domain bullets: a
    /// file domain's bullets are read straight off `MANIFEST.md` on disk by
    /// `routing_text` at request time, so a file-domain save has nothing in
    /// the cache to refresh. Calling it unconditionally keeps this call site
    /// correct without the caller needing to know which kind answered.
    pub async fn save_manifest(
        &self,
        domain: &str,
        markdown: &str,
        expected_checksum: &str,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // Same hard gate as `save_engram`: a MANIFEST with no frontmatter (or
        // an empty block) is not a manifest at all - it carries the domain's
        // routing bullets and Tag Aliases, so losing the frontmatter here
        // silently strips those too. `parse_engram` alone would not catch
        // this, since an empty frontmatter span parses to
        // `Frontmatter::default()` rather than an error.
        let parsed =
            parse_engram_lossless(markdown).map_err(|e| EngineError::Invalid(e.to_string()))?;
        if !parsed.has_frontmatter || parsed.raw_frontmatter.trim().is_empty() {
            return Err(EngineError::Invalid(
                "the document carries no frontmatter, so it is not a MANIFEST; \
                 keep the --- delimited frontmatter block at the top of the file"
                    .into(),
            ));
        }

        match self.content_source(domain)? {
            ContentSource::File { root } => {
                let path = root.join("MANIFEST.md");
                // The same compare-then-write section `save_engram` holds, for
                // the same reason: two saves of one MANIFEST must not both find
                // their token fresh. See `Engine::write_lock`.
                let lock = self.write_lock(&path);
                let _guard = lock.lock().await;
                let current = match std::fs::read_to_string(&path) {
                    Ok(source) => source,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(format!(
                            "domain '{domain}' has no MANIFEST.md at {}",
                            path.display()
                        )));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: path.display().to_string(),
                            source,
                        });
                    }
                };
                let found = sha256_hex(current.as_bytes());
                if found != expected_checksum {
                    return Err(EngineError::Conflict(stale_edit_message(
                        expected_checksum,
                        &found,
                    )));
                }
                write_file(&path, markdown)?;
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                self.reindex_file(&*store, domain_id, &root, "MANIFEST.md")
                    .await?;
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let desc = store
                    .find_engram(domain, "manifest")
                    .await?
                    .ok_or_else(|| {
                        EngineError::NotFound(format!(
                            "domain '{domain}' has no MANIFEST engram yet"
                        ))
                    })?;
                let stamp = virtual_stamp(markdown);
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    markdown,
                    stamp,
                    Some(expected_checksum),
                    true,
                )
                .await?;
            }
        }

        self.refresh_routing_cache().await;

        Ok(json!({
            "domain": domain,
            "checksum": sha256_hex(markdown.as_bytes()),
        }))
    }

    /// Change one or more MANIFEST policy keys - `generated_indexes`,
    /// `sharing` - through the edit path an engram edit takes: on a file
    /// domain the file changes under the write lock and the index follows; on
    /// a virtual domain the row is rewritten through the store's compare and
    /// swap; in a reviewing domain the write lands in the acting actor's draft
    /// of the MANIFEST and the folder's policy is unchanged until that draft
    /// lands; on a team domain the MANIFEST becomes an unshared local change,
    /// which is how the policy reaches the team.
    ///
    /// Every key is validated before the first is written, so a body with one
    /// bad key writes nothing. A key whose registry row says `Owner` needs
    /// [`DomainRight::Own`] on this domain; `Admin` keys are the caller's gate
    /// (the REST layer's `require_admin`), since the engine cannot ask a scope
    /// whether it administers the instance. No `expected_checksum`: the edit
    /// is one keyed line, and `apply_source_edit`'s compare-and-write
    /// serializes it against a concurrent editor save.
    ///
    /// The MANIFEST is resolved by its own permalink, `manifest`, the way
    /// [`Engine::manifest_markdown`] resolves a virtual domain's: the template
    /// always writes one, and going through the view is what puts the edit in
    /// the acting actor's draft rather than in the folder the team reviewed.
    ///
    /// Answers `{ domain, markdown, draft }`: the MANIFEST as it now reads for
    /// this caller, and whether that is their draft rather than the domain's.
    ///
    /// [`DomainRight::Own`]: crate::scope::DomainRight::Own
    pub async fn set_manifest_policies(
        &self,
        domain: &str,
        changes: &[(String, String)],
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if changes.is_empty() {
            return Err(EngineError::Invalid(
                "no policy named: send an object of at least one MANIFEST policy key to its value"
                    .to_string(),
            ));
        }
        let registry = crystalline_core::policy_registry();
        for (key, value) in changes {
            let Some(spec) = registry.iter().find(|spec| spec.key == key) else {
                let known: Vec<&str> = registry.iter().map(|spec| spec.key).collect();
                return Err(EngineError::Invalid(format!(
                    "`{key}` is not a MANIFEST policy; the policy keys are {}",
                    known.join(", ")
                )));
            };
            if !spec.values.contains(&value.as_str()) {
                return Err(EngineError::Invalid(format!(
                    "`{key}: {value}` is not a value `{key}` takes; write one of {}",
                    spec.values.join(", ")
                )));
            }
            if spec.changed_by == crystalline_core::PolicyRole::Owner {
                self.require_domain_owner_refusing(
                    domain,
                    scope,
                    EngineError::Forbidden(format!(
                        "only the owner of '{domain}' or an instance admin may change `{key}`"
                    )),
                )
                .await?;
            }
        }
        let view = DomainView::for_write(self, domain, scope).await?;
        let overlay = view.actor().map(str::to_string);
        let actor = self.actor_for(None, overlay.as_deref());
        let (desc, source) = view.resolve("manifest").await?;
        let edits: Vec<(String, String)> = changes.to_vec();
        self.apply_source_edit(&desc, &source, &view, None, &actor, None, move |current| {
            let mut out = current.to_string();
            for (key, value) in &edits {
                out = set_frontmatter_field(&out, key, value);
            }
            Ok(out)
        })
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

    /// Routing bullets for one virtual domain, read from its `MANIFEST.md`
    /// engram in the database. Empty when there is no MANIFEST engram yet.
    async fn virtual_routing_bullets_for(&self, name: &str) -> Vec<String> {
        let content = {
            let store = self.store.lock().await;
            match store.find_engram(name, "manifest").await.ok().flatten() {
                Some(d) => store
                    .engram_content(d.domain_id, &d.path)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        };
        let Some(source) = content else {
            return Vec::new();
        };
        let Ok(engram) = parse_engram(&source) else {
            return Vec::new();
        };
        Manifest::from_engram(&engram, &source)
            .routing_bullets()
            .to_vec()
    }

    /// Routing bullets for every virtual domain, keyed by domain name. Supplied
    /// to `crystalline_core::generate_prompt` (which never touches a database),
    /// served over the `routing_bullets` ctl request so `prompt system` stays
    /// inside its latency budget for virtual domains too and snapshotted by
    /// [`Engine::refresh_routing_cache`] for the MCP server instructions.
    pub async fn virtual_routing_bullets(&self) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        // Every registration, not the startup snapshot: a virtual domain the
        // CLI registered after this engine started asks it only to scaffold
        // the MANIFEST, and the refresh that follows caches bullets for the
        // domains found here.
        let domains = self.registered_domain_entries();
        for (name, entry) in &domains {
            if entry.is_virtual() {
                out.insert(name.clone(), self.virtual_routing_bullets_for(name).await);
            }
        }
        out
    }

    // --- generated index files -----------------------------------------------

    /// Regenerate a file domain's OKF `index.md` files, so the knowledge on
    /// disk keeps navigating statically after a mutation or a sync.
    ///
    /// Silently does nothing for a virtual domain (no files to navigate), for a
    /// read-only engine (the curating side owns the index files) and while the
    /// `index.files` setting is off. The pass itself is idempotent: a
    /// regeneration that renders the same bytes writes nothing, so an unchanged
    /// index file keeps its mtime and the watcher stays quiet. Failures are
    /// logged, never propagated: a generated navigation file is a convenience,
    /// and losing it must never fail the write, move, delete or sync that
    /// triggered the pass.
    pub(super) async fn refresh_index_files(&self, domain: &str) {
        if self.read_only || !self.config.read().unwrap().index_files() {
            return;
        }
        let ContentSource::File { root } = self.read_source(domain) else {
            return;
        };
        let name = domain.to_string();
        // A full walk plus one read per engram is blocking IO, so it runs on the
        // blocking pool rather than on a runtime worker.
        let joined = tokio::task::spawn_blocking(move || crate::index_files::refresh(&root)).await;
        match joined {
            Ok(report) if report.written > 0 || report.removed > 0 => {
                tracing::debug!(
                    "refreshed index files of '{name}': {} written, {} removed",
                    report.written,
                    report.removed
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("index refresh of '{name}' did not run: {e}"),
        }
    }

    // --- routing instructions ------------------------------------------------

    /// Recompute the cached virtual-domain routing bullets from the database.
    /// The async companion to [`Engine::routing_text`]: a virtual domain's
    /// bullets live in its MANIFEST engram in the store, so they need an await
    /// to read, but `routing_text` is sync and must not block. The daemon and
    /// the embedded stdio stack call this off the async path (at each MCP
    /// connection's initialize, and after every write that touches a virtual
    /// source) so the sync render only ever reads the cache under the lock.
    pub async fn refresh_routing_cache(&self) {
        let bullets = self.virtual_routing_bullets().await;
        *self.routing_virtual.write().unwrap() = bullets;
    }

    /// The routing instructions a fresh MCP connection is handed at initialize:
    /// the "CRYSTALLINE KNOWLEDGE ROUTING" block over every registered domain.
    /// Synchronous, because rmcp's `get_info` is sync and runs once per
    /// connection; it never blocks on async work, so the virtual bullets come
    /// from the [`Engine::routing_virtual`] cache alone (refreshed off the async
    /// path by [`Engine::refresh_routing_cache`]) and the file bullets are read
    /// straight from each domain's `MANIFEST.md` on disk.
    ///
    /// There is no workspace over MCP: a server serves one index to every
    /// connecting agent, so `prompt.rules` path-glob filters and repo-local
    /// `preferred_domains` never apply here (both need a workspace path). The
    /// effective config is composed live: with a `--config` override this
    /// re-reads that file and re-applies the environment overlay (mirroring
    /// [`Engine::refresh_domain`]) so a domain registered after startup shows up
    /// on the next connection; without one (tests and standalone) it takes the
    /// in-memory config plus any domain discovered since, and never touches the
    /// default global config path. Staleness is bounded to one connection: the
    /// block is an initialize-time snapshot, and the virtual bullets are only as
    /// fresh as the last cache refresh.
    ///
    /// This re-read looks redundant with `self.config` (in-memory), and mostly
    /// is: `configure`'s `Set`/`Unset` (Engine::configure), `domain_add`'s file
    /// and virtual arms and `origin_add` all persist to disk and then write
    /// `self.config` in the same call, under `file_config`-then-`config` lock
    /// order, before returning - a concurrent reader sees the new value the
    /// instant the write lock releases, no re-read needed. The CLI's local
    /// `domain add` (`cmd::domain_add_register` in the CLI crate) is the one
    /// path that does not: it is a free function with no `Engine` reference
    /// at all, so it edits the config file directly regardless of whether a
    /// daemon is live, and the only in-process signal a running daemon gets is
    /// the `sync` ctl call that follows, which resolves the name through
    /// `refresh_domain` into `discovered_domains` and never touches
    /// `self.config`. (`domain remove` used to be this path too; it now goes
    /// through `Engine::domain_remove`, over ctl when a daemon runs.) Serving
    /// from `self.config` alone would therefore leave a freshly added domain
    /// out of every connection's routing block until the daemon restarts, not
    /// just for one racing connection - a real regression, not the
    /// already-accepted bounded staleness this comment describes for the
    /// `None` branch below. So the re-read stays for as long as `domain add`
    /// is a mutation path that does not refresh `self.config`, and
    /// [`Engine::registered_domain_entries`] is the same rule for the listing.
    pub fn routing_text(&self) -> String {
        crystalline_core::render_instructions(&self.routing_output(&HashSet::new()))
    }

    /// [`Engine::routing_text`] with the domain lines replaced by the count
    /// line: every behavior rule, no domain named.
    ///
    /// What the legacy `initialize` handshake serves over HTTP. `get_info` is
    /// synchronous and rmcp calls it with no request context, so that one
    /// channel has no caller to resolve and cannot leave a private domain's
    /// bullets out of a per-caller block; it hands out the countable half
    /// instead and points at `list_domains`, which does resolve a caller. See
    /// [`crystalline_core::render_counted_instructions`] for the residue that
    /// leaves. Stdio never calls this: a local session is the machine owner.
    pub fn routing_text_counted(&self) -> String {
        crystalline_core::render_counted_instructions(&self.routing_output(&HashSet::new()))
    }

    /// The routing block's model over every registered domain except the named
    /// ones. The body of [`Engine::routing_text`],
    /// [`Engine::routing_text_counted`] and [`Engine::routing_text_scoped`], so
    /// a filtered block is the unfiltered one minus some bullets rather than a
    /// second rendering.
    fn routing_output(&self, hidden: &HashSet<String>) -> crystalline_core::PromptOutput {
        // (1) The effective config, composed the same way a fresh load would
        // see it. With a config path this is a fresh file read plus the overlay;
        // a read error falls back to the in-memory effective config.
        let global = match &self.config_path {
            Some(path) => match overlay::load_file(path) {
                Ok(file) => self.overlay.apply(&file),
                Err(_) => self.config(),
            },
            None => {
                // No config path to re-read (tests, standalone): start from the
                // in-memory config and append any domain discovered since
                // startup that it does not already carry, sorted for
                // determinism. Never touch the default global config path.
                let mut global = self.config();
                let discovered = self.discovered_domains.read().unwrap().clone();
                let mut extra: Vec<(String, DomainEntry)> = discovered
                    .into_iter()
                    .filter(|(name, _)| !global.domains.contains_key(name))
                    .collect();
                extra.sort_by(|a, b| a.0.cmp(&b.0));
                for (name, entry) in extra {
                    global.domains.insert(name, entry);
                }
                global
            }
        };

        // (2) Generate over every registered domain from the cached virtual map,
        // (3) force the engine's effective read-only mode, then (4) render.
        // A domain the caller may not see is dropped from both halves: out of
        // the config so it names no routing line, and out of the cached bullets
        // so nothing of its MANIFEST is rendered.
        let mut global = global;
        let virtual_bullets = self.routing_virtual.read().unwrap().clone();
        let virtual_bullets = if hidden.is_empty() {
            virtual_bullets
        } else {
            global.domains.retain(|name, _| !hidden.contains(name));
            virtual_bullets
                .into_iter()
                .filter(|(name, _)| !hidden.contains(name))
                .collect()
        };
        let mut output = crystalline_core::generate_prompt_unscoped(&global, &virtual_bullets);
        output.read_only = self.read_only();
        output
    }

    /// [`Engine::routing_text`] for a caller who may not see every domain: the
    /// same block, with the hidden domains' routing bullets left out.
    ///
    /// Async because resolving a scope reads the accounts database, which is
    /// also why the sync render cannot do this and does not try. The sync one
    /// stays, and stays unfiltered, for the surfaces that have no caller to
    /// resolve: the CLI and the control socket are the machine owner, and they
    /// already have the files on disk.
    ///
    /// The MCP handshake (`get_info`, which rmcp calls without a request
    /// context) is the one channel that is neither - an HTTP peer whose
    /// initialize instructions this server cannot key on anybody - and it is
    /// answered by [`Engine::routing_text_counted`] rather than by this: no
    /// caller to resolve means no bullets at all rather than everybody's. The
    /// era's own instructions channel (`server/discover`) does carry a request
    /// context, and the `onboarding` prompt carries one too, so both are
    /// scoped through here.
    pub async fn routing_text_scoped(&self, scope: &crate::scope::Scope) -> Result<String> {
        let hidden = self.hidden_for(scope).await?;
        Ok(crystalline_core::render_instructions(
            &self.routing_output(&hidden),
        ))
    }

    // --- browse --------------------------------------------------------------

    /// Browse a domain's engrams under a folder path. Works for any registered
    /// domain, file or virtual, since it lists rows from the store rather than
    /// walking a filesystem.
    ///
    /// One level at a time and bounded: at most [`TREE_LEVEL_CAP`] engrams come
    /// back, with `total` saying how many the level holds and `truncated`
    /// saying whether the two differ. `folders` is never cut - it is derived
    /// from the paths themselves rather than from the rows that survived the
    /// cap, so a truncated level still names every folder a reader can descend
    /// into.
    ///
    /// `total` counts the level, not the folder: it moves with `depth` and
    /// leaves out everything nested deeper, so a folder of ten engrams holding a
    /// subfolder of a thousand reports ten here. The paged listing scoped to the
    /// same folder ([`Engine::search_engrams_under`]) counts recursively and
    /// reports the larger number. Neither is the other's approximation: a level
    /// states a fact about the rows it drew, a folder listing promises the
    /// folder, and a client that means to say "N engrams in this folder" takes
    /// the number from the listing.
    ///
    /// A `glob` narrows the rows this level returned, so on a truncated level
    /// it selects within the cap rather than across the whole folder. The tree
    /// is a navigation aid; a folder too big to draw is what the paged listing
    /// is for.
    ///
    /// A domain `scope` may not see is refused exactly as an unregistered one,
    /// down to the registered set the error names.
    pub async fn browse_domain(
        &self,
        p: &BrowseParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        // A domain-exists check, not a filesystem-root requirement, so a virtual
        // domain browses.
        let hidden = self.hidden_for(scope).await?;
        self.domain_entry_scoped(&p.domain, &hidden)?;
        let raw = p.path.clone().unwrap_or_else(|| "/".to_string());
        let prefix = folder_prefix(&raw);
        let depth = p.depth.unwrap_or(1).clamp(1, TREE_MAX_DEPTH);
        let matcher = match &p.glob {
            Some(g) => Some(
                globset::Glob::new(g)
                    .map_err(|e| EngineError::Invalid(format!("invalid glob '{g}': {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };

        // Three bounded queries in the store rather than one listing of the
        // whole domain filtered here: the prefix and the depth cut are pushed
        // into SQL in every case, the root included, so a client that refetches
        // its tree can never pull tens of thousands of rows across per request.
        let store = self.store.lock().await;
        let mut level = store
            .browse_level(&p.domain, prefix.as_deref(), depth, TREE_LEVEL_CAP)
            .await?;
        drop(store);

        // This reader's own drafts shadow the level they are in: a path they
        // have tombstoned leaves it, a path they are drafting is described by
        // their draft, and a draft at a path the domain's files never held
        // joins it. Applied AFTER the domain screen above, which is the order
        // the whole actor dimension composes in.
        {
            let view = DomainView::for_read(self, &p.domain, &hidden, scope)?;
            view.level(prefix.as_deref(), depth, &mut level).await?;
        }

        // Whether the level was cut is a fact about the rows, decided before the
        // glob narrows them: a glob that matches two of five hundred rows has
        // not un-truncated the level, and `total` stays the level's own count so
        // a client can offer the listing instead.
        let truncated = level.total > level.engrams.len();
        let entries: Vec<Value> = level
            .engrams
            .iter()
            .filter(|d| matcher.as_ref().is_none_or(|m| m.is_match(&d.path)))
            .map(|d| {
                // `status` rides along with the rest of the descriptor, which
                // already carries it: a browse row is what a navigation tree is
                // drawn from, and whether an engram is retired is the one thing
                // such a tree has to say about a row it is not otherwise
                // describing. Leaving it out meant every client browsing a
                // domain had to fetch the listing again to learn it.
                json!({
                    "permalink": d.permalink,
                    "title": d.title,
                    "type": d.engram_type,
                    "status": d.status,
                    "path": d.path,
                })
            })
            .collect();

        Ok(json!({
            "domain": p.domain,
            "path": raw,
            "folders": level.folders,
            "engrams": entries,
            "truncated": truncated,
            "total": level.total,
        }))
    }
}
