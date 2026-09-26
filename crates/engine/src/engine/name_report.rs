//! What `crystalline doctor` asks about domain names, and the one repair it
//! offers for them.
//!
//! [`Engine::name_report`] reads the name table and the configuration: which
//! domains are shadowed, which names are contested, which aliases are
//! dropped, which team domains declare no name, which adoptions keep waiting,
//! and which links spell a domain by a name only this machine uses.
//! [`Engine::fix_local_spellings`] rewrites those links through the ordinary
//! write path. Both act as the machine owner: this is the operator's view,
//! reached over the ctl socket or a standalone engine, never through a
//! person's session.

use std::cell::RefCell;
use std::collections::BTreeMap;

use crystalline_core::names::NameTable;

use super::*;

impl Engine {
    /// The name findings, as JSON:
    ///
    /// ```json
    /// {
    ///   "shadowed": [{ "domain": "platform-2", "canonical": "platform", "held_by": "platform" }],
    ///   "conflicts": [{ "name": "shared", "claimants": ["a", "b"] }],
    ///   "dropped_aliases": [{ "domain": "a", "alias": "x", "held_by": "b" }],
    ///   "team_without_domain_name": [{ "domain": "kb", "repo": "acme/kb" }],
    ///   "adoption_pending": [{ "domain": "eng", "canonical": "platform",
    ///                          "canonical_seen": null, "reason": "..." }],
    ///   "local_spellings": [{ "domain": "ops", "path": "note.md", "spelling": "eng",
    ///                         "canonical": "engineering", "count": 2 }]
    /// }
    /// ```
    ///
    /// `held_by` of a dropped alias is `null` when no single domain owns the
    /// spelling. `team_without_domain_name` lists every domain with an origin
    /// whose MANIFEST declares no valid `domain_name`, since a team domain
    /// never gets one written automatically. `adoption_pending` lists a
    /// derived domain whose declared name reaches only it here but is not yet
    /// its local name (the adoption after a sync keeps trying), with a reason
    /// when one is known; a shadowed or contested name is reported under its
    /// own key instead. `local_spellings` counts, per engram and spelling,
    /// the links and `crystalline://` URLs [`Engine::fix_local_spellings`]
    /// would respell in the domain's own text. In a domain that reviews
    /// changes the fix lands in the owner's draft; once it has, the entry
    /// carries `in_draft: true`, since the domain's text changes only when
    /// that draft is accepted. Every list is sorted.
    pub async fn name_report(&self) -> Result<Value> {
        let table = self.name_table_now().await;
        let entries = self.registered_domain_entries_now().await;
        let busy: HashSet<String> = self.names_being_renamed().into_iter().collect();

        let mut shadowed = Vec::new();
        let mut team = Vec::new();
        let mut pending = Vec::new();
        for (name, entry) in &entries {
            let canonical = table.canonical(name).unwrap_or(name).to_string();
            if table.is_shadowed(name) {
                shadowed.push(json!({
                    "domain": name,
                    "canonical": canonical,
                    "held_by": table.resolve(&canonical),
                }));
            }
            if let Some(origin) = &entry.origin
                && self.declared_domain_name(name, entry).is_none()
            {
                team.push(json!({ "domain": name, "repo": origin.repo }));
            }
            let waits = entry.name_origin == Some(NameOrigin::Derived)
                && self.overlay.env_domain(name).is_none()
                && canonical != *name
                && table.resolve(&canonical) == Some(name.as_str())
                && entry.canonical_seen.as_deref() != Some(canonical.as_str());
            if waits {
                let reason = self
                    .adoption_reason(name, &canonical, &busy, &entries)
                    .await;
                pending.push(json!({
                    "domain": name,
                    "canonical": canonical,
                    "canonical_seen": entry.canonical_seen,
                    "reason": reason,
                }));
            }
        }
        let conflicts: Vec<Value> = table
            .conflicts()
            .iter()
            .map(|c| json!({ "name": c.name, "claimants": c.claimants }))
            .collect();
        let dropped: Vec<Value> = table
            .dropped_aliases()
            .iter()
            .map(|d| json!({ "domain": d.domain, "alias": d.alias, "held_by": d.held_by }))
            .collect();

        let mut local_spellings = Vec::new();
        for (domain, path, counts, in_draft) in self.local_spelling_counts(&table, &busy).await? {
            for (spelling, (canonical, count)) in counts {
                let mut entry = json!({
                    "domain": domain,
                    "path": path,
                    "spelling": spelling,
                    "canonical": canonical,
                    "count": count,
                });
                if in_draft {
                    entry["in_draft"] = json!(true);
                }
                local_spellings.push(entry);
            }
        }

        let by_domain = |a: &Value, b: &Value| {
            a["domain"]
                .as_str()
                .cmp(&b["domain"].as_str())
                .then(a["path"].as_str().cmp(&b["path"].as_str()))
        };
        shadowed.sort_by(by_domain);
        team.sort_by(by_domain);
        pending.sort_by(by_domain);
        Ok(json!({
            "shadowed": shadowed,
            "conflicts": conflicts,
            "dropped_aliases": dropped,
            "team_without_domain_name": team,
            "adoption_pending": pending,
            "local_spellings": local_spellings,
        }))
    }

