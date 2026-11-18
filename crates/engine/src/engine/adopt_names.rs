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

use crystalline_core::config::{RenameAnswer, RenameQuestion};
use crystalline_core::names::NameTable;

use super::*;

impl Engine {
    /// The rename question this instance holds open for `domain` (its local
    /// name): an explicit domain whose MANIFEST `domain_name` a sync saw
    /// change, and that nobody has finished or kept yet. `None` for a
    /// question already answered and for a domain with none.
    pub fn rename_question(&self, domain: &str) -> Option<RenameQuestion> {
        let config = self.config.read().unwrap();
        config
            .domains
            .get(domain)?
            .rename_question
            .clone()
            .filter(|question| question.answered.is_none())
    }

    /// Whether any domain here has an open rename question: the cheap check
    /// a tool answer makes before it looks for a domain to tell about.
    pub fn any_rename_question(&self) -> bool {
        self.config.read().unwrap().domains.values().any(|entry| {
            entry
                .rename_question
                .as_ref()
                .is_some_and(|question| question.answered.is_none())
        })
    }

    /// The rename guidance line for a tool answer about `domain` (any name
    /// it answers to), when that domain has an open rename question, `scope`
    /// may rename the domain (the line asks the person to act, and only
    /// those who may can) and the session `session` (a stable key for
    /// whoever is asking) has not been told about it yet. Marks the pair as
    /// told, so the next answer in the same session carries nothing.
    pub async fn rename_guidance_once(
        &self,
        session: &str,
        domain: &str,
        scope: &crate::scope::Scope,
    ) -> Option<String> {
        let registered = self.config.read().unwrap().domains.contains_key(domain);
        let local = if registered {
            domain.to_string()
        } else {
            self.name_table_now().await.resolve(domain)?.to_string()
        };
        let question = self.rename_question(&local)?;
        if !self.may_rename(&local, scope).await {
            return None;
        }
        let fresh = self
            .rename_nudged
            .lock()
            .unwrap()
            .insert((session.to_string(), local));
        fresh.then(|| {
            RENAME_PENDING_GUIDANCE
                .replace("{from}", &question.from)
                .replace("{to}", &question.to)
        })
    }

    /// Forget which sessions were told about `domain`'s rename question, once
    /// it is answered, dropped or moved with a rename, so a new question on
    /// the same domain is told again.
    pub(super) fn forget_rename_nudges(&self, domain: &str) {
        self.rename_nudged
            .lock()
            .unwrap()
            .retain(|(_, told)| told != domain);
    }

