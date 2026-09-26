//! Renaming a domain on this machine: one engine operation that pauses the
//! domain, moves every store keyed by its name step by step under a journal,
//! and writes the configuration last, so a crash after any step is finished
//! by the next engine before it serves.

use crystalline_core::manifest::domain_name_at;

use super::*;
use crate::rename::{RENAME_WAIT, RenameJournal, RenameStep, WriteTicket, move_state_dir};

/// Frees the one-rename slot when the rename that took it ends, however it
/// ends.
struct RenameSlot<'a> {
    slot: &'a std::sync::Mutex<Option<String>>,
}

impl Drop for RenameSlot<'_> {
    fn drop(&mut self) {
        *self.slot.lock().unwrap() = None;
    }
}

impl Engine {
    /// Rename a domain on this machine only: its local name, and every store
    /// keyed by it (the index row, the visibility records, the `origins/` and
    /// `overlays/` state folders, the provision receipt and the
    /// configuration). The MANIFEST and the content are untouched; the old
    /// local name, the canonical name and every alias become aliases of the
    /// new name, so every link spelled with them keeps resolving.
    ///
    /// The domain is paused while it moves: a write into it waits (up to
    /// 30 s) and then lands under the new name, a sync or a watcher pass
    /// skips it, and the sync at the end covers what they skipped. A journal
    /// is written before the first step and after every one, so a crash is
    /// finished by [`Engine::recover_rename_journal`] before the next serve.
    ///
    /// Refused, before any step: on a read-only instance; for a caller who
    /// does not own the domain; for a domain an environment variable defines;
    /// for an invalid name or one another domain registers here; while
    /// another rename is running; while another live instance serves the same
    /// index; and when state a removed domain left under the new name is in
    /// the way. Renaming onto a name another domain declares in its MANIFEST
    /// or keeps as an alias is allowed; the report names that domain under
    /// `shadows`.
    ///
    /// The report: `{ domain, previous, local_only, aliases, moved,
    /// shadows }`, plus a `note` when something is shadowed.
    pub async fn rename_domain_local(
        &self,
        old: &str,
        new: &str,
        origin: NameOrigin,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let hidden = self.hidden_for(scope).await?;
        let table = self.name_table_now().await;
        let old = names::localize_in(&table, old, &hidden);
        let old = old.as_str();
        self.require_domain_owner_refusing(old, scope, Engine::rename_refusal(old))
            .await?;
        if let Some(env) = self.overlay.env_domain(old) {
            return Err(EngineError::Conflict(format!(
                "domain '{old}' is defined by the environment variable {}; rename the variable \
                 to rename the domain",
                env.var
            )));
        }
        validate_domain_name(new).map_err(EngineError::Invalid)?;
        if new == old {
            return Err(EngineError::Invalid(format!(
                "'{new}' is already this domain's name here; pick a different name"
            )));
        }

        let _slot = self.take_rename_slot(old)?;
        let _admin = self.domain_admin().await;
        let _fence = self.fence_joins().await;
        let state_dir = self.journal_state_dir()?;
        if let Some(journal) = RenameJournal::load(&state_dir).map_err(io_error)? {
            // The same rename a crash stopped: finish it rather than refuse.
            if journal.old == old && journal.new == new {
                let origin_lock = self.origin_lock(old);
                let _origin = origin_lock.lock().await;
                self.rename_pause.pause(&[old, new]);
                return self.finish_rename(journal, &state_dir).await;
            }
            return Err(EngineError::Conflict(format!(
                "a rename of '{}' is still finishing; try again in a moment",
                journal.old
            )));
        }

        let entry = self.domain_entry(old)?;
        if self.registered_domain_entries_now().await.contains_key(new) {
            return Err(EngineError::Conflict(format!(
                "'{new}' is already a domain here; pick another name or rename that domain first"
            )));
        }
        self.refuse_shared_index(old).await?;
        self.refuse_leftovers(new, &state_dir).await?;

        // An origin pull or share of this domain finishes first, and none
        // starts until the rename is done.
        let origin_lock = self.origin_lock(old);
        let _origin = origin_lock.lock().await;

        // Records a removed domain left under the new name are stale: a live
        // domain there was refused above. Forgotten before any step, so a
        // stale owner never gains the renamed domain, whether or not it has
        // records of its own to carry over.
        if let Some(access) = self.rename_access().await? {
            access.forget_domain(new).await.map_err(|e| {
                EngineError::Internal(format!(
                    "clearing stale records under '{new}' before the rename: {e:#}"
                ))
            })?;
        }

        let table = self.name_table_now().await;
        let shadows = shadowed_by(&table, old, new);
        let canonical = self.declared_domain_name(old, &entry);
        let mut old_spellings: Vec<String> = vec![old.to_string()];
        if let Some(canonical) = table.canonical(old) {
            old_spellings.push(canonical.to_string());
        }
        old_spellings.extend(canonical.iter().cloned());
        old_spellings.extend(entry.aliases.iter().cloned());
        let old_spellings = distinct_without(old_spellings, new);

        // Rooms save their last text while the domain is still registered
        // under its old name, and a join names the domain it was opened in.
        if let Some(sessions) = self.collab.get().and_then(std::sync::Weak::upgrade) {
            sessions.dispose_domain(old).await;
        }
        self.joins.end_domain(old);

        self.rename_pause.pause(&[old, new]);
        if !self.rename_pause.drained(old, RENAME_WAIT).await {
            self.rename_pause.resume(&[old, new]);
            return Err(EngineError::Conflict(format!(
                "domain '{old}' is busy with a write that has not finished; try again in a moment"
            )));
        }
        let journal = RenameJournal {
            version: 1,
            old: old.to_string(),
            new: new.to_string(),
            local_only: true,
            origin,
            old_spellings,
            canonical,
            done: Vec::new(),
        };
        if let Err(e) = journal.save(&state_dir) {
            self.rename_pause.resume(&[old, new]);
            return Err(io_error(e));
        }
        let mut report = self.finish_rename(journal, &state_dir).await?;
        if let Value::Object(map) = &mut report {
            if !shadows.is_empty() {
                map.insert(
                    "note".to_string(),
                    json!(format!(
                        "'{new}' was the name {} answered to; it now reaches this domain. \
                         Rename {} or change its domain_name to line them up.",
                        quoted_list(&shadows),
                        if shadows.len() == 1 { "it" } else { "them" }
                    )),
                );
            }
            map.insert("shadows".to_string(), json!(shadows));
        }
        Ok(report)
    }

