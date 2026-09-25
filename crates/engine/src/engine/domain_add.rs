use super::*;

impl Engine {
    // --- domain add (local and virtual) ---------------------------------------

    /// Create or adopt a local file domain and bring it into the index, the
    /// non-GitHub half of `add_domain`. Resolves the on-disk root (an explicit
    /// `folder`, otherwise `<domains_root>/<name>`), creates it, scaffolds a
    /// `MANIFEST.md` when the folder does not already carry one (so a fresh
    /// folder becomes a domain and an existing one is adopted in place, its
    /// files untouched), registers it in the global config and syncs.
    ///
    /// At least one of `name`/`folder` is required. Without `name`, the name is
    /// derived from the folder's basename (auto-suffixed on collision); with an
    /// explicit `name`, a different-folder or virtual clash is refused. Pointing
    /// at a folder already registered adopts it idempotently. Refuses on a
    /// read-only instance; no `github.enabled` gate, so it works on a fresh
    /// install. Returns `{ domain, root, kind, manifest_created, adopted, sync }`.
    pub async fn domain_add_local(
        &self,
        name: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if name.is_none() && folder.is_none() {
            return Err(EngineError::Invalid(
                "provide a domain name, a folder, or both".to_string(),
            ));
        }

        // Against every registration this instance has, not the startup
        // snapshot alone: a domain the CLI registered after this engine
        // started is in the file and would otherwise be re-registered over,
        // or handed a name derived as if it were free.
        let mut cfg = self.config();
        cfg.domains = self.registered_domain_entries();

        if let Some(n) = name {
            // An env-defined domain of this name is owned by its variable.
            if let Some(env) = self.overlay.env_domain(n) {
                return Err(EngineError::Conflict(format!(
                    "domain '{n}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                    env.var
                )));
            }
            // A name nothing holds is a new registration, and a new name is
            // checked before anything touches the disk: the default folder is
            // `<domains_root>/<name>`, so `../up` would otherwise be created
            // beside the root before the refusal. A registered name is never
            // re-checked here; the decision below adopts or refuses it.
            if !cfg.domains.contains_key(n) {
                validate_domain_name(n).map_err(EngineError::Invalid)?;
            }
        }

        // An explicit folder wins; otherwise a named domain lands under the
        // configured root at `<root>/<name>`.
        let root = match folder {
            Some(f) => crystalline_core::config::expand_tilde(f),
            None => {
                let domains_root = self.config.read().unwrap().domains_root();
                origin::default_domain_folder(&domains_root, name.expect("checked above"))
            }
        };
        std::fs::create_dir_all(&root).map_err(|e| {
            EngineError::Internal(format!("creating domain directory {}: {e}", root.display()))
        })?;
        let canonical = std::fs::canonicalize(&root)
            .map_err(|e| EngineError::Internal(format!("resolving {}: {e}", root.display())))?;

        // Decide the domain name and whether we adopt an existing registration.
        let (domain_name, adopted) = match name {
            Some(n) => match decide_registration(
                n,
                cfg.domains.get(n),
                &RegistrationRequest::File { root: &canonical },
            ) {
                Registration::Adopt => (n.to_string(), true),
                Registration::Register => (n.to_string(), false),
                Registration::Conflict(msg) => return Err(EngineError::Conflict(msg)),
            },
            // No name: adopt an existing registration of this exact folder,
            // else derive a fresh unique name from the folder basename.
            None => match existing_file_domain_at(&canonical, &cfg) {
                Some(existing) => (existing.to_string(), true),
                None => (unique_domain_name(&canonical, &cfg), false),
            },
        };

        // Create-or-adopt: scaffold a MANIFEST.md only when the folder lacks one.
        let manifest = canonical.join("MANIFEST.md");
        let manifest_created = if manifest.exists() {
            false
        } else {
            let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
            std::fs::write(
                &manifest,
                crystalline_core::manifest_template(&domain_name, &today),
            )
            .map_err(|e| EngineError::Internal(format!("writing {}: {e}", manifest.display())))?;
            true
        };

        // Register a genuinely new domain, mirroring `origin_add`'s write-lock-
        // first file-then-effective pattern so a concurrent read never observes a
        // half-applied config and no env value bakes into the saved file. An
        // adopted registration is already in the config.
        if !adopted {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard);
            file.domains
                .insert(domain_name.clone(), DomainEntry::file(canonical.clone()));
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }

