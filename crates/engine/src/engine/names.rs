//! The name table cache and the spelling push.
//!
//! One [`NameTable`] over every registered domain answers which local name a
//! spelling means: the local name itself, the canonical name the domain's
//! MANIFEST declares, or a machine-local alias. The engine caches it and
//! rebuilds it when a registration or a declared name may have changed, and
//! pushes its spellings into the index's `domain_spelling` table, which the
//! resolve pass reads to bind a reference spelled with any of them.

use std::sync::atomic::Ordering as AtomicOrdering;

use crystalline_core::manifest::domain_name_of_source;
use crystalline_core::names::{NameTable, config_name_inputs};

use super::*;
use crate::params::DomainArgs;

/// The path of a domain's MANIFEST, relative to its root.
const MANIFEST_PATH: &str = "MANIFEST.md";

impl Engine {
    /// The current name table, rebuilt when stale. Never touches the store
    /// once the virtual names are loaded.
    ///
    /// A stale rebuild runs on the blocking pool, and the first call on an
    /// engine that never read its virtual domains' MANIFEST engrams reads them
    /// first, so a one-shot command or a REST request before any MCP
    /// connection knows the names they declare.
    ///
    /// The stale mark is cleared when the rebuild starts, not when it ends:
    /// a lookup that arrives while the build is on the pool is answered from
    /// the table that stands. So "marked stale" means "a later lookup builds",
    /// not "the very next lookup sees the change"; a caller that needs the
    /// change seen at once calls [`Engine::refresh_names`] and waits for it.
    pub async fn name_table_now(&self) -> Arc<NameTable> {
        if !self.virtual_names_loaded.load(AtomicOrdering::SeqCst) {
            self.reload_virtual_manifests().await;
            self.names_stale.store(true, AtomicOrdering::SeqCst);
        }
        if self.names_stale.swap(false, AtomicOrdering::SeqCst) {
            return self.rebuild_names().await;
        }
        self.names.read().unwrap().1.clone()
    }

    /// Rebuild the table now, push every spelling into the index and
    /// re-resolve the references whose spelling changed. Best effort on
    /// the index half: a failure is logged, never returned to a verb.
    ///
    /// Reads each virtual domain's MANIFEST engram first, in the same pass
    /// that caches its routing bullets, because a virtual domain declares its
    /// name in the database where the table build cannot reach.
    pub async fn refresh_names(&self) {
        // A rename between its index row and its config step: the push
        // would drop the alias the index row step left for the old name,
        // which the configuration does not list yet. The rename refreshes
        // once its config step is done.
        if self.names_frozen() {
            self.mark_names_stale();
            return;
        }
        self.reload_virtual_manifests().await;
        // Rebuilt here rather than marked and left to `name_table_now`: a
        // lookup racing this call may have taken the stale mark and be
        // building from what it read before, and the push must carry what is
        // true now.
        self.names_stale.store(false, AtomicOrdering::SeqCst);
        let table = self.rebuild_names().await;
        let pairs = table.spellings();
        let pushed = {
            let store = self.store.lock().await;
            self.push_spellings(&*store, &pairs).await
        };
        match pushed {
            // The push reset and rebound references in every domain and names
            // none, so every registered domain is announced whole, the rule
            // `resolve_references_into` uses: a spelling change is rare, and a
            // `domain` event only refetches what a page shows.
            Ok(true) => {
                let mut names: Vec<String> = self.registered_domain_names().into_iter().collect();
                names.sort();
                for name in names {
                    self.announce_domain(&name, None, None);
                }
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("recording the domain name spellings in the index failed: {e}");
            }
        }
    }

    /// The local name `spelling` resolves to, if any, for a caller that may
    /// see every domain.
    pub async fn local_domain_name(&self, spelling: &str) -> Option<String> {
        self.table_knowing([spelling.to_string()])
            .await
            .resolve(spelling)
            .map(str::to_string)
    }