    /// Answer `domain`'s open rename question with `answer`, when its new
    /// name is still `to` (any, when `None`): a question a newer MANIFEST
    /// edit moved on is about another name and stays open. Written through
    /// the configuration file as the adoption writes it. Answers whether a
    /// question was answered.
    pub(super) fn answer_rename_question(
        &self,
        domain: &str,
        answer: RenameAnswer,
        to: Option<&str>,
    ) -> Result<bool> {
        let answered = {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard)?;
            self.shadow_base_entry(&mut file, domain);
            let question = file
                .domains
                .get_mut(domain)
                .and_then(|entry| entry.rename_question.as_mut())
                .filter(|question| {
                    question.answered.is_none() && to.is_none_or(|to| question.to == to)
                });
            match question {
                Some(question) => {
                    question.answered = Some(answer);
                    self.commit_config(&mut file_guard, file)?;
                    true
                }
                None => false,
            }
        };
        self.forget_rename_nudges(domain);
        Ok(answered)
    }

    /// Record that a MANIFEST save which declares `to` keeps the links as
    /// they are ("Save without renaming"): a question already answered
    /// "keep", so the adoption after the next sync asks nothing. An open
    /// question keeps the name it started from; otherwise the question
    /// starts from the name this machine last saw. A domain the
    /// configuration file does not hold (an environment variable defines
    /// it) records nothing.
    pub(super) fn record_kept_rename(&self, domain: &str, to: &str) -> Result<()> {
        {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard)?;
            self.shadow_base_entry(&mut file, domain);
            let Some(entry) = file.domains.get_mut(domain) else {
                return Ok(());
            };
            let from = match &entry.rename_question {
                Some(question) if question.answered.is_none() => question.from.clone(),
                _ => entry
                    .canonical_seen
                    .clone()
                    .unwrap_or_else(|| domain.to_string()),
            };
            if from == to {
                // Back to the name the question started from: the adoption
                // drops the question.
                return Ok(());
            }
            entry.rename_question = Some(RenameQuestion {
                from,
                to: to.to_string(),
                seen_at: time::now().rfc3339_secs_z(),
                answered: Some(RenameAnswer::Keep),
            });
            self.commit_config(&mut file_guard, file)?;
        }
        self.forget_rename_nudges(domain);
        Ok(())
    }

    /// Runs after a sync pass. First the one-time catch-up: every entry with
    /// `name_origin: None` gets [`infer_name_origin`], persisted, and the
    /// MANIFEST write-back where [`needs_manifest_write_back`] says so (never
    /// for a team domain). Then, per domain whose canonical name differs from
    /// `canonical_seen`: the previous canonical (when it differs from the
    /// local name) becomes an alias; a derived domain whose new canonical
    /// reaches only it here is renamed to it with
    /// [`Engine::rename_domain_local`] recorded as derived; an explicit one
    /// keeps its name, and when the name it had seen changed, a rename
    /// question is recorded for a person ([`Engine::rename_question`]); a
    /// name another domain holds here is left shadowed.
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
            // A respell a failed finish left behind for a question that has
            // since changed is dropped first, so it holds nothing back.
            self.drop_stale_respell_journal().await;
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
            self.record_canonicals(&table, &busy, &mut report, None)?
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

    /// After a pull from a domain's origin: close every open rename question
    /// whose links match once the pull landed, because a teammate finished
    /// the rename and the respelled files arrived with it, the MANIFEST
    /// change in the same pull or an earlier one. `clean_before` is
    /// [`Engine::rename_questions_already_clean`] taken before the pull, and
    /// those questions stay open: their links were respelled here, not by the
    /// pull. Only a pull does this. A rename question is a person's to
    /// answer, so links respelled here (by a person's edit, an agent's edit
    /// or an evolve run) never close it.
    pub async fn settle_rename_questions_after_pull(&self, clean_before: &HashSet<String>) {
        if self.read_only || self.config_path.is_none() || !self.any_rename_question() {
            return;
        }
        let busy: HashSet<String> = self.names_being_renamed().into_iter().collect();
        if let Err(e) = self
            .close_settled_rename_questions(&busy, clean_before)
            .await
        {
            tracing::warn!("checking whether a rename question is settled failed: {e}");
        }
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

    /// After a write landed `domain`'s MANIFEST: record what its new
    /// `domain_name` means for this machine in that same write, as the next
    /// adoption would (the alias, `canonical_seen`, and the rename question
    /// for a domain named on purpose), so no later write respells the old
    /// name before the question exists. A domain whose name is derived is
    /// renamed by the next adoption, as before; nothing is planned here.
    ///
    /// Never waits: the write that calls this may hold a write ticket a
    /// running adoption's rename waits for, so while an adoption or a domain
    /// change is under way this leaves it to that one, and the write funnel
    /// holds the old name meanwhile (see [`Engine::rename_held_spellings`]).
    pub(super) async fn ask_rename_after_manifest_write(&self, domain: &str) {
        if self.read_only || self.config_path.is_none() || !self.holds_state_dir() {
            return;
        }
        if !matches!(self.off_machine().await, Ok(None)) {
            return;
        }
        let Ok(_adopting) = self.adoption_lock.try_lock() else {
            return;
        };
        let Ok(_admin) = self.domain_admin.try_lock() else {
            return;
        };
        let busy: HashSet<String> = self.names_being_renamed().into_iter().collect();
        let table = self.name_table_now().await;
        let mut report = Vec::new();
        if let Err(e) = self.record_canonicals(&table, &busy, &mut report, Some(domain)) {
            tracing::warn!(
                domain,
                "recording the name the MANIFEST of '{domain}' declares failed; the next sync \
                 tries again: {e}"
            );
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

    /// The configuration file an adoption edits: the file as it stands,
    /// through [`Engine::fresh_file_config`], so a file which is there and
    /// does not parse is an error. An adoption is bookkeeping nobody asked
    /// for, so it must not write the snapshot over a file somebody is editing
    /// by hand, or over a broken one `crystalline status` is about to report.
    /// The next sync tries again.
    fn adoption_file_config(&self, snapshot: &GlobalConfig) -> Result<GlobalConfig> {
        self.fresh_file_config(snapshot)
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
            self.commit_config(&mut file_guard, file)?;
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
        only: Option<&str>,
    ) -> Result<Vec<(String, String)>> {
        let mut planned = Vec::new();
        let mut file_guard = self.file_config.write().unwrap();
        let mut file = self.adoption_file_config(&file_guard)?;
        let mut changed = false;
        for (name, entry) in file.domains.iter_mut() {
            if self.adoption_skips(name, busy) || only.is_some_and(|only| only != name) {
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
            // The name changed back to the one an open question started
            // from: nothing is left to ask.
            if entry
                .rename_question
                .as_ref()
                .is_some_and(|question| question.from == canonical)
            {
                entry.rename_question = None;
                self.forget_rename_nudges(name);
                changed = true;
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
                // A name seen before, changed by hand in the MANIFEST: ask
                // a person whether to finish the rename. The first sight of
                // a declared name is no rename, so it asks nothing.
                if let Some(previous) = entry.canonical_seen.clone() {
                    ask_rename(entry, previous, &canonical);
                }
            }
            entry.canonical_seen = Some(canonical);
            changed = true;
        }
        if changed {
            self.commit_config(&mut file_guard, file)?;
        }
        Ok(planned)
    }
}

/// Record that `entry`'s canonical name moved from `previous` to `canonical`
/// with the local name kept, as a question for a person.
///
/// A question already answered for this same new name (the MANIFEST editor
/// records "keep" before it saves) is left as it is. An open question keeps
/// the name it started from and takes the newest name, so a second edit
/// before an answer asks about the whole change once. Anything else starts
/// a new question.
fn ask_rename(entry: &mut DomainEntry, previous: String, canonical: &str) {
    let seen_at = time::now().rfc3339_secs_z();
    match &mut entry.rename_question {
        Some(question) if question.to == canonical && question.answered.is_some() => {}
        Some(question) if question.answered.is_none() => {
            question.to = canonical.to_string();
            question.seen_at = seen_at;
        }
        slot => {
            *slot = Some(RenameQuestion {
                from: previous,
                to: canonical.to_string(),
                seen_at,
                answered: None,
            });
        }
    }
}