        // Tell a running daemon's watcher to watch the new root; an adopted
        // domain is already watched. This engine's own sync runs regardless.
        if !adopted && let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Add(domain_name.clone(), canonical.clone()));
        }

        let sync = self.sync(Some(&domain_name)).await?;
        if !self.request_embed()
            && let Err(e) = self.embed_pending().await
        {
            tracing::warn!("embedding after creating '{domain_name}' failed: {e}");
        }

        Ok(json!({
            "domain": domain_name,
            "root": canonical.display().to_string(),
            "kind": "file",
            "manifest_created": manifest_created,
            "adopted": adopted,
            "sync": sync,
        }))
    }

    /// Create a virtual (database-backed) domain, the DB half of `add_domain`.
    /// Registers a `DomainEntry::virtual_domain()` in the global config, then
    /// scaffolds a `MANIFEST.md` engram into the database (a no-op when one is
    /// already present). Re-creating an existing virtual domain is idempotent; a
    /// file domain of the same name is refused. No filesystem root, no watcher,
    /// no sync. Refuses on a read-only instance; no `github.enabled` gate.
    /// Returns `{ domain, kind, manifest_created, registered }`.
    pub async fn domain_add_virtual(&self, name: &str) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let Some(env) = self.overlay.env_domain(name) {
            return Err(EngineError::Conflict(format!(
                "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                env.var
            )));
        }
        // The same source this path always read (this instance's own
        // config), not `registered_domain_entries()`: a discovered-only entry
        // must not be adopted here without being written to the config.
        let existing = self.config.read().unwrap().domains.get(name).cloned();
        let is_new =
            match decide_registration(name, existing.as_ref(), &RegistrationRequest::Virtual) {
                Registration::Adopt => false,
                Registration::Register => {
                    validate_domain_name(name).map_err(EngineError::Invalid)?;
                    true
                }
                Registration::Conflict(msg) => return Err(EngineError::Conflict(msg)),
            };

        // Register before scaffolding: `scaffold_virtual_manifest` reads the
        // content source, which requires the domain to already be registered.
        if is_new {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard);
            file.domains
                .insert(name.to_string(), DomainEntry::virtual_domain());
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
        }

        let today = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let scaffold = self
            .scaffold_virtual_manifest(name, &crystalline_core::manifest_template(name, &today))
            .await?;
        let manifest_created = scaffold
            .get("created")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        Ok(json!({
            "domain": name,
            "kind": "virtual",
            "manifest_created": manifest_created,
            "registered": is_new,
        }))
    }

    /// What a caller is told when they may see a domain and may not end it.
    ///
    /// One sentence for both surfaces, naming who can rather than saying only
    /// that the caller cannot: an instance admin always, and for a private
    /// domain its owner. The owner is never named - who owns a domain is a
    /// membership fact, and a refusal is not the place to hand it out.
    fn removal_refusal(name: &str) -> EngineError {
        EngineError::Forbidden(format!(
            "unregistering domain '{name}' is for an instance admin, or for the owner of a \
             private domain; ask an admin to remove it"
        ))
    }

    /// What a caller is told when a removal would delete a virtual domain's
    /// engrams and nothing said the loss was intended.
    ///
    /// One sentence for every surface, naming the flag in each of their
    /// spellings, because the rule is one rule and the surfaces are three. It
    /// speaks only about a virtual domain: a file or team domain's markdown is
    /// never touched by a removal, so there is nothing there to confirm.
    /// `held` is `None` when the count could not be read at all, which is a
    /// refusal in its own right: an unreadable index is not an empty one, and
    /// the one branch that decides whether knowledge is deleted must not read
    /// a failure as "there was nothing there".
    fn purge_refusal(name: &str, held: Option<i64>) -> EngineError {
        let holding = match held {
            Some(1) => "holding 1 engram".to_string(),
            Some(n) => format!("holding {n} engrams"),
            None => "whose engrams could not be counted, because the index could not be read"
                .to_string(),
        };
        EngineError::ConfirmationRequired(format!(
            "domain '{name}' is a virtual domain {holding}: its knowledge lives in the \
             database, so unregistering it DELETES those engrams and leaves no files to \
             re-adopt. Export or share what is worth keeping first, then repeat the removal \
             with purge set - 'purge: true' over MCP, '?purge=true' on the JSON API, '--purge' \
             at the command line. A file or team domain needs no purge, since a removal never \
             touches its files."
        ))
    }

    /// The kind a removal speaks about: three, where the registry itself knows
    /// two.
    ///
    /// A team domain is a file domain carrying an origin, and for every other
    /// purpose that distinction is the origin's business. It matters here
    /// because the recovery differs: re-adding the FOLDER of a team domain
    /// registers a plain local one and drops the origin, the base commit and
    /// the team connection, so a confirmation that offered that recovery would
    /// be telling somebody the wrong thing on the way to a destructive act.
    fn removal_kind(entry: &DomainEntry) -> &'static str {
        if entry.is_virtual() {
            "virtual"
        } else if entry.origin.is_some() {
            "team"
        } else {
            "file"
        }
    }

    /// How many engrams the index holds for `name`, refusing a virtual domain
    /// that holds knowledge unless `purge` says the loss was intended.
    ///
    /// The count and the refusal come out of one read on purpose: they are the
    /// same fact asked twice, and a preview whose count disagreed with the
    /// refusal that follows it would be worse than either alone.
    ///
    /// **The KIND is the primary key of this decision, and it comes from the
    /// config entry, which cannot fail to be read.** Only a virtual domain can
    /// lose knowledge to a removal, so a file or team domain never reaches the
    /// refusal at all and its count is a display value: an unreadable index
    /// costs it nothing more than an absent number. That ordering is what keeps
    /// this gate from failing open, and it is why the kind is tested before the
    /// count rather than after it.
    ///
    /// **For a virtual domain, a count that cannot be read is a refusal.** An
    /// error from the index is not the same fact as an empty index, and reading
    /// it as one would delete somebody's only copy of their knowledge on the
    /// strength of a failed query - `domain_stats` is an aggregate sweep over
    /// every domain and can time out on a database whose targeted delete would
    /// have succeeded, so "the clear would probably have failed too" is not an
    /// argument a confirmation gate may rest on. With `purge` already set there
    /// is nothing left to confirm, so the error costs the caller only the
    /// number in the receipt.
    ///
    /// A `None` count for a virtual domain that COULD be read is a domain the
    /// index has no row for, which is one nothing ever synced; it is not
    /// knowledge to protect, since there is nothing recorded to lose. In
    /// practice that case is unreachable for a domain a caller could be looking
    /// at, because `domain_add_virtual` scaffolds a MANIFEST engram into the
    /// database from the moment the domain exists.
    async fn removal_engrams(
        &self,
        name: &str,
        entry: &DomainEntry,
        purge: bool,
    ) -> Result<RemovalCount> {
        let store = self.store.lock().await;
        let stats = store.domain_stats().await;
        drop(store);
        let counted = match &stats {
            Ok(rows) => match rows.iter().find(|d| d.name == name) {
                Some(row) => RemovalCount::Known(row.engrams),
                None => RemovalCount::Absent,
            },
            Err(_) => RemovalCount::Unreadable,
        };
        // The kind first: a file or team domain loses no knowledge here, so a
        // read that failed only costs it the number.
        if !entry.is_virtual() {
            return Ok(counted);
        }
        match (counted, purge) {
            (RemovalCount::Unreadable, false) => return Err(Engine::purge_refusal(name, None)),
            (RemovalCount::Unreadable, true) => {
                // Already confirmed: the removal proceeds. Nothing is lost
                // from the removal's own receipt, which never carried a count -
                // the number belongs to the PREVIEW, and a preview that could
                // not read it says so with `engrams_unknown`. What is lost is
                // the chance to have shown the figure before the decision, and
                // that decision was already taken. Logged rather than swallowed
                // silently, because an index that cannot be swept is worth
                // knowing about.
                tracing::warn!(
                    domain = name,
                    error = stats
                        .as_ref()
                        .err()
                        .map(|e| format!("{e:#}"))
                        .unwrap_or_default(),
                    "the engram count for '{name}' could not be read; the confirmed removal \
                     proceeds without it"
                );
            }
            (RemovalCount::Known(n), false) if n > 0 => {
                return Err(Engine::purge_refusal(name, Some(n)));
            }
            _ => {}
        }
        Ok(counted)
    }

    /// Every actor's draft count in one domain, as the index rows have them,
    /// or `None` when the index could not be asked at all.
    ///
    /// **The rows are the authority and the journal is their mirror.** A draft
    /// row can land while its journal entry fails - `write_overlay_entry`
    /// reports that as a `draft_warning` and keeps the row - so a count read
    /// from the mirror would quietly drop exactly those, in front of the calls
    /// that end somebody's unshared work for good. This is the one place a
    /// count is derived, so the removal's question, the status surfaces and the
    /// drafts route cannot answer "who is drafting here" three different ways.
    ///
    /// **A deletion counts as an entry.** A draft that takes a file away is
    /// unshared work exactly as a draft that writes one is, and a count that
    /// left it out would tell somebody they hold nothing while a removal is
    /// still theirs to lose.
    ///
    /// A pure read, and deliberately: [`Store::domain_id`] is the read-only way
    /// to a domain id, so a status call on a read-only instance can ask this
    /// without registering anything. A domain the index holds no row for has no
    /// drafts in it either - a registration nothing has synced yet - and that
    /// is an honest empty rather than an unknown.
    pub(crate) async fn overlay_counts_by_actor(&self, name: &str) -> Option<Vec<(String, u64)>> {
        let mut held: BTreeMap<String, u64> = BTreeMap::new();
        {
            let store = self.store.lock().await;
            let id = match store.domain_id(name).await {
                Ok(Some(id)) => Some(id),
                // A domain this index holds no row for has no drafted rows in
                // it - a registration nothing has synced yet - and that is an
                // honest empty rather than an unknown. It can still hold FILES,
                // so this is a `None` to skip the row half with and never an
                // early return.
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(
                        domain = name,
                        error = format!("{e:#}"),
                        "the index could not say which domain '{name}' is, so nobody's drafts in \
                         it can be counted"
                    );
                    return None;
                }
            };
            if let Some(id) = id {
                match store.overlay_counts(id).await {
                    Ok(counts) => held.extend(counts),
                    Err(e) => {
                        tracing::warn!(
                            domain = name,
                            error = format!("{e:#}"),
                            "the drafts held in domain '{name}' could not be counted"
                        );
                        return None;
                    }
                }
            }
        }
        if let Some(files) = self.overlay_file_counts(name)? {
            for (actor, entries) in files {
                *held.entry(actor).or_default() += entries;
            }
        }
        Some(held.into_iter().collect())
    }

    /// Every actor's files-overlay count in one domain, `None` when it could
    /// not be read at all, and `Some(None)` for a domain that cannot hold one.
    ///
    /// **Only a domain that reviews changes is walked**, which is what keeps a
    /// domain taking changes directly byte for byte the answer it was: nothing
    /// writes a files overlay outside review mode, so a walk there could only
    /// ever report zero - and a state directory somebody has damaged would turn
    /// every direct domain's count into an unknown over a tree that holds
    /// nothing. It also keeps a directory walk off the listing of a machine
    /// whose domains all take changes directly.
    ///
    /// A state directory that cannot be resolved is unknown rather than empty:
    /// this tree is the only place a draft file's bytes exist, so "nowhere to
    /// look" is not "nothing there".
    ///
    /// The one corner where a direct domain is not byte for byte what it was:
    /// it is not this function (gated, so a direct domain never reaches the
    /// tree) but its ungated listing twin, which flags every actor of a direct
    /// domain whose `<state>/overlays/<domain>` is damaged. What that changes
    /// is one case and only one: a **mid-fold recovery** there - a domain the
    /// key came off mid-verb, still holding rows or visible files - refuses
    /// where it used to fold half. That is the more correct answer, and it is
    /// named here rather than discovered. A plain statement that an
    /// already-direct domain takes changes directly is untouched:
    /// [`Engine::refuse_unlistable_files`] applies only where the domain
    /// reviews changes or a draft file is actually visible.
    pub(super) fn overlay_file_counts(&self, name: &str) -> Option<Option<BTreeMap<String, u64>>> {
        if !self.reviews_changes(name) {
            return Some(None);
        }
        let Ok(state_dir) = self.journal_state_dir() else {
            tracing::warn!(
                domain = name,
                "the files overlay of '{name}' could not be located, so what anybody has \
                 drafted there is unknown"
            );
            return None;
        };
        let held = crate::overlay_files::by_actor(&state_dir, name);
        if held.unlistable().is_some() {
            return None;
        }
        Some(Some(held.counts()))
    }

    /// Sweep a domain's overlay journal as part of ending it, answering with
    /// how many mirrored drafts went.
    ///
    /// Best effort, like [`Engine::forget_domain_records`] beside it and for
    /// the same reason: by the time this runs the rows are already cleared, and
    /// answering with an error would tell the caller their removal did not
    /// happen when it did. A journal that could not be removed is logged, and
    /// nothing restores from it either way - [`Engine::restore_overlays`]
    /// refuses a domain nobody registers.
    pub(super) async fn sweep_domain_journal(&self, name: &str) -> u64 {
        let state_dir = match self.journal_state_dir() {
            Ok(dir) => dir,
            Err(e) => {
                tracing::warn!(
                    domain = name,
                    error = format!("{e:#}"),
                    "the overlay journal for '{name}' could not be located and was not swept"
                );
                return 0;
            }
        };
        match crate::overlay_journal::journal_remove_domain(&state_dir, name) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(
                    domain = name,
                    error = format!("{e:#}"),
                    "the overlay journal for '{name}' could not be swept; the drafts it \
                     mirrors are unreachable (nothing restores into an unregistered domain) \
                     but the folder is left on disk"
                );
                0
            }
        }
    }

    /// The one refusal for a write that would land in the folder of a domain
    /// that reviews changes, wherever it comes from.
    ///
    /// Review mode rests on one rule: the folder the team shares changes only
    /// by a pull, and everything else joins its author's own draft. Two verbs
    /// can still reach that folder sideways, and neither has an answer for
    /// "whose draft is this" - a cross-domain move builds its view from the
    /// SOURCE domain, and an archive import is an admin handing a domain a zip
    /// that belongs to nobody in particular. So both are refused here, in one
    /// sentence, which teaches the rule and names the way in: write it there,
    /// as a draft, and share it.
    ///
    /// `what` is what the caller was doing, so the refusal reads as an answer
    /// to their own request rather than as a fact about the domain.
    #[doc(hidden)]
    pub fn refuse_write_into_reviewed_folder(&self, domain: &str, what: &str) -> Result<()> {
        if !self.reviews_changes(domain) {
            return Ok(());
        }
        Err(EngineError::Refused(format!(
            "domain '{domain}' reviews changes before they land, so its folder changes only \
             through a reviewed proposal and {what} has nowhere to go: a draft belongs to one \
             domain's overlay. Write it there - the write joins your own draft - and share it, \
             or take review mode off first"
        )))
    }

    /// Whether `name` is a domain the environment defines, as the conflict both
    /// surfaces answer with.
    ///
    /// An environment-defined domain is immune to unregistration: the variable
    /// is its source of truth, so no version of this request would succeed and
    /// the way out is to unset the variable. Spelled once here so a preview and
    /// the removal itself cannot word it differently.
    ///
    /// [`Engine::set_review_mode`] answers with it too, including for the call
    /// that takes review mode OFF, and that is deliberate rather than
    /// over-broad: `CRYSTALLINE_DOMAIN_<NAME>_REVIEW` rides on an env-defined
    /// domain, whose review key is not in the config file this would write, and
    /// the environment would put the mode straight back on the effective config
    /// while the fold was still running. So the exit for such a domain is the
    /// variable first and the verb second: unset it, restart, and then
    /// `crystalline domain review <name> direct` (or its REST twin) folds or
    /// discards the drafts that are left - which works, because leaving review
    /// mode never asks whether the domain is in it, only what rows it holds.
    /// Pinned by `a_domain_that_reviews_nothing_can_still_fold_the_drafts_it_holds`.
    pub(super) fn env_domain_conflict(&self, name: &str) -> Option<EngineError> {
        self.overlay.env_domain(name).map(|env| {
            EngineError::Conflict(format!(
                "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                env.var
            ))
        })
    }

    /// The one gate on ending a domain, for every surface.
    ///
    /// Two steps, in this order and for the reason the write gate states:
    ///
    /// 1. **A domain this caller may not see is the not-found.** Decided first,
    ///    so a stranger naming a private domain learns exactly what a stranger
    ///    naming a domain nobody registered learns. A permission refusal here
    ///    would be an existence oracle.
    /// 2. **Then the right, which must be [`DomainRight::Own`].** That is the
    ///    whole of Jordi's rule, and it falls out of the ladder rather than
    ///    being restated: an instance admin owns every domain, a private
    ///    domain's owner owns theirs, and nobody else ever reaches `Own` - a
    ///    manager stops at `Manage`, and on a shared domain the best a
    ///    non-admin gets is `Write`. So "owner-of-private or admin, shared
    ///    domains admins only" is one comparison.
    ///
    /// [`Scope::Anonymous`] is refused by an arm of its own rather than by the
    /// fold. The fold would refuse it too, but only because a resolver happens
    /// to be installed; nobody in particular does not end a domain on an
    /// instance that never made anyone authenticate either, and that is a rule
    /// rather than a consequence.
    ///
    /// [`DomainRight::Own`]: crate::scope::DomainRight::Own
    /// [`Scope::Anonymous`]: crate::scope::Scope::Anonymous
    pub async fn require_domain_owner(
        &self,
        name: &str,
        scope: &crate::scope::Scope,
    ) -> Result<()> {
        self.require_domain_owner_refusing(name, scope, Engine::removal_refusal(name))
            .await
    }

    /// The same gate, worded for a call that is about the domain rather than
    /// about ending it.
    ///
    /// One ladder, two sentences: the rule about who holds a domain is the same
    /// whether they are ending it or reading who is drafting in it, and a
    /// second copy of the ladder is how the two would drift apart. `refusal` is
    /// built by the caller so its own verb is the one named in the answer.
    pub(super) async fn require_domain_owner_refusing(
        &self,
        name: &str,
        scope: &crate::scope::Scope,
        refusal: EngineError,
    ) -> Result<()> {
        self.require_domain(name, scope).await?;
        if matches!(scope, crate::scope::Scope::Anonymous) {
            return Err(refusal);
        }
        if self.domain_right(scope, name).await? < crate::scope::DomainRight::Own {
            return Err(refusal);
        }
        Ok(())
    }

    /// Who is drafting in one domain and how much, for whoever holds it.
    ///
    /// The coordination view: names and counts, never a path and never a line
    /// of anybody's work. A draft is unshared by definition, and what its
    /// author has not shared stays theirs until they do; what the person
    /// answerable for the domain needs in order to coordinate is that somebody
    /// is holding something and roughly how much, which is exactly this.
    ///
    /// A deletion counts as an entry, for the reason
    /// [`Engine::overlay_counts_by_actor`] gives. A domain that takes changes
    /// directly answers with an empty list rather than a refusal: nobody can
    /// draft there, so nobody is, and a client asking the same question of
    /// every domain gets one shape back.
    ///
    /// Gated exactly as unregistering it is - an instance admin, or a private
    /// domain's owner - and a caller who may not see the domain is answered as
    /// one naming a domain nobody registered.
    pub async fn domain_drafts(&self, name: &str, scope: &crate::scope::Scope) -> Result<Value> {
        self.require_domain_owner_refusing(
            name,
            scope,
            EngineError::Forbidden(format!(
                "who is drafting in domain '{name}' is for an instance admin, or for the owner \
                 of a private domain; your own drafts are in this domain's status"
            )),
        )
        .await?;
        match self.overlay_counts_by_actor(name).await {
            Some(counts) => Ok(json!({ "actors": crate::review::counts_json(&counts) })),
            None => Err(EngineError::Internal(format!(
                "who is drafting in domain '{name}' could not be read, because the index could \
                 not be asked"
            ))),
        }
    }

    /// What a removal would end, for a surface that asks before it acts.
    ///
    /// `{ domain, kind, engrams, files_kept }` - the three things somebody
    /// needs in order to answer the question, plus the one that decides how it
    /// is worded: a file or team domain's files stay on disk and a virtual
    /// domain's rows ARE its knowledge.
    ///
    /// Every refusal the removal itself would raise is raised here first, in
    /// the same order - the gate, the environment conflict, the unconfirmed
    /// purge, the unnamed drafts of other actors - so a question is never put
    /// about a removal that would refuse anyway. Advisory rather than
    /// authoritative: the removal re-decides all of it under its own lock,
    /// which is where the decision has to hold.
    pub async fn domain_remove_preview(
        &self,
        name: &str,
        scope: &crate::scope::Scope,
        purge: bool,
        end_drafts: &[String],
    ) -> Result<Value> {
        self.require_domain_owner(name, scope).await?;
        if let Some(conflict) = self.env_domain_conflict(name) {
            return Err(conflict);
        }
        let entry = self.domain_entry(name)?;
        let engrams = self.removal_engrams(name, &entry, purge).await?;
        let counts = self.overlay_counts_by_actor(name).await;
        crate::review::removal_choices(
            name,
            counts.as_deref(),
            crate::scope::overlay_actor(scope).as_deref(),
            end_drafts,
        )?;
        let (drafts, drafts_unknown) = match &counts {
            Some(counts) => (crate::review::counts_json(counts), false),
            None => (Vec::new(), true),
        };
        Ok(json!({
            "domain": name,
            "kind": Engine::removal_kind(&entry),
            "drafts": drafts,
            // The same distinction `engrams_unknown` draws, for the same
            // reason: nobody drafting here and "the index could not be asked"
            // are different answers to a question about somebody's unshared
            // work. Two things can set it now
            // ([`Engine::overlay_counts_by_actor`]): an index that could not
            // answer, and a files overlay that could not be listed. The journal
            // mirror beside them still sets nothing, because the count was
            // never the mirror's to give - but the files under the same
            // `overlays/<domain>` root are not a mirror, they are the only copy
            // of what they hold, so they do.
            "drafts_unknown": drafts_unknown,
            "engrams": engrams.as_json(),
            // Why the count is absent, so the question can say which: an index
            // that could not be read is a number that exists and is
            // unavailable, and it reads nothing like a domain that has synced
            // nothing yet.
            "engrams_unknown": engrams.is_unreadable(),
            "files_kept": !entry.is_virtual(),
        }))
    }

    /// Unregister a domain, gate and ordering included: the one entry point
    /// every surface calls.
    ///
    /// [`Engine::domain_remove`] below is the registry step alone. This is the
    /// whole of it, and the order of the four steps is not free to rearrange
    /// (see [`crate::collab::session::CollabSessions::dispose_domain`], which
    /// records the argument in full):
    ///
    /// 1. The domain-admin lock and the join fence go up, so no socket can open
    ///    a room in this domain from here on and no registration of the same
    ///    name can interleave. Without the fence the sweep would close what is
    ///    open and a join arriving one instant later would open a fresh room
    ///    over a domain that is about to vanish.
    /// 2. Every refusal is decided **inside** those guards: the gate
    ///    ([`Engine::require_domain_owner`]), the environment conflict, the
    ///    unconfirmed purge of a virtual domain's engrams, and the private
    ///    drafts of every OTHER actor, which have to be named before they are
    ///    ended ([`crate::review::removal_choices`]). That ordering is the
    ///    point of holding the lock at all - a gate decided outside it is a
    ///    check somebody's ownership transfer can land behind - and it is what
    ///    makes the preview above advisory rather than authoritative.
    /// 3. The rooms are swept while the domain is STILL registered, so each
    ///    room's final save lands in the file that stays on disk.
    /// 4. Only then is the domain unregistered, and only then are its
    ///    visibility and membership records swept.
    ///
    /// **The records go last, and that direction is deliberate.** They live in
    /// the accounts database and the registration lives in the config file, so
    /// there is no transaction spanning both and there cannot be one. Sweeping
    /// first and then failing the unregistration would leave a domain that is
    /// still registered and now SHARED - visible to everyone, the opposite of
    /// what its owner asked for. Failing the other way round leaves an acl row
    /// for a domain nobody has registered, which grants nothing until a domain
    /// of that name exists again; it is logged, and the residue is that a later
    /// re-add of the same name comes back private under the old owner rather
    /// than shared. That is the safe direction, and it is the same shape as the
    /// store's own domain row, which [`Engine::domain_remove`] also leaves in
    /// place.
    ///
    /// The report is [`Engine::domain_remove`]'s plus `rooms_closed`, so a
    /// client can say how many co-editing sessions it just ended.
    pub async fn unregister_domain(
        &self,
        name: &str,
        scope: &crate::scope::Scope,
        purge: bool,
        end_drafts: &[String],
    ) -> Result<Value> {
        // Ahead of the guards, and only this one: its answer is the same for
        // every caller and every name, so it discloses nothing and there is
        // nothing for a concurrent change to move.
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let _admin = self.domain_admin().await;
        let _fence = self.fence_joins().await;
        self.require_domain_owner(name, scope).await?;
        if let Some(conflict) = self.env_domain_conflict(name) {
            return Err(conflict);
        }
        // Before the sweep, not after: a removal that is going to refuse must
        // not have closed somebody's co-editing room on the way to refusing.
        let entry = self.domain_entry(name)?;
        self.removal_engrams(name, &entry, purge).await?;
        // Beside the purge and for the same reason, and before the sweep for
        // the same one again: a removal that is going to refuse must not have
        // closed somebody's co-editing room on the way to refusing. The count
        // is re-read here rather than carried from the preview, because the
        // preview ran outside these guards and somebody may have started
        // drafting since.
        crate::review::removal_choices(
            name,
            self.overlay_counts_by_actor(name).await.as_deref(),
            crate::scope::overlay_actor(scope).as_deref(),
            end_drafts,
        )?;
        // Counted here and not after the sweep, and here rather than below the
        // registry step: the files overlay is only walked for a domain that
        // reviews changes, and one line further down this domain is not
        // registered at all. The journal's own `remove_dir_all` takes these
        // files with the drafts by construction, so what is missing without
        // this line is the NUMBER - a removal that swept files it never
        // mentioned in front of the person who confirmed it.
        // Gated on the domain still reviewing changes, like every other reader
        // of that count: a domain whose fold failed mid-way and left files
        // behind reports a number that excludes them, while the journal sweep
        // below still takes them. The same under-report the orphan collector
        // beside it carries, and named here rather than hidden.
        let files_swept: u64 = self
            .overlay_file_counts(name)
            .flatten()
            .map(|per_actor| per_actor.values().sum())
            .unwrap_or(0);
        let rooms_closed = match self.collab.get().and_then(std::sync::Weak::upgrade) {
            Some(sessions) => sessions.dispose_domain(name).await,
            None => 0,
        };
        let mut report = self.domain_remove(name).await?;
        self.forget_domain_records(name).await;
        // Every draft in this domain has just ended, whichever way each one
        // ended, so every link on one and every session inside one ends with
        // them - the same call, for the same reason, that leaving review mode
        // makes.
        self.end_domain_grants(name).await;
        // The rows are cleared; the mirror that would bring them back goes with
        // them. A domain's removal takes every actor's drafts with it, and
        // leaving the journal behind would mean a domain re-added under this
        // name resurrecting somebody's old private drafts into it.
        let drafts_swept = self.sweep_domain_journal(name).await + files_swept;
        if let Value::Object(map) = &mut report {
            map.insert("rooms_closed".to_string(), Value::from(rooms_closed));
            map.insert("drafts_swept".to_string(), Value::from(drafts_swept));
        }
        Ok(report)
    }
}