    /// Rewrite every domain spelling `source` carries by something other than
    /// its canonical name to that canonical name, wherever the registrations
    /// as they stand resolve the canonical back to the same domain (see
    /// [`NameTable::normalize`]) AND the spelling does not resolve to a domain
    /// in `hidden`. Answers the text and how many spellings were rewritten,
    /// `0` when none were.
    ///
    /// The single point every write funnels its final text through before it
    /// is stored: a link written `[[eng-knowledge:x]]` or `[[engineering:x]]`
    /// when `eng-knowledge` declares the canonical name `eng` (and lists
    /// `engineering` as an alias) is stored as `[[eng:x]]`. A spelling that is
    /// shadowed or contested is left exactly as written, since
    /// [`NameTable::normalize`] answers `None` for it: content never gets a
    /// spelling that points elsewhere. A spelling that resolves to a domain
    /// this writer may not see is left exactly as written too, for the same
    /// reason [`localize_in`] leaves one alone: rewriting `[[old-secret:x]]`
    /// into `[[secret:x]]` would tell a caller who cannot see `secret` both
    /// that it exists and what its canonical name is, the exact oracle
    /// `hidden` exists everywhere else to close.
    pub(super) async fn normalize_domain_spellings(
        &self,
        source: &str,
        hidden: &HashSet<String>,
    ) -> (String, usize) {
        let table = self.name_table_now().await;
        crystalline_core::relink::respell_domains(source, &|domain: &str| {
            let local = table.resolve(domain)?;
            if hidden.contains(local) {
                return None;
            }
            table.normalize(domain).map(str::to_string)
        })
    }

    /// [`Engine::normalize_domain_spellings`], with the hidden set worked out
    /// only when there turns out to be something to decide about.
    ///
    /// A first pass with an empty hidden set answers whether `source` holds
    /// any spelling that normalization would ever touch, for any hidden set:
    /// hiding a domain can only turn a would-be rewrite into a kept spelling,
    /// never manufacture a new one, so a probe that rewrites nothing answers
    /// the real question too and is returned as-is. Only when it rewrites
    /// something does this call [`Engine::hidden_for`] and redo the pass for
    /// real, over the ORIGINAL `source` rather than the probe's (possibly
    /// leaky) output. The common case - an ordinary write with no
    /// cross-domain spelling to respell - costs nothing beyond that first
    /// pass; `hidden_for` runs only when there is something to actually
    /// gate. Exposed to the co-editing session (`pub(crate)`), which
    /// normalizes its own text under [`crate::scope::Scope::Unrestricted`]
    /// before ever handing it to a save.
    pub(crate) async fn normalize_domain_spellings_for(
        &self,
        source: &str,
        scope: &crate::scope::Scope,
    ) -> Result<(String, usize)> {
        let probe = self
            .normalize_domain_spellings(source, &HashSet::new())
            .await;
        if probe.1 == 0 {
            return Ok(probe);
        }
        let hidden = self.hidden_for(scope).await?;
        Ok(self.normalize_domain_spellings(source, &hidden).await)
    }

    /// `localize` for one caller: a spelling that resolves to a domain in
    /// `hidden` stays exactly as typed, so the ordinary unknown-domain path
    /// answers it with the caller's own words and never names the local name.
    pub async fn localize_visible(&self, spelling: &str, hidden: &HashSet<String>) -> String {
        // A spelling of a domain being renamed waits for the rename, so it
        // maps to the name the domain has once it is done.
        self.read_past_renames(&[spelling.to_string()]).await;
        let table = self.table_knowing([spelling.to_string()]).await;
        localize_in(&table, spelling, hidden)
    }

    /// Wait for a rename of a domain a read names, and past the wait answer
    /// with the names as they stand, saying why the read was slow: a rename
    /// that stopped keeps its domain paused until the next start.
    async fn read_past_renames(&self, spellings: &[String]) {
        if let Err(e) = self.wait_for_renames(spellings).await {
            tracing::warn!(
                "a read waited 30 s for a domain rename that has not finished ({e}); if that \
                 rename stopped, its domain stays paused until the next start, and the read is \
                 answered with the names as they stand"
            );
        }
    }

    /// `p` with every domain it names mapped to a local name, through
    /// [`Engine::localize_visible`]. Mapping a local name answers it
    /// unchanged, so a value localized twice is localized once.
    pub async fn localized<P: DomainArgs + Clone>(&self, p: &P, hidden: &HashSet<String>) -> P {
        // A read of a domain being renamed waits for the rename, and past
        // the wait is answered by the names as they stand.
        self.read_past_renames(&spellings_of(p)).await;
        let table = self.table_knowing(spellings_of(p)).await;
        let mut p = p.clone();
        p.localize_domains(&|spelling| localize_in(&table, spelling, hidden));
        p
    }

