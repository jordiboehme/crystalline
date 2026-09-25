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

/// The path of a domain's MANIFEST, relative to its root.
const MANIFEST_PATH: &str = "MANIFEST.md";

impl Engine {
    /// The current name table, rebuilt when stale. Never touches the store.
    pub fn name_table(&self) -> Arc<NameTable> {
        if self.names_stale.swap(false, AtomicOrdering::SeqCst) {
            return self.rebuild_names();
        }
        self.names.read().unwrap().clone()
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
        let table = self.rebuild_names();
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

    /// Build the table from every registration, store it and answer it. Warns
    /// about a dropped alias or a contested canonical name when it is new
    /// since the previous table, so a refresh on every connection does not
    /// repeat the same warning.
    fn rebuild_names(&self) -> Arc<NameTable> {
        let entries = self.registered_domain_entries();
        let virtual_names = self.virtual_domain_names.read().unwrap().clone();
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
        let table = Arc::new(NameTable::build(&inputs));
        let previous = std::mem::replace(&mut *self.names.write().unwrap(), table.clone());
        warn_new_name_problems(&previous, &table);
        table
    }

    /// Read every virtual domain's MANIFEST engram once and cache both what
    /// it routes by and the name it declares.
    async fn reload_virtual_manifests(&self) {
        let mut bullets = BTreeMap::new();
        let mut names = BTreeMap::new();
        for (name, entry) in &self.registered_domain_entries() {
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

/// Warn about each dropped alias and each contested canonical name `table`
/// has and `previous` did not.
fn warn_new_name_problems(previous: &NameTable, table: &NameTable) {
    for dropped in table.dropped_aliases() {
        if previous.dropped_aliases().contains(dropped) {
            continue;
        }
        match &dropped.held_by {
            Some(owner) => tracing::warn!(
                "alias '{}' of domain '{}' is ignored: domain '{owner}' already answers to that name",
                dropped.alias,
                dropped.domain
            ),
            None => tracing::warn!(
                "alias '{}' of domain '{}' is ignored: more than one domain claims that name",
                dropped.alias,
                dropped.domain
            ),
        }
    }
    for conflict in table.conflicts() {
        if previous.conflicts().contains(conflict) {
            continue;
        }
        tracing::warn!(
            "domain name '{}' resolves nowhere: domains {} all declare it in their MANIFEST",
            conflict.name,
            conflict.claimants.join(", ")
        );
    }
}