    /// Finish a rename a crash left behind: every step the journal does not
    /// list as done, then the configuration, then the journal goes. Called
    /// before serving and before the first sync, by the daemon and by both
    /// standalone openers. No journal: `Ok(None)`.
    ///
    /// Runs on a read-only instance too: the rename was started by a
    /// writable one, and a half-renamed domain serves nothing correctly.
    pub async fn recover_rename_journal(&self) -> Result<Option<Value>> {
        // An engine that knows no state directory cannot have written a
        // journal.
        let Ok(state_dir) = self.journal_state_dir() else {
            return Ok(None);
        };
        let Some(journal) = RenameJournal::load(&state_dir).map_err(io_error)? else {
            return Ok(None);
        };
        let _slot = self.take_rename_slot(&journal.old)?;
        let _admin = self.domain_admin().await;
        let _fence = self.fence_joins().await;
        let origin_lock = self.origin_lock(&journal.old);
        let _origin = origin_lock.lock().await;
        self.rename_pause.pause(&[&journal.old, &journal.new]);
        tracing::info!(
            "finishing the rename of domain '{}' to '{}' that an earlier run left half done",
            journal.old,
            journal.new
        );
        self.finish_rename(journal, &state_dir).await.map(Some)
    }

    /// [`Engine::recover_rename_journal`] for an opener: the daemon, the
    /// embedded MCP stack and the standalone command engine call it before
    /// anything reads a name. The outcome is logged rather than returned: a
    /// rename that cannot be finished keeps its one domain paused and says
    /// why, and every other domain is served.
    pub async fn finish_leftover_rename(&self) {
        match self.recover_rename_journal().await {
            Ok(Some(report)) => tracing::info!(
                "finished renaming domain '{}' to '{}', which an earlier run left half done",
                report["previous"].as_str().unwrap_or_default(),
                report["domain"].as_str().unwrap_or_default()
            ),
            Ok(None) => {}
            Err(err) => tracing::error!(
                "a domain rename an earlier run left half done could not be finished ({err}); \
                 that domain stays paused until the next start or until the rename is run again"
            ),
        }
    }

    /// Whether `domain` is being renamed right now, under its old name or its
    /// new one.
    pub fn is_renaming(&self, domain: &str) -> bool {
        self.rename_pause.is_paused(domain)
    }