    /// [`Engine::localized`] for a verb that has not asked what `scope` may
    /// see: the hidden set is only worked out when some spelling in `p` names
    /// a domain by another name than its local one, so a call that names
    /// every domain by its local name pays nothing for it.
    pub async fn localized_for<P: DomainArgs + Clone>(
        &self,
        p: &P,
        scope: &crate::scope::Scope,
    ) -> Result<P> {
        // A write into a domain being renamed waits for the rename, then
        // maps to the name the domain has now.
        self.wait_for_renames(&spellings_of(p)).await?;
        let table = self.table_knowing(spellings_of(p)).await;
        let respelled = std::cell::Cell::new(false);
        let mut probe = p.clone();
        probe.localize_domains(&|spelling| {
            let local = localize_in(&table, spelling, &HashSet::new());
            if local != spelling {
                respelled.set(true);
            }
            local
        });
        if !respelled.get() {
            return Ok(probe);
        }
        let hidden = self.hidden_for(scope).await?;
        let mut p = p.clone();
        p.localize_domains(&|spelling| localize_in(&table, spelling, &hidden));
        Ok(p)
    }

    /// The table to map `spellings` through, sure to know every one of them
    /// that is itself a registered local name.
    ///
    /// Local names always win, but the cached table only knows the
    /// registrations it was built from, and a registration written by another
    /// process (the CLI's `domain add` with a daemon running) marks nothing
    /// stale here. Mapped through such a table, the new domain's own name
    /// would land on the older domain whose canonical name or alias it
    /// shadows. So a spelling the table would map to a DIFFERENT local name
    /// is first looked up in the registrations as they stand, and a hit
    /// rebuilds the table before anything is mapped. A spelling the table
    /// maps to itself, or to nothing, costs no read at all.
    async fn table_knowing(&self, spellings: impl IntoIterator<Item = String>) -> Arc<NameTable> {
        let table = self.name_table_now().await;
        let respelled: Vec<String> = spellings
            .into_iter()
            .filter(|s| table.resolve(s).is_some_and(|local| local != s))
            .collect();
        if respelled.is_empty() {
            return table;
        }
        let entries = self.registered_domain_entries_now().await;
        if !respelled.iter().any(|s| entries.contains_key(s)) {
            return table;
        }
        self.mark_names_stale();
        self.name_table_now().await
    }

    /// How many times the spelling push replaced the index's spellings since
    /// this engine was built.
    ///
    /// The seam exists because the push compares before it writes, and a
    /// skipped replace is invisible in every answer the engine gives: the
    /// spellings are the same either way. Nothing in the daemon, the CLI or
    /// the MCP surface reads this.
    #[cfg(any(test, feature = "testing"))]
    pub fn spelling_replaces_issued(&self) -> u64 {
        self.spelling_replaces.load(AtomicOrdering::Relaxed)
    }

    /// Forget the cached table, so the next lookup rebuilds it. For a change
    /// that needs no index write of its own: a domain found by a named lookup
    /// is synced (and pushed) by the verb that found it.
    pub(super) fn mark_names_stale(&self) {
        self.names_stale.store(true, AtomicOrdering::SeqCst);
    }

    /// What a content write owes the caches: a virtual source may have
    /// rewritten its MANIFEST engram, the source of its routing bullets and
    /// its declared name, and a file domain's root `MANIFEST.md` is where the
    /// file domain declares its name.
    pub(super) async fn after_source_write(&self, source: &ContentSource, path: &str) {
        match source {
            ContentSource::Virtual => self.refresh_routing_cache().await,
            ContentSource::File { .. } if path == MANIFEST_PATH => self.refresh_names().await,
            ContentSource::File { .. } => {}
        }
    }

    /// Whether a batch of changed relative paths holds a domain's MANIFEST.
    pub(super) fn touches_manifest(paths: &[String]) -> bool {
        paths.iter().any(|p| p == MANIFEST_PATH)
    }

