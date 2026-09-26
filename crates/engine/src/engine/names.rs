//! The name table cache and the spelling push.
//!
//! One [`NameTable`] over every registered domain answers which local name a
//! spelling means: the local name itself, the canonical name the domain's
//! MANIFEST declares, or a machine-local alias. The engine caches it and
//! rebuilds it when a registration or a declared name may have changed, and
//! pushes its spellings into the index's `domain_spelling` table, which the
//! resolve pass reads to bind a reference spelled with any of them.

use std::sync::atomic::Ordering as AtomicOrdering;

use crystalline_core::manifest::{domain_name_at, domain_name_of_source};
use crystalline_core::names::{NameInput, NameTable};

use super::*;
use crate::params::DomainArgs;

/// The path of a domain's MANIFEST, relative to its root.
const MANIFEST_PATH: &str = "MANIFEST.md";

impl Engine {
    /// The current name table, rebuilt when stale. Never touches the store.
    ///
    /// A stale rebuild reads the configuration file and every file domain's
    /// MANIFEST on the calling thread; a verb on the async path asks
    /// [`Engine::name_table_now`] instead, which reads them on the blocking
    /// pool.
    pub fn name_table(&self) -> Arc<NameTable> {
        if self.names_stale.swap(false, AtomicOrdering::SeqCst) {
            let generation = self.names_ticket();
            let virtual_names = self.virtual_domain_names.read().unwrap().clone();
            let table = build_table(&self.registered_domain_entries(), &virtual_names);
            return self.install_names(generation, table);
        }
        self.names.read().unwrap().1.clone()
    }

    /// [`Engine::name_table`] for a verb: a stale rebuild runs on the blocking
    /// pool, and the first call on an engine that never read its virtual
    /// domains' MANIFEST engrams reads them first, so a one-shot command or a
    /// REST request before any MCP connection knows the names they declare.
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
        self.reload_virtual_manifests().await;
        // Rebuilt here rather than marked and left to `name_table`: a lookup
        // racing this call may have taken the stale mark and be building from
        // what it read before, and the push must carry what is true now.
        self.names_stale.store(false, AtomicOrdering::SeqCst);
        let table = self.rebuild_names().await;
        let pairs = table.spellings();
        let store = self.store.lock().await;
        if let Err(e) = self.push_spellings(&*store, &pairs).await {
            tracing::warn!("recording the domain name spellings in the index failed: {e}");
        }
    }

    /// The local name `spelling` resolves to, if any.
    pub fn local_domain_name(&self, spelling: &str) -> Option<String> {
        self.name_table().resolve(spelling).map(str::to_string)
    }

    /// `local_domain_name`, falling back to the input unchanged so a later
    /// `UnknownDomain` names what the caller typed.
    pub fn localize(&self, spelling: &str) -> String {
        self.local_domain_name(spelling)
            .unwrap_or_else(|| spelling.to_string())
    }

    /// `localize` for one caller: a spelling that resolves to a domain in
    /// `hidden` stays exactly as typed, so the ordinary unknown-domain path
    /// answers it with the caller's own words and never names the local name.
    pub async fn localize_visible(&self, spelling: &str, hidden: &HashSet<String>) -> String {
        let table = self.name_table_now().await;
        localize_in(&table, spelling, hidden)
    }

    /// `p` with every domain it names mapped to a local name, through
    /// [`Engine::localize_visible`]. Mapping a local name answers it
    /// unchanged, so a value localized twice is localized once.
    pub async fn localized<P: DomainArgs + Clone>(&self, p: &P, hidden: &HashSet<String>) -> P {
        let table = self.name_table_now().await;
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
        let table = self.name_table_now().await;
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
    async fn registered_domain_entries_now(&self) -> IndexMap<String, DomainEntry> {
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
    pub fn install_names_for_test(&self, generation: u64, inputs: &[NameInput]) -> Arc<NameTable> {
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
    /// decide.
    async fn push_spellings(
        &self,
        store: &dyn Store,
        pairs: &[(String, String)],
    ) -> crystalline_index::Result<()> {
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
            return Ok(());
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
            return Ok(());
        }

        #[cfg(any(test, feature = "testing"))]
        self.spelling_replaces.fetch_add(1, AtomicOrdering::Relaxed);
        let changed = store.replace_domain_spellings(&list).await?;
        if changed.is_empty() {
            return Ok(());
        }
        tracing::debug!(spellings = ?changed, "domain name spellings changed meaning");
        store.reset_references_to_spellings(&changed).await?;

        // The reset unbound references in every domain that spelled one of
        // the changed names, whoever registers it, so every row gets a pass.
        let mut every: Vec<DomainId> = store
            .domain_spellings()
            .await?
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        every.sort_by_key(|id| id.0);
        every.dedup();
        store.begin().await?;
        let pass = async {
            for id in &every {
                store.resolve_pending_relations(*id).await?;
                store.resolve_pending_links(*id).await?;
            }
            Ok::<(), crystalline_index::IndexError>(())
        }
        .await;
        match pass {
            Ok(()) => store.commit().await,
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }
}

/// The table over `entries`: each file domain's canonical name read from its
/// MANIFEST on disk, each virtual domain's from `virtual_names`.
fn build_table(
    entries: &IndexMap<String, DomainEntry>,
    virtual_names: &BTreeMap<String, String>,
) -> NameTable {
    let inputs: Vec<NameInput> = entries
        .iter()
        .map(|(local, entry)| NameInput {
            local: local.clone(),
            canonical: if entry.is_virtual() {
                virtual_names.get(local).cloned()
            } else {
                entry.file_path().and_then(|root| domain_name_at(&root))
            },
            aliases: entry.aliases.clone(),
        })
        .collect();
    NameTable::build(&inputs)
}

/// The local name `spelling` means in `table`, unless that domain is in
/// `hidden`: then, as for a spelling nothing answers to, the spelling itself.
fn localize_in(table: &NameTable, spelling: &str, hidden: &HashSet<String>) -> String {
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
        assert_eq!(engine.name_table().resolve("fresh-alias"), None);

        let mut saved = GlobalConfig::default();
        let mut entry = DomainEntry::virtual_domain();
        entry.aliases = vec!["fresh-alias".to_string()];
        saved.domains.insert("fresh".to_string(), entry);
        engine.persist_config(&saved).unwrap();

        assert_eq!(engine.name_table().resolve("fresh-alias"), Some("fresh"));
    }
}