    /// Arm a failure right after `step` of the next rename, leaving its
    /// journal behind as a crash would; `None` disarms it.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_rename_after(&self, step: Option<RenameStep>) {
        *self.rename_fail_after.lock().unwrap() = step;
    }

    /// Hold the next rename right after `step` until the answered hold is
    /// released.
    #[cfg(any(test, feature = "testing"))]
    pub fn hold_rename_after(&self, step: RenameStep) -> Arc<crate::rename::RenameHold> {
        let hold = Arc::new(crate::rename::RenameHold::default());
        *self.rename_hold.lock().unwrap() = Some((step, hold.clone()));
        hold
    }

    /// Wait while a rename has paused one of the domains `spellings` name,
    /// through the table as it stands. A no-op when nothing is paused.
    /// `Err` with the conflict a caller answers when the wait runs out.
    pub(super) async fn wait_for_renames(&self, spellings: &[String]) -> Result<()> {
        if !self.rename_pause.any() {
            return Ok(());
        }
        let table = self.names.read().unwrap().1.clone();
        let mut names: Vec<String> = spellings.to_vec();
        names.extend(
            spellings
                .iter()
                .filter_map(|s| table.resolve(s).map(str::to_string)),
        );
        self.rename_pause
            .wait_clear(&names, RENAME_WAIT)
            .await
            .map(|_| ())
            .map_err(|busy| {
                // Answered in the caller's own words: the local name a
                // canonical name or alias resolves to may be one of a domain
                // this caller may not see.
                let typed = spellings
                    .iter()
                    .find(|s| **s == busy || table.resolve(s) == Some(busy.as_str()))
                    .cloned()
                    .unwrap_or(busy);
                renaming_conflict(typed)
            })
    }

    /// Count one write into `name` for as long as the ticket lives, waiting
    /// first while a rename has the domain paused. A write that waited for a
    /// rename of the very name it holds is refused, since the name it holds
    /// is gone; it is sent again under the name it reaches now.
    pub(crate) async fn enter_write(&self, name: &str) -> Result<WriteTicket<'_>> {
        loop {
            let waited = self
                .rename_pause
                .wait_clear(&[name.to_string()], RENAME_WAIT)
                .await
                .map_err(renaming_conflict)?;
            if waited && !self.registered_domain_names().contains(name) {
                return Err(EngineError::Conflict(format!(
                    "domain '{name}' was renamed while this request waited; send it again"
                )));
            }
            if let Some(ticket) = self.rename_pause.try_enter(name) {
                return Ok(ticket);
            }
        }
    }

    /// Count one pass of a sync or the watcher into `name`, or `None` when a
    /// rename has it paused: the pass is skipped, and the sync that ends the
    /// rename covers it.
    pub(super) fn try_enter_sync(&self, name: &str) -> Option<WriteTicket<'_>> {
        self.rename_pause.try_enter(name)
    }

    /// Every name a rename in flight or a leftover journal holds, which a
    /// sweep of unregistered domains must count as registered.
    pub(super) fn names_being_renamed(&self) -> Vec<String> {
        let mut names = self.rename_pause.names();
        if let Ok(dir) = self.journal_state_dir()
            && let Ok(Some(journal)) = RenameJournal::load(&dir)
        {
            names.push(journal.old);
            names.push(journal.new);
        }
        names
    }

    /// Whether a spelling push has to wait for a rename's config step.
    pub(super) fn names_frozen(&self) -> bool {
        self.names_frozen.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Run every step of `journal` it does not list as done, re-key what
    /// memory holds, write the configuration, drop the journal, publish the
    /// names, lift the pause and sync the domain under its new name.
    async fn finish_rename(&self, mut journal: RenameJournal, state_dir: &Path) -> Result<Value> {
        let (old, new) = (journal.old.clone(), journal.new.clone());
        let mut moved: Vec<&'static str> = Vec::new();
        for &step in journal.steps() {
            if !journal.done.contains(&step) {
                // From the index row step until the configuration is written,
                // no spelling push; the MANIFEST and relink steps of a full
                // rename come before that window.
                if !matches!(step, RenameStep::Manifest | RenameStep::Relink) {
                    self.names_frozen
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if step == RenameStep::Config {
                    self.rekey_in_memory(&old, &new);
                }
                if let Err(e) = self.run_rename_step(step, &journal, state_dir).await {
                    return Err(self.rename_stopped(&journal, step, e));
                }
                journal.done.push(step);
                journal
                    .save(state_dir)
                    .map_err(|e| self.rename_stopped(&journal, step, io_error(e)))?;
            }
            moved.push(step.name());
            #[cfg(any(test, feature = "testing"))]
            self.rename_checkpoint(step).await?;
        }
        self.names_frozen
            .store(false, std::sync::atomic::Ordering::SeqCst);
        RenameJournal::remove(state_dir).map_err(io_error)?;

        // The table and the index learn the new name and keep the old
        // spellings as aliases; references the index row step unbound (the
        // new name was another domain's spelling) bind again.
        self.mark_names_stale();
        self.refresh_names().await;
        self.resolve_pending_everywhere().await;
        self.origin_locks.lock().unwrap().remove(&old);
        self.rename_pause.resume(&[&old, &new]);

        let entry = self.domain_entry(&new)?;
        if let Err(e) = self.sync(Some(&new)).await {
            tracing::warn!(
                domain = %new,
                error = %e,
                "the sync after renaming '{old}' to '{new}' failed; the next sync catches up"
            );
        }
        Ok(json!({
            "domain": new,
            "previous": old,
            "local_only": journal.local_only,
            "aliases": entry.aliases,
            "moved": moved,
        }))
    }

    /// A step failed: the journal stays and the domain stays paused, since
    /// half its stores answer to the new name and half to the old one. The
    /// next start finishes it; so does sending the same rename
    /// again while the configuration still holds the old name.
    fn rename_stopped(
        &self,
        journal: &RenameJournal,
        step: RenameStep,
        e: EngineError,
    ) -> EngineError {
        tracing::error!(
            "the rename of domain '{}' to '{}' stopped at its {} step: {e}",
            journal.old,
            journal.new,
            step.name()
        );
        EngineError::Internal(format!(
            "the rename of domain '{}' to '{}' stopped at its {} step ({e}); it finishes the \
             next time the daemon starts",
            journal.old,
            journal.new,
            step.name()
        ))
    }

    /// The test seams after each step: hold, then fail.
    #[cfg(any(test, feature = "testing"))]
    async fn rename_checkpoint(&self, step: RenameStep) -> Result<()> {
        let hold = {
            let mut armed = self.rename_hold.lock().unwrap();
            match armed.as_ref() {
                Some((at, _)) if *at == step => armed.take().map(|(_, hold)| hold),
                _ => None,
            }
        };
        if let Some(hold) = hold {
            hold.hold().await;
        }
        let fail = {
            let mut armed = self.rename_fail_after.lock().unwrap();
            if *armed == Some(step) {
                armed.take()
            } else {
                None
            }
        };
        if fail.is_some() {
            return Err(EngineError::Internal(format!(
                "the rename stopped after its {} step (test failpoint)",
                step.name()
            )));
        }
        Ok(())
    }

    /// One step, safe to run again.
    async fn run_rename_step(
        &self,
        step: RenameStep,
        journal: &RenameJournal,
        state_dir: &Path,
    ) -> Result<()> {
        let (old, new) = (journal.old.as_str(), journal.new.as_str());
        match step {
            RenameStep::IndexRow => {
                let store = self.store.lock().await;
                store.rename_domain_row(old, new).await?;
            }
            RenameStep::AuthTables => {
                if let Some(access) = self.rename_access().await? {
                    access.rename_domain(old, new).await.map_err(|e| {
                        EngineError::Internal(format!("moving the records of '{old}': {e:#}"))
                    })?;
                }
            }
            RenameStep::OriginsDir => {
                move_state_dir(&self.rename_origins_root()?, old, new).map_err(io_error)?;
            }
            RenameStep::OverlaysDir => {
                move_state_dir(
                    &state_dir.join(crate::overlay_journal::JOURNAL_DIR),
                    old,
                    new,
                )
                .map_err(io_error)?;
            }
            RenameStep::ProvisionReceipt => {
                let path = state_dir.join(RECEIPT_FILE);
                if path.is_file() {
                    let internal = |e: anyhow::Error| EngineError::Internal(format!("{e:#}"));
                    let mut receipt =
                        crystalline_core::provision::receipt::load(&path).map_err(internal)?;
                    if receipt.rename_domain(old, new) {
                        crystalline_core::provision::receipt::save(&path, &receipt)
                            .map_err(internal)?;
                    }
                }
            }
            RenameStep::Config => self.rename_config(journal)?,
            RenameStep::Manifest | RenameStep::Relink => {
                return Err(EngineError::Internal(format!(
                    "this version cannot finish the {} step of a rename; run the version that \
                     started it",
                    step.name()
                )));
            }
        }
        Ok(())
    }

    /// The Config step: `old` leaves the configuration and `new` takes its
    /// place with the same entry, its name origin recorded and every old
    /// spelling kept as an alias. Load-modify-save through `file_config` then
    /// `config`, as registration does; the table is marked stale only after
    /// both hold the new name. A configuration that already carries `new` has
    /// the same merge applied again, which changes nothing.
    fn rename_config(&self, journal: &RenameJournal) -> Result<()> {
        let (old, new) = (journal.old.as_str(), journal.new.as_str());
        let entry = {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard);
            let (index, mut entry) = match (
                file.domains.get_index_of(old),
                file.domains.get_index_of(new),
            ) {
                (Some(_), Some(_)) => {
                    return Err(EngineError::Conflict(format!(
                        "both '{old}' and '{new}' are in the configuration file; remove the \
                             one that is not this domain by hand and rerun"
                    )));
                }
                (Some(i), None) | (None, Some(i)) => {
                    let (_, entry) = file.domains.shift_remove_index(i).expect("indexed");
                    (i, entry)
                }
                (None, None) => {
                    return Err(EngineError::Internal(format!(
                        "domain '{old}' is no longer in the configuration file, so the \
                             rename to '{new}' has nothing to write"
                    )));
                }
            };
            entry.name_origin = Some(journal.origin);
            let mut aliases = entry.aliases.clone();
            aliases.extend(journal.old_spellings.iter().cloned());
            entry.aliases = distinct_without(aliases, new);
            if journal.canonical.is_some() {
                entry.canonical_seen = journal.canonical.clone();
            }
            file.domains
                .shift_insert(index, new.to_string(), entry.clone());
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
            entry
        };
        self.discovered_domains.write().unwrap().remove(old);
        // After both configurations hold the new name: a lookup that took the
        // stale mark persist set would have built from the old snapshot.
        self.mark_names_stale();
        if let Some(tx) = &self.watch_tx
            && !entry.is_virtual()
            && let Some(root) = entry.file_path()
        {
            let _ = tx.send(WatchEvent::Remove(old.to_string()));
            let _ = tx.send(WatchEvent::Add(new.to_string(), root));
        }
        Ok(())
    }

    /// What memory keys by the old name moves to the new one before the
    /// configuration does.
    fn rekey_in_memory(&self, old: &str, new: &str) {
        {
            let mut hosted = self.hosted.write().unwrap();
            if let Some(id) = hosted.remove(old) {
                hosted.insert(new.to_string(), id);
            }
        }
        self.origin_poller.rename_domain(old, new);
        {
            let mut routing = self.routing_virtual.write().unwrap();
            if let Some(bullets) = routing.remove(old) {
                routing.insert(new.to_string(), bullets);
            }
        }
        {
            let mut declared = self.virtual_domain_names.write().unwrap();
            if let Some(name) = declared.remove(old) {
                declared.insert(new.to_string(), name);
            }
        }
    }

    /// Take the one-rename slot, or say which rename holds it.
    fn take_rename_slot(&self, old: &str) -> Result<RenameSlot<'_>> {
        let mut slot = self.rename_slot.lock().unwrap();
        if let Some(running) = slot.as_deref() {
            return Err(EngineError::Conflict(format!(
                "a rename of '{running}' is still finishing; try again in a moment"
            )));
        }
        *slot = Some(old.to_string());
        Ok(RenameSlot {
            slot: &self.rename_slot,
        })
    }

    /// Refuse while another live instance serves this index: the domain row
    /// is shared, so a rename on this machine alone would rename the domain
    /// under that instance too.
    async fn refuse_shared_index(&self, old: &str) -> Result<()> {
        let now = Utc::now();
        let stats = {
            let store = self.store.lock().await;
            store.domain_stats().await?
        };
        if let Some(row) = stats.iter().find(|row| self.hosted_elsewhere(row, now)) {
            return Err(EngineError::Conflict(format!(
                "domain '{old}' is in an index that instance {} also serves (last heartbeat {}); \
                 a rename on this machine alone would rename it under that instance too. Stop \
                 that instance first, then rename",
                row.host_instance_id.as_deref().unwrap_or_default(),
                row.host_heartbeat_at.as_deref().unwrap_or("unknown")
            )));
        }
        Ok(())
    }

    /// Refuse when state a removed domain left under the new name is in the
    /// way: an index row (removal keeps it) or a state folder. Both would
    /// otherwise stop the rename halfway or hand the old state to it.
    async fn refuse_leftovers(&self, new: &str, state_dir: &Path) -> Result<()> {
        let row = {
            let store = self.store.lock().await;
            store.domain_id(new).await?
        };
        if row.is_some() {
            return Err(EngineError::Conflict(format!(
                "the index still holds a domain named '{new}' from a domain removed earlier; \
                 pick another name"
            )));
        }
        for parent in [
            self.rename_origins_root()?,
            state_dir.join(crate::overlay_journal::JOURNAL_DIR),
        ] {
            let taken = match std::fs::read_dir(&parent) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.file_name().to_string_lossy() == new),
                Err(_) => false,
            };
            if taken {
                return Err(EngineError::Conflict(format!(
                    "{} is left over from a domain once named '{new}'; move it out of the way \
                     and rename again",
                    parent.join(new).display()
                )));
            }
        }
        Ok(())
    }

    /// The accounts database the rename moves records in: the one the HTTP
    /// surface installed, else the file under the state directory, opened for
    /// this rename alone (a daemon at startup and a standalone command have
    /// none installed). `None` when there is no accounts database at all.
    async fn rename_access(&self) -> Result<Option<Arc<crate::scope::DomainAccess>>> {
        if let Some(access) = self.domain_access.get() {
            return Ok(Some(access.clone()));
        }
        let path = self.journal_state_dir()?.join("web-auth.db");
        if !path.is_file() {
            return Ok(None);
        }
        let auth = crate::auth_store::AuthStore::open(&path)
            .await
            .map_err(|e| {
                EngineError::Internal(format!(
                    "opening the accounts database {}: {e:#}",
                    path.display()
                ))
            })?;
        Ok(Some(Arc::new(crate::scope::DomainAccess::new(Arc::new(
            auth,
        )))))
    }

    /// Where the `origins/<name>/` folders live: the test override, else the
    /// state directory's `origins`, which is where the real resolver puts
    /// them too.
    fn rename_origins_root(&self) -> Result<PathBuf> {
        match &self.origins_dir_override {
            Some(dir) => Ok(dir.clone()),
            None => Ok(self.journal_state_dir()?.join("origins")),
        }
    }

    /// The valid name `name`'s MANIFEST declares, if any.
    fn declared_domain_name(&self, name: &str, entry: &DomainEntry) -> Option<String> {
        if entry.is_virtual() {
            return self.virtual_domain_names.read().unwrap().get(name).cloned();
        }
        entry.file_path().and_then(|root| domain_name_at(&root))
    }

    /// The refusal for a caller who may see the domain and does not own it.
    fn rename_refusal(name: &str) -> EngineError {
        EngineError::Forbidden(format!(
            "renaming domain '{name}' is for an instance admin, or for the owner of a private \
             domain; ask an admin to rename it"
        ))
    }
}