    /// Build the table from every registration, store it and answer it. The
    /// build reads the configuration file and each file domain's MANIFEST, so
    /// it runs on the blocking pool.
    async fn rebuild_names(&self) -> Arc<NameTable> {
        let generation = self.names_ticket();
        let virtual_names = self.virtual_domain_names.read().unwrap().clone();
        let read = self.registration_reader();
        let built = tokio::task::spawn_blocking(move || build_table(&read(), &virtual_names)).await;
        match built {
            Ok(table) => self.install_names(generation, table),
            Err(e) => {
                // The build panicked: keep the table that stands and let the
                // next lookup try again.
                tracing::warn!("building the domain name table failed: {e}");
                self.names_stale.store(true, AtomicOrdering::SeqCst);
                self.names.read().unwrap().1.clone()
            }
        }
    }

    /// [`Engine::registered_domain_entries`] as a closure over owned copies
    /// of its inputs, for the blocking pool: the call re-reads the
    /// configuration file.
    fn registration_reader(
        &self,
    ) -> impl FnOnce() -> IndexMap<String, DomainEntry> + Send + 'static {
        let snapshot = self.config.read().unwrap().domains.clone();
        let discovered = self.discovered_domains.read().unwrap().clone();
        let path = self.config_file_path();
        let overlay = self.overlay.clone();
        move || union_registrations(snapshot, &discovered, reread_config_at(path, &overlay))
    }

    /// [`Engine::registered_domain_entries`], with the file read on the
    /// blocking pool.
    pub(super) async fn registered_domain_entries_now(&self) -> IndexMap<String, DomainEntry> {
        let read = self.registration_reader();
        match tokio::task::spawn_blocking(read).await {
            Ok(entries) => entries,
            // Only a panic in the read lands here; answer what memory holds.
            Err(_) => {
                let snapshot = self.config.read().unwrap().domains.clone();
                let discovered = self.discovered_domains.read().unwrap().clone();
                union_registrations(snapshot, &discovered, None)
            }
        }
    }

    /// A generation for a build about to read its inputs. Taken before the
    /// reads, so of two racing builds the one that read last numbers higher.
    fn names_ticket(&self) -> u64 {
        self.names_generation.fetch_add(1, AtomicOrdering::SeqCst) + 1
    }

    /// Store `table`, built under `generation`, unless a build that took a
    /// later ticket already stored its own, and answer the table that stands.
    /// Warns about a dropped alias or a contested canonical name when it is
    /// new since the previous table, so a refresh on every connection does
    /// not repeat the same warning.
    fn install_names(&self, generation: u64, table: NameTable) -> Arc<NameTable> {
        let mut slot = self.names.write().unwrap();
        if slot.0 > generation {
            return slot.1.clone();
        }
        let table = Arc::new(table);
        let (_, previous) = std::mem::replace(&mut *slot, (generation, table.clone()));
        drop(slot);
        warn_new_name_problems(&previous, &table);
        table
    }

    /// A build ticket and the store a build ends in, for the test that a
    /// slower build never replaces a newer one: two racing builds are two
    /// tickets taken in one order and installed in the other.
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn names_ticket_for_test(&self) -> u64 {
        self.names_ticket()
    }

    /// See [`Engine::names_ticket_for_test`].
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    pub fn install_names_for_test(
        &self,
        generation: u64,
        inputs: &[crystalline_core::names::NameInput],
    ) -> Arc<NameTable> {
        self.install_names(generation, NameTable::build(inputs))
    }

    /// Read every virtual domain's MANIFEST engram once and cache both what
    /// it routes by and the name it declares.
    async fn reload_virtual_manifests(&self) {
        let mut bullets = BTreeMap::new();
        let mut names = BTreeMap::new();
        for (name, entry) in &self.registered_domain_entries_now().await {
            if !entry.is_virtual() {
                continue;
            }
            let (routing, declared) = self.virtual_manifest_facts(name).await;
            bullets.insert(name.clone(), routing);
            if let Some(declared) = declared {
                names.insert(name.clone(), declared);
            }
        }
        *self.routing_virtual.write().unwrap() = bullets;
        *self.virtual_domain_names.write().unwrap() = names;
        self.virtual_names_loaded
            .store(true, AtomicOrdering::SeqCst);
    }

    /// One virtual domain's routing bullets and declared valid `domain_name`,
    /// from one read of its MANIFEST engram. Empty and `None` when it has no
    /// MANIFEST engram yet.
    pub(super) async fn virtual_manifest_facts(&self, name: &str) -> (Vec<String>, Option<String>) {
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
            return (Vec::new(), None);
        };
        let bullets = match parse_engram(&source) {
            Ok(engram) => Manifest::from_engram(&engram, &source)
                .routing_bullets()
                .to_vec(),
            Err(_) => Vec::new(),
        };
        (bullets, domain_name_of_source(&source))
    }

    /// Record `pairs` (spelling, local name) as the spellings of the domains
    /// they name, and re-resolve what a changed spelling now means.
    ///
    /// The scope is every registered domain that has a row, which covers
    /// every domain this instance hosts too: a hosted domain is a registered
    /// one. A domain with no row yet is skipped; its first sync pushes again.
    ///
    /// The replace runs only when the table differs from what the index
    /// holds for those domains, so an unchanged table costs one read. A
    /// spelling the index maps to a domain outside the scope is not counted
    /// as a difference: on a shared index it is another instance's claim,
    /// and taking it back on every refresh would flip it between the two for
    /// ever; on any index it may be another row's own name, which the replace
    /// re-claims for that row last, so the pair could never land. When the
    /// replace does run it gets the whole list, and the index's own rules
    /// decide. Answers whether it changed anything.
    async fn push_spellings(
        &self,
        store: &dyn Store,
        pairs: &[(String, String)],
    ) -> crystalline_index::Result<bool> {
        let mut ids: HashMap<&str, DomainId> = HashMap::new();
        for (_, local) in pairs {
            if ids.contains_key(local.as_str()) {
                continue;
            }
            if let Some(id) = store.domain_id(local).await? {
                ids.insert(local, id);
            }
        }
        let list: Vec<(String, DomainId)> = pairs
            .iter()
            .filter_map(|(spelling, local)| {
                ids.get(local.as_str()).map(|id| (spelling.clone(), *id))
            })
            .collect();
        if list.is_empty() {
            return Ok(false);
        }
        let scope: HashSet<DomainId> = ids.values().copied().collect();
        let current = store.domain_spellings().await?;
        let holder: HashMap<&str, DomainId> =
            current.iter().map(|(s, id)| (s.as_str(), *id)).collect();
        let wanted: HashSet<(&str, DomainId)> = list
            .iter()
            .filter(|(spelling, _)| {
                holder
                    .get(spelling.as_str())
                    .is_none_or(|held| scope.contains(held))
            })
            .map(|(spelling, id)| (spelling.as_str(), *id))
            .collect();
        let held: HashSet<(&str, DomainId)> = current
            .iter()
            .filter(|(_, id)| scope.contains(id))
            .map(|(spelling, id)| (spelling.as_str(), *id))
            .collect();
        if wanted == held {
            return Ok(false);
        }

        #[cfg(any(test, feature = "testing"))]
        self.spelling_replaces.fetch_add(1, AtomicOrdering::Relaxed);
        let changed = store.replace_domain_spellings(&list).await?;
        if changed.is_empty() {
            return Ok(false);
        }
        tracing::debug!(spellings = ?changed, "domain name spellings changed meaning");
        store.reset_references_to_spellings(&changed).await?;

        // The reset unbound references in every domain that spelled one of
        // the changed names, whoever registers it, so every row gets a pass.
        resolve_pending_in_every_domain(store).await?;
        Ok(true)
    }

    /// Bind every pending reference in every domain the index knows, for a
    /// change that may have unbound references without a spelling push to
    /// notice it: a rename that took a spelling another domain held. Each
    /// actor's drafts are bound in that actor's own view first, then the rest
    /// against the base. One transaction and one store-lock window per domain,
    /// so a request waits for one domain's pass at most, never for all of
    /// them. Best effort, like the push.
    pub(super) async fn resolve_pending_everywhere(&self) {
        self.resolve_pending_domain_by_domain(&HashSet::new()).await;
    }

    /// The pass a daemon or an embedded server runs once its first sync has
    /// returned. An index upgrade may have unbound references in a virtual
    /// domain or in a draft, and no sync from disk reaches either. A file
    /// domain's base rows are left alone: the sync that just ran bound them.
    pub(super) async fn resolve_pending_after_startup(&self) {
        let files: Vec<String> = self
            .config()
            .domains
            .iter()
            .filter(|(_, entry)| !entry.is_virtual())
            .map(|(name, _)| name.clone())
            .collect();
        let mut synced = HashSet::new();
        {
            let store = self.store.lock().await;
            for name in &files {
                match store.domain_id(name).await {
                    Ok(Some(id)) => {
                        synced.insert(id);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            "binding the references left pending at startup failed: {e}"
                        );
                        return;
                    }
                }
            }
        }
        self.resolve_pending_domain_by_domain(&synced).await;
    }

    /// Bind the pending references, in every domain, that a newly registered
    /// `domain` answers: the rows spelled with one of its names. A reference
    /// elsewhere that already named it waited pending, and its own domain's
    /// next sync is not coming on its own. Only those rows are read.
    pub(super) async fn resolve_references_into(&self, domain: &str) {
        let store = self.store.lock().await;
        let bound = async {
            let Some(id) = store.domain_id(domain).await? else {
                return Ok(0);
            };
            let spellings: Vec<String> = store
                .domain_spellings()
                .await?
                .into_iter()
                .filter(|(_, holder)| *holder == id)
                .map(|(spelling, _)| spelling)
                .collect();
            store.resolve_references_to_spellings(&spellings).await
        }
        .await;
        drop(store);
        match bound {
            // The pass answers one count across every domain and names none,
            // so every other registered domain is announced whole: a
            // registration is rare and a `domain` event only refetches what a
            // page is showing. The new domain's own rows rode its sync.
            Ok(count) if count > 0 => {
                let mut others: Vec<String> = self
                    .registered_domain_names()
                    .into_iter()
                    .filter(|name| name != domain)
                    .collect();
                others.sort();
                for name in others {
                    self.announce_domain(&name, None, None);
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("binding the references that name '{domain}' failed: {e}");
            }
        }
    }

    /// One resolve pass per domain row the index's spellings reach, each in
    /// its own transaction under its own store-lock window. `base_done` names
    /// the domains whose base rows need no pass; their drafts still get one.
    async fn resolve_pending_domain_by_domain(&self, base_done: &HashSet<DomainId>) {
        let every = {
            let store = self.store.lock().await;
            every_domain_id(&*store).await
        };
        let every = match every {
            Ok(every) => every,
            Err(e) => {
                tracing::warn!("binding the pending references failed: {e}");
                return;
            }
        };
        // The local name of each row, for the announcement: the store has no
        // id-to-name lookup, so the registered names are asked once. A row no
        // registered name maps to (a leftover) is bound and never announced.
        let names: HashMap<DomainId, String> = {
            let registered = self.registered_domain_names();
            let store = self.store.lock().await;
            let mut names = HashMap::new();
            for name in registered {
                if let Ok(Some(id)) = store.domain_id(&name).await {
                    names.insert(id, name);
                }
            }
            names
        };
        for id in every {
            let store = self.store.lock().await;
            let base = !base_done.contains(&id);
            let bound = in_one_transaction(&*store, bind_pending(&*store, id, base)).await;
            drop(store);
            match bound {
                // Nothing's text changed, but a pending link or relation is
                // bound now, which the reading page, the backlinks and the
                // graph show; the pass names no engram, so the domain is
                // announced whole.
                Ok(count) if count > 0 => {
                    if let Some(name) = names.get(&id) {
                        self.announce_domain(name, None, None);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        "binding the pending references of domain {} failed: {e}",
                        id.0
                    );
                }
            }
        }
    }
}