    /// Respell every link and `crystalline://` URL that names a domain by a
    /// name only this machine uses (its local name or an alias, where the
    /// domain declares another name) to the domain's declared name, through
    /// the ordinary write path: a team domain records a local change and a
    /// reviewing domain takes the write as usual. Answers how many links were
    /// rewritten. A file that cannot be written is logged and left, so it is
    /// still in the next [`Engine::name_report`].
    pub async fn fix_local_spellings(&self) -> Result<u64> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let table = self.name_table_now().await;
        let busy: HashSet<String> = self.names_being_renamed().into_iter().collect();
        let scope = crate::scope::Scope::Unrestricted;
        let mut fixed: u64 = 0;
        for (domain, path, _, in_draft) in self.local_spelling_counts(&table, &busy).await? {
            // Already respelled in the owner's draft: nothing to write again.
            if in_draft {
                continue;
            }
            match self.respell_one(&domain, &path, &table, &scope).await {
                Ok(n) => fixed += n as u64,
                Err(e) => tracing::warn!(
                    domain = domain.as_str(),
                    path = path.as_str(),
                    "could not write the domain names into the links of this engram: {e}"
                ),
            }
        }
        Ok(fixed)
    }

    /// Whether an index row a removed domain left behind holds `name`: a row
    /// under the name that no registration here, no rename in flight and no
    /// other live instance on a shared index accounts for.
    pub(super) async fn leftover_row_holds(&self, name: &str) -> bool {
        if self
            .registered_domain_entries_now()
            .await
            .contains_key(name)
            || self.names_being_renamed().iter().any(|n| n == name)
        {
            return false;
        }
        let store = self.store.lock().await;
        let Ok(Some(id)) = store.domain_id(name).await else {
            return false;
        };
        match store.domain_host(id).await {
            Ok(Some(host)) => !self.held_elsewhere(
                Some(&host.instance_id),
                Some(&host.heartbeat_at),
                Utc::now(),
            ),
            Ok(None) => true,
            // Unknown is not left behind: nothing here could drop it.
            Err(_) => false,
        }
    }

    /// Why the adoption of `canonical` by `name` has not happened yet, in
    /// words, or `None` when nothing here says (the next sync tries again).
    /// The error of the last failed rename comes first; an engine that ran
    /// no adoption of its own (a standalone `doctor`) has only the reasons
    /// the state shows.
    async fn adoption_reason(
        &self,
        name: &str,
        canonical: &str,
        busy: &HashSet<String>,
        entries: &IndexMap<String, DomainEntry>,
    ) -> Option<String> {
        if let Some(error) = self.adoption_failures.lock().unwrap().get(name).cloned() {
            return Some(format!("the last rename to '{canonical}' failed: {error}"));
        }
        if !busy.is_empty() {
            return Some(
                "a rename has not finished yet; the adoption waits for it, and the next sync \
                 after it tries again"
                    .to_string(),
            );
        }
        if !entries.contains_key(canonical) && self.leftover_row_holds(canonical).await {
            return Some(format!(
                "the index still holds a row named '{canonical}' for a domain that is not \
                 registered here; run `crystalline doctor --fix` to drop it, then the next \
                 sync renames '{name}'"
            ));
        }
        None
    }

    /// Every base engram that spells a domain by a name the name table would
    /// rewrite, with its count per spelling: `(domain, path, spelling ->
    /// (canonical, count), in_draft)`, sorted by domain, then path. The count
    /// is taken with the same rewrite [`Engine::fix_local_spellings`]
    /// applies, so the report and the fix never disagree. `in_draft` says the
    /// fix already sits in the owner's review draft. `busy` is every name a
    /// rename holds ([`Engine::names_being_renamed`]), read once by the
    /// caller.
    #[allow(clippy::type_complexity)]
    async fn local_spelling_counts(
        &self,
        table: &NameTable,
        busy: &HashSet<String>,
    ) -> Result<Vec<(String, String, BTreeMap<String, (String, usize)>, bool)>> {
        let spellings: Vec<String> = table
            .spellings()
            .into_iter()
            .filter(|(spelling, _)| table.normalize(spelling).is_some())
            .map(|(spelling, _)| spelling)
            .collect();
        if spellings.is_empty() {
            return Ok(Vec::new());
        }
        let found = {
            let store = self.store.lock().await;
            store.engrams_referencing_domains(&spellings).await?
        };
        let mut out = Vec::new();
        for (domain, path) in found {
            let Ok(Some((_, _, text))) = self.base_text(&domain, &path).await else {
                continue;
            };
            let counts = count_respellings(&text, table);
            if !counts.is_empty() {
                let in_draft = self.draft_holds_fix(&domain, &path, table, busy).await;
                out.push((domain, path, counts, in_draft));
            }
        }
        Ok(out)
    }

    /// Whether `path` in a domain that reviews changes has an owner's draft
    /// that spells no local-only name any more: the fix is written and waits
    /// for review. Anything that cannot be read answers `false`, and so does
    /// a domain a rename holds: reading the draft takes a write ticket, which
    /// waits up to 30 s on a paused domain, once per engram.
    async fn draft_holds_fix(
        &self,
        domain: &str,
        path: &str,
        table: &NameTable,
        busy: &HashSet<String>,
    ) -> bool {
        if !self.reviews_changes(domain) || self.is_renaming(domain) || busy.contains(domain) {
            return false;
        }
        let scope = crate::scope::Scope::Unrestricted;
        let Ok(view) = DomainView::for_write(self, domain, &scope).await else {
            return false;
        };
        if view.actor().is_none() {
            return false;
        }
        let Ok(Some(desc)) = self.descriptor_at(domain, path).await else {
            return false;
        };
        let Ok(source) = self.content_source(domain) else {
            return false;
        };
        match view.text_at(&source, &desc).await {
            Ok(Some(text)) => respell_local(&text, table).1 == 0,
            _ => false,
        }
    }

    /// Respell one engram's local-only domain names, answering how many
    /// links changed; nothing is written when none would.
    async fn respell_one(
        &self,
        domain: &str,
        path: &str,
        table: &NameTable,
        scope: &crate::scope::Scope,
    ) -> Result<usize> {
        let view = DomainView::for_write(self, domain, scope).await?;
        let Some(desc) = self.descriptor_at(domain, path).await? else {
            return Ok(0);
        };
        let source = self.content_source(domain)?;
        let Some(text) = view.text_at(&source, &desc).await? else {
            return Ok(0);
        };
        let count = respell_local(&text, table).1;
        if count == 0 {
            return Ok(0);
        }
        let actor = self.actor_for(None, view.actor());
        self.apply_source_edit(
            &desc,
            &source,
            &view,
            None,
            &actor,
            None,
            scope,
            |current| Ok(respell_local(current, table).0),
        )
        .await?;
        Ok(count)
    }
}

/// `text` with every domain spelling the table would rewrite replaced by its
/// canonical name, and how many were.
fn respell_local(text: &str, table: &NameTable) -> (String, usize) {
    crystalline_core::relink::respell_domains(text, &|domain: &str| {
        table.normalize(domain).map(str::to_string)
    })
}

/// How many times each rewritable spelling occurs in `text`'s links, with
/// its canonical name.
fn count_respellings(text: &str, table: &NameTable) -> BTreeMap<String, (String, usize)> {
    let counts: RefCell<BTreeMap<String, (String, usize)>> = RefCell::new(BTreeMap::new());
    crystalline_core::relink::respell_domains(text, &|domain: &str| {
        let canonical = table.normalize(domain)?;
        counts
            .borrow_mut()
            .entry(domain.to_string())
            .or_insert_with(|| (canonical.to_string(), 0))
            .1 += 1;
        Some(canonical.to_string())
    });
    counts.into_inner()
}
