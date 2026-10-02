//! Lining each domain's name up with its MANIFEST once a sync or a pull has
//! finished: the one-time catch-up for entries written before 0.20.0, then
//! the adoption of a new or changed canonical name.
//!
//! Never called from [`Engine::sync`] itself: the surfaces that sync (the
//! daemon's startup and watcher, the ctl `sync`, an origin update or poll
//! tick, the CLI's direct sync) call it after their sync has returned, so
//! the sync a rename ends with, or the one inside a registration, never runs
//! an adoption of its own.

#[cfg(any(test, feature = "testing"))]
use std::sync::atomic::Ordering as AtomicOrdering;

use crystalline_core::names::NameTable;

use super::*;

impl Engine {
    /// Runs after a sync pass. First the one-time catch-up: every entry with
    /// `name_origin: None` gets [`infer_name_origin`], persisted, and the
    /// MANIFEST write-back where [`needs_manifest_write_back`] says so (never
    /// for a team domain). Then, per domain whose canonical name differs from
    /// `canonical_seen`: the previous canonical (when it differs from the
    /// local name) becomes an alias; a derived domain whose new canonical
    /// reaches only it here is renamed to it with
    /// [`Engine::rename_domain_local`] recorded as derived; an explicit one
    /// keeps its name; a name another domain holds here is left shadowed.
    /// `canonical_seen` is updated in every case but two, both so a later
    /// pass tries again: a planned rename, whose own config step records it
    /// (a rename that fails is tried after the next sync), and a derived
    /// domain whose name is shadowed or contested (it adopts the name once
    /// the other claimant goes away).
    ///
    /// Skipped for a domain an environment variable defines (nothing about it
    /// can be persisted) and for a domain a rename holds or a leftover rename
    /// journal names (the journal finishes that rename, not this). While any
    /// rename is pending, planned renames wait for a later pass.
    ///
    /// Serialized with every other registration change through
    /// [`Engine::domain_admin`], which it releases before a rename takes it
    /// again; so no caller may hold that lock. Two adoptions run one after the
    /// other, the second one waiting for the first one's renames. A rename
    /// that fails is logged and reported, never returned: the sync this rides
    /// on has landed.
    ///
    /// Answers the list of what it did, for logs and for the ctl `sync`
    /// reply's `names`: `{domain, action, ..}` with `action` one of
    /// `inferred`, `alias`, `kept`, `shadowed`, `contested`, `deferred`,
    /// `waiting`, `renamed` or `failed`; a derived domain's `shadowed` or
    /// `contested` comes back on every pass while it lasts, without a log
    /// line, and so does `waiting` (`reason: leftover_row`: an empty index
    /// row of a removed domain holds the name until `crystalline doctor
    /// --fix` drops it). Empty on
    /// a read-only instance and on an engine that knows no configuration file
    /// of its own.
    pub async fn adopt_domain_names(&self) -> Result<Value> {
        #[cfg(any(test, feature = "testing"))]
        self.adoptions.fetch_add(1, AtomicOrdering::Relaxed);
        // A read-only instance writes no configuration; an engine with no
        // path of its own would write the machine's global file.
        if self.read_only || self.config_path.is_none() {
            return Ok(json!([]));
        }
        // An adoption renames this machine's state folders and configuration
        // keys: only the process that holds the state directory does it.
        if !self.holds_state_dir() {
            tracing::info!(
                "domain names are not lined up with their MANIFESTs here: this command does not \
                 hold this machine's state directory; the daemon, or a plain `crystalline sync`, \
                 does it"
            );
            return Ok(json!([]));
        }
        // Nor over an index `--db` or `--config` named instead of this
        // machine's own: the renames would move this machine's state folders
        // and write its configuration while its own index keeps the old names.
        match self.off_machine().await {
            Ok(None) => {}
            Ok(Some(why)) => {
                tracing::warn!(
                    "domain names are not lined up with their MANIFESTs here: lining them up \
                     renames this machine's own state and configuration, and {why}; a \
                     Crystalline started without --db and --config does it"
                );
                return Ok(json!([]));
            }
            Err(e) => {
                tracing::warn!(
                    "domain names are not lined up with their MANIFESTs here: whether this is \
                     this machine's own index could not be told ({e})"
                );
                return Ok(json!([]));
            }
        }
        // One adoption at a time, from its plan to the end of its renames.
        // `domain_admin` covers only the plan, because a rename takes it
        // again. Without this, a second adoption (the watcher's and a ctl
        // `sync` right after the same MANIFEST change) planned the same
        // rename while the first one was about to run it, then found the
        // rename slot taken and reported the rename as failed while it was
        // landing. Waiting here instead, the second one plans from what the
        // first one wrote and finds nothing left to do.
        let _adopting = self.adoption_lock.lock().await;
        let mut report: Vec<Value> = Vec::new();
        let planned = {
            let _admin = self.domain_admin().await;
            let busy: HashSet<String> = self.names_being_renamed().into_iter().collect();

            let caught_up = self.infer_missing_name_origins(&busy)?;
            for (name, origin) in &caught_up {
                let written = match self.write_back_domain_name(name).await {
                    Ok(written) => written,
                    Err(e) => {
                        tracing::warn!(
                            domain = %name,
                            error = %e,
                            "writing the domain name back into its MANIFEST failed; add \
                             domain_name: {name} to it by hand"
                        );
                        false
                    }
                };
                report.push(json!({
                    "domain": name,
                    "action": "inferred",
                    "name_origin": origin,
                    "manifest_written": written,
                }));
            }
            if !caught_up.is_empty() {
                self.refresh_names().await;
            }
            let table = self.name_table_now().await;
            self.record_canonicals(&table, &busy, &mut report)?
        };
        #[cfg(any(test, feature = "testing"))]
        {
            let hold = self.adoption_hold.lock().unwrap().take();
            if let Some(hold) = hold {
                hold.hold().await;
            }
        }

        for (old, new) in planned {
            // An empty row an older version's `domain remove` left under the
            // name refuses the rename. Nothing a sync does changes that, so
            // the adoption waits quietly (on every sync, without a warning)
            // and `crystalline doctor` names the row and drops it with --fix.
            if self.leftover_row_holds(&new).await {
                tracing::debug!(
                    domain = %old,
                    "renaming '{old}' to '{new}' waits: the index still holds a row named \
                     '{new}' from a removed domain; `crystalline doctor --fix` drops it"
                );
                self.adoption_failures.lock().unwrap().remove(&old);
                report.push(json!({
                    "domain": old,
                    "canonical": new,
                    "action": "waiting",
                    "reason": "leftover_row",
                }));
                continue;
            }
            match self
                .rename_domain_local(
                    &old,
                    &new,
                    NameOrigin::Derived,
                    &crate::scope::Scope::Unrestricted,
                )
                .await
            {
                Ok(_) => {
                    self.adoption_failures.lock().unwrap().remove(&old);
                    tracing::info!(
                        "domain '{old}' is now called '{new}', the name its MANIFEST declares; \
                         '{old}' stays an alias"
                    );
                    report.push(json!({ "domain": new, "previous": old, "action": "renamed" }));
                }
                Err(e) => {
                    // Another adoption (an update's and the watcher's, after
                    // one pull) got there between this one's plan and its
                    // rename: the domain already has the name.
                    let registered = self.registered_domain_entries_now().await;
                    if registered.contains_key(&new) && !registered.contains_key(&old) {
                        continue;
                    }
                    tracing::warn!(
                        domain = %old,
                        error = %e,
                        "the MANIFEST of '{old}' declares domain_name '{new}', but renaming it \
                         here failed; the next sync tries again, or run `crystalline domain \
                         rename {old} {new} --local`"
                    );
                    self.adoption_failures
                        .lock()
                        .unwrap()
                        .insert(old.clone(), e.to_string());
                    report.push(json!({
                        "domain": old,
                        "canonical": new,
                        "action": "failed",
                        "error": e.to_string(),
                    }));
                }
            }
        }
        Ok(Value::Array(report))
    }