/// Every domain row the index's spellings reach, in id order.
async fn every_domain_id(store: &dyn Store) -> crystalline_index::Result<Vec<DomainId>> {
    let mut every: Vec<DomainId> = store
        .domain_spellings()
        .await?
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    every.sort_by_key(|id| id.0);
    every.dedup();
    Ok(every)
}

/// Bind one domain's pending rows: each drafting actor's in that actor's own
/// view, as a draft write binds them, and then, when `base`, every row still
/// pending against the base. No transaction of its own.
async fn bind_pending(
    store: &dyn Store,
    id: DomainId,
    base: bool,
) -> crystalline_index::Result<u64> {
    let mut bound = 0;
    for (actor, _) in store.overlay_counts(id).await? {
        bound += store.reresolve_actor_references(id, &actor).await?;
    }
    if base {
        bound += store.resolve_pending_relations(id).await?;
        bound += store.resolve_pending_links(id).await?;
    }
    Ok(bound)
}

/// `pass` inside one transaction, rolled back when it fails; answers what
/// the pass answered once the transaction committed.
async fn in_one_transaction<T>(
    store: &dyn Store,
    pass: impl std::future::Future<Output = crystalline_index::Result<T>>,
) -> crystalline_index::Result<T> {
    store.begin().await?;
    match pass.await {
        Ok(value) => {
            store.commit().await?;
            Ok(value)
        }
        Err(e) => {
            let _ = store.rollback().await;
            Err(e)
        }
    }
}