/// The provision receipt's file name under the state directory.
const RECEIPT_FILE: &str = "provisions.json";

/// The conflict a request answers when a rename kept a domain paused past the
/// wait.
fn renaming_conflict(name: String) -> EngineError {
    EngineError::Conflict(format!(
        "domain '{name}' is being renamed; try again in a moment"
    ))
}

fn io_error(e: std::io::Error) -> EngineError {
    EngineError::Internal(e.to_string())
}

/// `names` in their first order, each once, without `new`.
fn distinct_without(names: Vec<String>, new: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        if name != new && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The domains that answered to `new` before a rename of `old` gave it to
/// `old`: the one it resolved to, or every claimant of a contested canonical.
fn shadowed_by(table: &crystalline_core::names::NameTable, old: &str, new: &str) -> Vec<String> {
    let mut shadows: Vec<String> = table
        .resolve(new)
        .filter(|local| *local != old)
        .map(str::to_string)
        .into_iter()
        .collect();
    for conflict in table.conflicts() {
        if conflict.name == new {
            shadows.extend(conflict.claimants.iter().filter(|c| *c != old).cloned());
        }
    }
    shadows.sort();
    shadows.dedup();
    shadows
}

/// `'a'`, `'a' and 'b'`, `'a', 'b' and 'c'`.
fn quoted_list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}