    /// [`Engine::adopt_domain_names`] for a caller whose own work has landed
    /// already: a failure is logged, naming `after` (what just ran), and
    /// answered as an empty list.
    pub async fn adopt_domain_names_after(&self, after: &str) -> Value {
        match self.adopt_domain_names().await {
            Ok(report) => report,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "lining the domain names up with their MANIFESTs after {after} failed; the \
                     next sync tries again"
                );
                json!([])
            }
        }
    }

    /// What a daemon or an embedded server runs once its first sync has
    /// returned. The names are lined up with the MANIFESTs first (the one-time
    /// catch-up for a configuration from before 0.20.0, and any `domain_name`
    /// a MANIFEST gained while nothing ran), then every domain gets one
    /// resolve pass for what no sync from disk reaches: an index upgrade may
    /// have unbound references in a virtual domain or a draft. File domains'
    /// base rows are skipped, since the sync that just ran bound them, and
    /// the pass runs domain by domain so requests wait for one domain at most.
    pub async fn settle_after_initial_sync(&self) {
        self.adopt_domain_names_after("the initial sync").await;
        self.resolve_pending_after_startup().await;
    }

    /// Hold the next adoption after its plan, before its renames, until the
    /// answered hold is released.
    #[cfg(any(test, feature = "testing"))]
    pub fn hold_next_adoption_after_plan(&self) -> Arc<crate::rename::RenameHold> {
        let hold = Arc::new(crate::rename::RenameHold::default());
        *self.adoption_hold.lock().unwrap() = Some(hold.clone());
        hold
    }

    /// How many times [`Engine::adopt_domain_names`] ran on this engine.
    #[cfg(any(test, feature = "testing"))]
    pub fn adoptions_run(&self) -> u64 {
        self.adoptions.load(AtomicOrdering::Relaxed)
    }

    /// Whether the adoption leaves `name` alone: a domain an environment
    /// variable defines, or one a rename holds or a journal names.
    fn adoption_skips(&self, name: &str, busy: &HashSet<String>) -> bool {
        busy.contains(name) || self.overlay.env_domain(name).is_some()
    }

    /// The configuration file an adoption edits: the file as it stands, like
    /// [`Engine::fresh_file_config`], except that a file which is there and
    /// does not parse is an error instead of a reason to fall back to the
    /// snapshot. An adoption is bookkeeping nobody asked for, so it must not
    /// write the snapshot over a file somebody is editing by hand, or over a
    /// broken one `crystalline status` is about to report. The next sync
    /// tries again.
    fn adoption_file_config(&self, snapshot: &GlobalConfig) -> Result<GlobalConfig> {
        match self.config_file_path() {
            Some(path) if path.is_file() => overlay::load_file(&path).map_err(|e| {
                EngineError::Invalid(format!(
                    "{e}; the configuration file is left as it is until it parses again"
                ))
            }),
            _ => Ok(snapshot.clone()),
        }
    }

    /// The catch-up half: infer and persist the name origin of every entry
    /// in the configuration file that has none. Answers each entry it
    /// inferred, in file order.
    fn infer_missing_name_origins(
        &self,
        busy: &HashSet<String>,
    ) -> Result<Vec<(String, NameOrigin)>> {
        let mut file_guard = self.file_config.write().unwrap();
        let mut file = self.adoption_file_config(&file_guard)?;
        let mut inferred = Vec::new();
        for (name, entry) in file.domains.iter_mut() {
            if entry.name_origin.is_some() || self.adoption_skips(name, busy) {
                continue;
            }
            let origin = infer_name_origin(name, entry, false);
            entry.name_origin = Some(origin);
            inferred.push((name.clone(), origin));
        }
        if !inferred.is_empty() {
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }
        Ok(inferred)
    }

    /// The adoption half: compare each domain's canonical name in `table`
    /// with the `canonical_seen` its entry records, record aliases and the
    /// new `canonical_seen`, and answer the renames to run once
    /// `domain_admin` is released.
    fn record_canonicals(
        &self,
        table: &NameTable,
        busy: &HashSet<String>,
        report: &mut Vec<Value>,
    ) -> Result<Vec<(String, String)>> {
        let mut planned = Vec::new();
        let mut file_guard = self.file_config.write().unwrap();
        let mut file = self.adoption_file_config(&file_guard)?;
        let mut changed = false;
        for (name, entry) in file.domains.iter_mut() {
            if self.adoption_skips(name, busy) {
                continue;
            }
            let canonical = table.canonical(name).unwrap_or(name).to_string();
            if entry.canonical_seen.as_deref() == Some(canonical.as_str()) {
                continue;
            }
            if let Some(previous) = entry.canonical_seen.clone()
                && previous != *name
                && previous != canonical
                && !entry.aliases.contains(&previous)
            {
                entry.aliases.push(previous.clone());
                changed = true;
                report.push(json!({ "domain": name, "action": "alias", "alias": previous }));
            }
            let reaches_it = table.resolve(&canonical) == Some(name.as_str());
            if canonical != *name && reaches_it && entry.name_origin == Some(NameOrigin::Derived) {
                if busy.is_empty() {
                    // The rename's config step records `canonical_seen`.
                    planned.push((name.clone(), canonical));
                } else {
                    tracing::info!(
                        "renaming domain '{name}' to '{canonical}', the name its MANIFEST \
                         declares, waits until the pending rename is finished"
                    );
                    report.push(json!({
                        "domain": name,
                        "canonical": canonical,
                        "action": "deferred",
                    }));
                }
                continue;
            }
            if canonical != *name && !reaches_it {
                // Shadowed (another domain answers to the name here) or
                // contested (more than one declares it, which the table
                // warns about with its own fix). A normal state, so no
                // warning of its own. A derived domain does not record the
                // name as seen: once the other claimant goes away, the next
                // pass finds the name free and adopts it.
                let held_by = table.resolve(&canonical);
                report.push(json!({
                    "domain": name,
                    "canonical": canonical,
                    "action": if held_by.is_some() { "shadowed" } else { "contested" },
                    "held_by": held_by,
                }));
                if entry.name_origin == Some(NameOrigin::Derived) {
                    continue;
                }
            } else if canonical != *name {
                report.push(json!({
                    "domain": name,
                    "canonical": canonical,
                    "action": "kept",
                }));
            }
            entry.canonical_seen = Some(canonical);
            changed = true;
        }
        if changed {
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }
        Ok(planned)
    }
}