/// One resolve pass over every domain row the index's spellings reach, in
/// one transaction, for the spelling push, which already holds the store.
async fn resolve_pending_in_every_domain(store: &dyn Store) -> crystalline_index::Result<()> {
    let every = every_domain_id(store).await?;
    in_one_transaction(store, async {
        for id in &every {
            bind_pending(store, *id, true).await?;
        }
        Ok::<(), crystalline_index::IndexError>(())
    })
    .await
}

/// The table over `entries`: each file domain's canonical name read from its
/// MANIFEST on disk, each virtual domain's from `virtual_names`.
fn build_table(
    entries: &IndexMap<String, DomainEntry>,
    virtual_names: &BTreeMap<String, String>,
) -> NameTable {
    NameTable::build(&config_name_inputs(entries, virtual_names))
}

/// Every domain spelling `p` carries, as written.
fn spellings_of<P: DomainArgs + Clone>(p: &P) -> Vec<String> {
    let seen = std::cell::RefCell::new(Vec::new());
    p.clone().localize_domains(&|spelling| {
        seen.borrow_mut().push(spelling.to_string());
        spelling.to_string()
    });
    seen.into_inner()
}

/// The local name `spelling` means in `table`, unless that domain is in
/// `hidden`: then, as for a spelling nothing answers to, the spelling itself.
pub(super) fn localize_in(table: &NameTable, spelling: &str, hidden: &HashSet<String>) -> String {
    match table.resolve(spelling) {
        Some(local) if !hidden.contains(local) => local.to_string(),
        _ => spelling.to_string(),
    }
}

/// Whether two spellings of a reference's domain name one domain in `table`:
/// equal as written, or resolving to the same local name. A reference with
/// no domain matches only another with none.
pub(super) fn same_domain(table: &NameTable, a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) if a != b => table
            .resolve(a)
            .is_some_and(|local| table.resolve(b) == Some(local)),
        (a, b) => a == b,
    }
}

/// Warn about each dropped alias and each contested canonical name `table`
/// has and `previous` did not, each with the change that fixes it.
fn warn_new_name_problems(previous: &NameTable, table: &NameTable) {
    for dropped in table.dropped_aliases() {
        if previous.dropped_aliases().contains(dropped) {
            continue;
        }
        let why = match &dropped.held_by {
            Some(owner) => format!("domain '{owner}' already answers to that name"),
            None => "more than one domain claims that name".to_string(),
        };
        tracing::warn!(
            "alias '{alias}' of domain '{domain}' is ignored: {why}; remove '{alias}' from the \
             aliases of '{domain}' in the configuration file to silence this",
            alias = dropped.alias,
            domain = dropped.domain,
        );
    }
    for conflict in table.conflicts() {
        if previous.conflicts().contains(conflict) {
            continue;
        }
        tracing::warn!(
            "domain name '{}' resolves nowhere: domains {} all declare it in their MANIFEST; \
             change domain_name in the MANIFEST of all but one of them",
            conflict.name,
            conflict.claimants.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_index::TursoStore;

    /// A saved configuration marks the table stale on its own, so a
    /// registration whose follow-up fails (a sync after the config write)
    /// still answers to its names on the next lookup, with nothing synced.
    #[tokio::test]
    async fn a_saved_configuration_is_in_the_next_table() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.yaml");
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Engine::new(
            Arc::new(Mutex::new(store)),
            GlobalConfig::default(),
            None,
            Some(config_path),
        );
        assert_eq!(engine.name_table_now().await.resolve("fresh-alias"), None);

        let mut saved = GlobalConfig::default();
        let mut entry = DomainEntry::virtual_domain();
        entry.aliases = vec!["fresh-alias".to_string()];
        saved.domains.insert("fresh".to_string(), entry);
        engine.persist_config(&saved).unwrap();

        assert_eq!(
            engine.name_table_now().await.resolve("fresh-alias"),
            Some("fresh")
        );
    }
}
