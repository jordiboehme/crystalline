use super::*;

impl Engine {
    // --- review mode -------------------------------------------------------

    /// The sessions this process is holding joins for. See [`crate::join`].
    pub fn joins(&self) -> &Arc<crate::join::Joins> {
        &self.joins
    }

    /// One named actor's draft at one path, or `None` when they hold none
    /// there.
    ///
    /// **The one read in this engine that answers about somebody else's draft
    /// by name**, and it exists for the share-link surface alone: minting a
    /// link has to know the author is holding what they are sharing, and
    /// redeeming one has to hand the grantee the draft the link was for. Both
    /// are the grant itself rather than a read that happened to widen, which
    /// is why they go through a named seam instead of through a view.
    ///
    /// It is not a [`DomainView`] and deliberately cannot become one: it takes
    /// a path rather than an identifier, so it can never resolve a name onto
    /// somebody else's draft, and it answers one row rather than a listing, so
    /// it can never be folded into a search.
    ///
    /// **Its callers, by name and exhaustively**, and
    /// `another_actors_draft_is_read_only_by_the_grant_surface` in
    /// crates/service/tests/overlay_domains.rs scans for any other:
    /// `rest::draft_links`'s `mint` and `list`, which name the CALLER's own
    /// account and so read nobody else's rows at all; `rest::draft_links`'s
    /// `open_link`, which names the grant row's `owner`; and, on the engine
    /// side, [`Engine::granted_read`], [`Engine::teach_granted_miss`],
    /// [`Engine::screen_granted_path`] and
    /// [`Engine::screen_joined_attachment`], each of which names an owner that
    /// came out of a grant row this instance minted. **No caller takes an
    /// actor from a request**, and one that did would be answering one reader
    /// with another reader's unshared work.
    ///
    /// `None` for a domain that takes changes directly, for one this index has
    /// never been told about, and for an actor holding nothing there - three
    /// ways of saying the same thing, which is that there is no draft to
    /// share.
    pub async fn overlay_draft_at(
        &self,
        domain: &str,
        actor: &str,
        path: &str,
    ) -> Result<Option<GrantedDraft>> {
        if !self.reviews_changes(domain) {
            return Ok(None);
        }
        let held = {
            let store = self.store.lock().await;
            // The read-only id lookup, never an upserting one: asking about a
            // domain this index has never seen must not register it.
            let Some(domain_id) = store.domain_id(domain).await? else {
                return Ok(None);
            };
            store.overlay_entry(domain_id, actor, path).await?
        };
        let Some(entry) = held else {
            return Ok(None);
        };
        // A tombstone is this actor's deletion of the base page, not a draft of
        // it: there is nothing to open and nothing to edit, so it is not
        // shareable and a link on a path that became one has nothing to give.
        if entry.tombstone {
            return Ok(None);
        }
        Ok(Some(GrantedDraft {
            path: entry.path,
            permalink: entry.permalink,
            content: entry.content,
            checksum: entry.sha256,
        }))
    }

    /// Whose draft `account` may see at that path, or `None` for the ordinary
    /// answer of nobody's.
    ///
    /// One hop into the accounts database, through the resolver the HTTP
    /// surface installs. `None` when no resolver is installed at all - a
    /// one-shot CLI command, the embedded stdio stack, a test engine - which
    /// is the right answer rather than a missing one: those surfaces are the
    /// machine owner, who has every draft on disk already and needs no link to
    /// be handed one.
    pub async fn granted_owner(
        &self,
        account: &str,
        domain: &str,
        path: &str,
    ) -> Result<Option<String>> {
        let Some(access) = self.domain_access.get() else {
            return Ok(None);
        };
        access
            .overlay_grant_for(account, domain, path)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))
    }

    /// End every share-link and every join into ONE draft, because that draft
    /// has just ended.
    ///
    /// Called where an actor's own draft is taken away - discarded outright
    /// when nothing in the folder stands under it, or replaced by their
    /// deletion of the page the team holds. A grant lasts exactly as long as
    /// the thing it grants, and a row that merely LOOKED dead while nothing
    /// stood at the path would spring back onto whatever its author drafted
    /// there next. The freshness check every grant surface makes stays where
    /// it is, as the belt to this.
    ///
    /// Reported rather than propagated, for the reason
    /// [`Engine::end_domain_grants`] gives: a delete that happened is not
    /// unsaid by a link that outlived it.
    pub(crate) async fn end_draft_grants(&self, domain: &str, owner: &str, path: &str) {
        self.joins.end_draft(domain, owner, path);
        let Some(access) = self.domain_access.get() else {
            return;
        };
        if let Err(e) = access.end_overlay_grants(domain, owner, path).await {
            tracing::warn!(
                domain = %domain,
                path = %path,
                error = %e,
                "could not end the draft share-links of a draft that was taken away"
            );
        }
    }

    /// End every share-link and every join into one domain, because every
    /// draft in it has just ended.
    ///
    /// Called by the fold, which is the one moment that ends all of them at
    /// once: whatever each actor chose, folded or discarded, no draft in the
    /// domain survives it, so no link on one and no session in one can stand.
    /// A grant ends with the thing it grants.
    ///
    /// Silently does nothing about links when no accounts database is
    /// installed - the CLI, the stdio stack, a test engine - for the reason
    /// [`Engine::granted_owner`] answers `None` there: no link was ever minted
    /// on such an instance. The joins are ended either way, since the registry
    /// is this engine's own.
    pub(crate) async fn end_domain_grants(&self, domain: &str) {
        self.joins.end_domain(domain);
        let Some(access) = self.domain_access.get() else {
            return;
        };
        if let Err(e) = access.end_domain_overlay_grants(domain).await {
            // A fold that happened is not unsaid by a link that outlived it,
            // and the link opens nothing either way: `redeem_overlay_grant`
            // hands back a path, and the draft at that path is gone. Reported
            // rather than propagated, so the fold's own answer stays the
            // fold's.
            tracing::warn!(
                domain = %domain,
                error = %e,
                "could not end the draft share-links of a domain that left review mode"
            );
        }
    }

    /// Every actor's drafts in one domain, ordered by actor and by path.
    ///
    /// The one place a per-actor view of an overlay is derived, so the fold
    /// plan below and Task 8's removal gate answer from the same rows rather
    /// than each deriving their own. The rows are the authority and the journal
    /// is their mirror: a draft whose mirror failed to land is still a draft its
    /// author is holding (`write_overlay_entry` reports that as a warning and
    /// keeps the row), and a plan read from the journal would quietly drop
    /// exactly those.
    pub(crate) async fn overlay_actor_drafts(
        &self,
        domain: &str,
        domain_id: DomainId,
    ) -> Result<Vec<ActorDrafts>> {
        // The actor set is the UNION of the two, and that is load bearing
        // rather than tidy: an actor holding only files appears in neither
        // `overlay_counts` nor `overlay_entries`, so a list drawn from the rows
        // alone would leave them out of the plan - `review::choices` would
        // never ask what happens to their files, and the sweep that ends every
        // actor's work would never reach them. Their bytes would survive into a
        // domain that no longer reviews anything, belonging to nobody.
        let files = self.overlay_domain_files(domain);
        let mut actors: BTreeSet<String> = files.per_actor.keys().cloned().collect();
        let held = {
            let store = self.store.lock().await;
            let mut held: BTreeMap<String, Vec<StoredEngram>> = BTreeMap::new();
            for (actor, _) in store.overlay_counts(domain_id).await? {
                let entries = store.overlay_entries(domain_id, &actor).await?;
                if entries.is_empty() {
                    continue;
                }
                actors.insert(actor.clone());
                held.insert(actor, entries);
            }
            held
        };
        let mut out = Vec::new();
        for actor in actors {
            let entries = held.get(&actor).cloned().unwrap_or_default();
            let own = files.per_actor.get(&actor);
            // An actor whose files folder is there, readable and empty holds
            // nothing at all, and nothing is not a thing to decide about:
            // listing them would put a `bob (0 draft(s))` line in the plan that
            // `review::choices` then demands an answer for, and would make a
            // domain holding nothing impossible to take out of review mode
            // without naming somebody who is not there. The LISTING keeps them
            // (the sweep wants exactly those folders); this per-actor view is
            // where they drop out.
            if entries.is_empty()
                && own.is_some_and(|read| read.entries.is_empty() && !read.unreadable)
                && !files.unreadable
            {
                continue;
            }
            out.push(ActorDrafts {
                entries,
                files: own.map(|read| read.entries.clone()).unwrap_or_default(),
                // An actor whose own folder could not be enumerated is flagged
                // and **listed**: the plan reports them and says their files
                // could not be read, where dropping them made a plan claim
                // there was nothing to decide over somebody's only copy of
                // their work. The domain's own folder failing is unknown for
                // everybody in it, so it is carried on every actor rather than
                // on none.
                files_unreadable: files.unreadable || own.is_some_and(|read| read.unreadable),
                actor,
            });
        }
        Ok(out)
    }

    /// Every actor's files-overlay entries in one domain, with the honesty
    /// flags beside them.
    ///
    /// The listing twin of [`Engine::overlay_file_counts`], and **deliberately
    /// not gated the way that one is**: the tree is walked whatever mode the
    /// domain is in.
    ///
    /// Its callers are the fold and the convergence pass, and the fold takes
    /// the review key off in the middle of itself. A read that asked the key
    /// would see an actor's files on the first call and not on a repeat - the
    /// mid-fold recovery [`Engine::set_review_mode`] documents - so the repeat
    /// would sweep files it had never folded. The count beside this one is
    /// gated because its callers are listings of every domain, where a direct
    /// domain must answer byte for byte what it always did.
    pub(super) fn overlay_domain_files(&self, domain: &str) -> crate::overlay_files::DomainFiles {
        let Ok(state_dir) = self.journal_state_dir() else {
            // **Nothing rather than unknown, and only here.** A process that
            // cannot resolve its own state directory has never written an
            // overlay file either - every write goes through this same resolver
            // - so for this reader there is nothing it is failing to see, and
            // the fold it feeds would refuse every domain on a machine whose
            // state directory has gone missing rather than folding the rows it
            // can still reach. The counting twin asks the resolver itself and
            // answers `unknown` there, which is the right answer for a removal
            // gate: that one is about what a person is being asked to end.
            tracing::warn!(
                domain,
                "the files overlay of '{domain}' could not be located; no file can have been \
                 written there by this process, so this reads as nothing rather than as an \
                 unknown"
            );
            return crate::overlay_files::DomainFiles {
                per_actor: BTreeMap::new(),
                unreadable: false,
            };
        };
        crate::overlay_files::by_actor(&state_dir, domain)
    }

    /// The refusal leaving review mode owes when any part of this domain's
    /// files overlay cannot be listed, or [`None`] when all of it can.
    ///
    /// **Whatever anybody answered, and whether or not that actor is listed.**
    /// An unreadable listing used to drop a file-only actor out of the plan
    /// entirely while the sweep at the end - a walk of the tree, not of the
    /// plan - went on removing every actor it could reach. So a confirm with no
    /// folding actor at all destroyed a readable actor's only copy of their
    /// work after the plan had said there was nothing to decide. The decision
    /// is keyed off the listing itself for that reason, never off who is
    /// folding.
    ///
    /// It is the same class of refusal an unreadable row count raises on a
    /// removal without purge: the branch that decides whether somebody's only
    /// copy of their work ends must never read a failure as "there was nothing
    /// there".
    pub(super) fn refuse_unlistable_files(&self, domain: &str) -> Option<EngineError> {
        let files = self.overlay_domain_files(domain);
        let whose = files.unlistable()?;
        // **Only where there is something for it to protect.** A domain that
        // takes changes directly and holds no draft file anybody can see has no
        // fold for a half-read listing to spoil, and the sweep that could have
        // destroyed something is guarded on its own account. Refusing there
        // would turn `PUT {"mode":"direct"}` on a domain that already takes
        // changes directly - a statement that writes nothing - into an error
        // over a tree nobody in that domain can write to.
        let anything_held = files
            .per_actor
            .values()
            .any(|read| !read.entries.is_empty());
        if !self.reviews_changes(domain) && !anything_held {
            return None;
        }
        let whose = match whose {
            Some(actor) => format!("the files '{actor}' has drafted in domain '{domain}'"),
            None => format!("the files overlay of domain '{domain}'"),
        };
        Some(EngineError::Conflict(format!(
            "{whose} could not be read, and leaving review mode ends every draft in it one way \
             or the other. Nothing was folded, nothing was ended and the domain reviews changes \
             still; answer again once the state directory can be read"
        )))
    }

    /// Turn review mode on for a domain, or take it off and settle every
    /// actor's drafts on the way out.
    ///
    /// **Turning it on** is a promise: from here on, a change to this domain
    /// joins its author's own draft and reaches the folder the team shares only
    /// through a reviewed proposal. Three things have to be true for that
    /// promise to be keepable, and each refusal says which one is not:
    ///
    /// 1. **A GitHub origin.** Review with no proposal flow behind it is a gate
    ///    with no door: the drafts would have nowhere to go and the mode would
    ///    only stop people writing.
    /// 2. **A folder, so not a virtual domain.** A virtual domain's engrams ARE
    ///    its rows, so a fold would have nothing to land in.
    /// 3. **Nothing unshared in the folder already.** Work already sitting in
    ///    the tree went round no review at all, and turning the mode on over it
    ///    would bless it silently. The refusal names the paths and says to share
    ///    or revert them first. When [`crate::origin::unshared_work`] answers
    ///    `None` (no origin state recorded yet, or a tree that cannot be
    ///    walked) that is "nothing KNOWN to be unshared" rather than "clean",
    ///    and it is read as permission: a domain connected but never pulled has
    ///    no snapshot to compare against, and refusing every one of those would
    ///    make the mode unreachable exactly where it is wanted.
    ///
    /// **Taking it off** ends every actor's private drafts, so it is never
    /// decided by omission. [`ReviewModeConfirm::Preview`] answers the plan -
    /// who holds what, which drafts are deletions, which paths more than one
    /// actor is drafting and which drafts could not land - and writes nothing.
    /// [`ReviewModeConfirm::Confirmed`] carries one [`FoldChoice`] per actor
    /// holding drafts: an actor left out refuses naming them, and an actor named
    /// who holds nothing refuses too, because both are somebody meaning a
    /// different domain or a different moment.
    ///
    /// The order of the confirmed path is not free to rearrange, and it is the
    /// removal's order with the same argument made about a different pair:
    ///
    /// 1. Every refusal is decided **inside** the domain-admin lock and the join
    ///    fence, including the collision check, so nothing is written at all by
    ///    a call that is going to refuse.
    /// 2. The key comes off, and only then are the co-editing rooms swept
    ///    ([`crate::collab::session::CollabSessions::dispose_domain`]). **That
    ///    pair is the reason this step exists at all**: a room is a room over
    ///    one overlay document, so a room swept while the domain is still
    ///    reviewing lands its unsaved text in a DRAFT row - refreshing one the
    ///    plan was drawn against, or making one for an actor no answer covers -
    ///    and step 4 drops every actor's rows a moment later, so that text
    ///    would be typed into a bin. Swept one instant after the key comes off,
    ///    the same save lands in the file: the room's view falls back to the
    ///    folder's own text the moment the domain stops reviewing changes (see
    ///    `crate::collab::session`'s `room_view`), which is exactly what the
    ///    removal path means by sweeping while the domain is still
    ///    registered - there, the drafts are what unregistering ends, so a
    ///    swept room's save goes into the draft and out with it.
    ///    The sweep is told which actors are being DISCARDED and closes a room
    ///    over one of their drafts WITHOUT saving it: discard means those rows
    ///    are dropped unwritten, and a room whose view has just fallen back to
    ///    the folder would otherwise publish the very text this call said must
    ///    not reach it. What was unsaved ends with the draft it was typed
    ///    into, which is what discarding it means.
    /// 3. The folds land as ordinary file writes and deletions, over the files
    ///    those saves just landed in. Where a fold and a room are about the same
    ///    path the fold is the last word - which is the answer the plan was
    ///    confirmed for, and is worth being exact about: the draft row is that
    ///    room's last AUTOSAVE, so a delta typed since it and flushed by the
    ///    sweep a moment ago is superseded by slightly older text. The other
    ///    order loses the same bytes (the fold would be overwritten instead),
    ///    so this is the cost of folding a path somebody is editing rather than
    ///    a cost of the ordering. Nothing writes an index row here: the sync at
    ///    the end reads the tree the way it reads every other change to it.
    /// 4. Every actor's rows and mirror go, folded and discarded alike, before
    ///    the sync - both because the restore runs in every sync pass and would
    ///    put a missed mirror straight back, and because a draft row still
    ///    holding an address would meet the base row the fold just gave it to.
    ///    They go over the overlay as it stands then, not as the plan found it,
    ///    so a draft written into the window between the two is dropped rather
    ///    than stranded.
    /// 5. Then the domain syncs, which is what puts the folds in the index.
    ///
    /// A failure in the middle of step 3 leaves the domain taking changes
    /// directly with part of its overlay folded, and the recovery is the same
    /// call again: leaving review mode does not require the domain to be in it,
    /// so a repeat picks up the drafts that are left and finishes.
    ///
    /// `folds` naming the same actor twice is refused rather than resolved:
    /// two answers about one person's unshared work is a caller that does not
    /// know what it is asking for.
    pub async fn set_review_mode(
        &self,
        domain: &str,
        mode: Option<crystalline_core::config::ReviewMode>,
        confirm: ReviewModeConfirm,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let previewing = matches!(confirm, ReviewModeConfirm::Preview);
        // Ahead of the guards, and only this one: its answer is the same for
        // every caller and every name, so it discloses nothing.
        //
        // **The preview is refused with everything else, deliberately.** It
        // reads like a question, but `leave_review_mode` resolves the domain id
        // through `upsert_domain` before it can ask anything, and that is a
        // write. Serving it here would mean a route this surface declares
        // `read_only_exempt: false` writing on a read-only instance, which is
        // the kind of gap nothing downstream would ever notice. A read-only
        // instance has nothing to answer about anyway: it refuses every write
        // that could have made a draft.
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let _admin = self.domain_admin().await;
        let _fence = self.fence_joins().await;
        self.require_domain_owner(domain, scope).await?;
        if let Some(conflict) = self.env_domain_conflict(domain) {
            return Err(conflict);
        }
        let entry = self.domain_entry(domain)?;
        match mode {
            Some(crystalline_core::config::ReviewMode::Overlay) => {
                self.enable_review_mode(domain, &entry, previewing).await
            }
            None => self.leave_review_mode(domain, &entry, confirm).await,
        }
    }

    /// The enable half of [`Engine::set_review_mode`]: the three gates, then
    /// the config key.
    async fn enable_review_mode(
        &self,
        domain: &str,
        entry: &DomainEntry,
        previewing: bool,
    ) -> Result<Value> {
        let receipt = |applied: bool| {
            json!({
                "domain": domain,
                "mode": "overlay",
                "review": "overlay",
                "applied": applied,
            })
        };
        if entry.is_overlay() {
            // Already what was asked for. Answered rather than refused, the way
            // privatizing an already-private domain is: a PUT states a mode,
            // and this one already holds.
            return Ok(receipt(!previewing));
        }
        if entry.is_virtual() {
            return Err(EngineError::Conflict(format!(
                "domain '{domain}' is a virtual domain: its engrams live in the database and it \
                 has no folder for a reviewed change to land in, so it cannot review changes. \
                 Register the knowledge as a file domain connected to a GitHub repository first"
            )));
        }
        if entry.origin.is_none() {
            return Err(EngineError::Conflict(format!(
                "domain '{domain}' has no GitHub origin, and review mode with nothing to propose \
                 a reviewed change to is a gate with no door: connect it to a GitHub repository \
                 first, then enable review"
            )));
        }
        let Some(root) = entry.file_path() else {
            return Err(EngineError::Conflict(format!(
                "domain '{domain}' has no folder on this machine, so there is nothing here to \
                 review changes to"
            )));
        };
        let state_dir = self.origin_state_dir(domain)?;
        if let Some(unshared) = crate::origin::unshared_work(&root, &state_dir)
            && !unshared.paths.is_empty()
        {
            return Err(EngineError::Conflict(format!(
                "domain '{domain}' has {} unshared change(s) in its folder that the team has not \
                 reviewed: {}. Review mode is the promise that every change is reviewed before it \
                 lands, and this work went round it, so share or revert these first, then enable \
                 review",
                unshared.paths.len(),
                unshared.paths.join(", ")
            )));
        }
        if previewing {
            return Ok(receipt(false));
        }
        self.write_review_key(domain, Some(crystalline_core::config::ReviewMode::Overlay))?;
        Ok(receipt(true))
    }

    /// The disable half of [`Engine::set_review_mode`]: the per-actor plan, and
    /// the folds and discards that carry it out.
    async fn leave_review_mode(
        &self,
        domain: &str,
        entry: &DomainEntry,
        confirm: ReviewModeConfirm,
    ) -> Result<Value> {
        let root = entry.file_path();
        let domain_id = {
            let store = self.store.lock().await;
            store
                .upsert_domain(
                    domain,
                    root.as_ref().map(|r| r.to_string_lossy()).as_deref(),
                    if entry.is_virtual() {
                        DomainKind::Virtual
                    } else {
                        DomainKind::File
                    },
                )
                .await?
        };
        let drafts = self.overlay_actor_drafts(domain, domain_id).await?;
        let base = {
            let store = self.store.lock().await;
            store.list_engrams(domain, None, None).await?
        };

        let choices = match &confirm {
            ReviewModeConfirm::Preview => {
                return Ok(review::plan_json(domain, entry, &drafts, &base));
            }
            ReviewModeConfirm::Confirmed { folds } => folds,
        };
        // **Before the choices are even read**, and whatever they say. Leaving
        // review mode ends every draft in the domain one way or the other, so a
        // part of the overlay nobody can list is a part of the answer nobody
        // can give - and asking somebody to decide about drafts this machine
        // cannot enumerate is putting a question it could not honour. The plan
        // above still answers and reports the actor whose files could not be
        // read, so the refusal here is never the first the caller hears of it.
        if let Some(refusal) = self.refuse_unlistable_files(domain) {
            return Err(refusal);
        }
        let choices = review::choices(domain, &drafts, choices)?;
        let folding: Vec<&ActorDrafts> = drafts
            .iter()
            .filter(|d| choices.get(&d.actor) == Some(&FoldChoice::Fold))
            .collect();
        if let Some(refusal) = review::collision(domain, &folding, &base) {
            return Err(EngineError::Conflict(refusal));
        }

        // Nothing to do, said as nothing done. The conjunction is the point:
        // a domain that is not reviewing AND holds no drafts is a `PUT
        // {"mode":"direct"}` that states what already holds, and closing every
        // co-editing room in it, syncing it and refreshing the routing cache
        // would be a lot of consequence for a statement. A domain the key has
        // already come off but whose overlay is not empty is the mid-fold
        // recovery [`Engine::set_review_mode`] documents, and that one has to
        // run.
        if !entry.is_overlay() && drafts.is_empty() {
            return Ok(review::left_json(domain, Vec::new(), Vec::new(), 0));
        }

        // The key comes off first, and then the rooms go, and that pair is the
        // whole of step 2: see the ordering on [`Engine::set_review_mode`].
        //
        // The sweep is told which actors are being DISCARDED, and a room over
        // one of their drafts is closed without being saved. Every other room
        // saves first, as it always did. Discard means the rows are dropped
        // unwritten, so a room over one of them - with the key already off,
        // and its view already fallen back to the folder - would otherwise
        // publish into the reviewed tree the one text this call said must not
        // go there.
        //
        // **This trusts `choices` to be total, and that trust is load
        // bearing.** `discarded` is read straight off the confirmed map above
        // with no further check that every drafting actor is in it -
        // `review::choices`'s own refusal a few lines up is what makes the
        // set total, by answering `ConfirmationRequired` for any actor its
        // caller left out. If that refusal ever loosens, an actor holding
        // drafts but missing from the map would fall through this filter
        // unnamed, and their room would save into the tree rather than close
        // discarding - the publish this call exists to prevent.
        self.write_review_key(domain, None)?;
        let discarded: HashSet<String> = choices
            .iter()
            .filter(|(_, choice)| **choice == FoldChoice::Discard)
            .map(|(actor, _)| actor.clone())
            .collect();
        let rooms_closed = match self.collab.get().and_then(std::sync::Weak::upgrade) {
            Some(sessions) => sessions.dispose_domain_discarding(domain, &discarded).await,
            None => 0,
        };

        // **And the same question again, of the folder the folded rooms just
        // wrote into.** The key is off, so every swept room that saved wrote
        // into the folder,
        // and a participant who edited the frontmatter's permalink line has
        // moved a base engram's address between the check above and this line; a fold validated
        // against the older folder would write a second engram at that address
        // and the first thing to notice would be the `sync` at the end - by
        // which time the files are written, every actor's rows are dropped and
        // every later sync of this domain fails the same way, with nothing left
        // to undo it from. Asked here instead, the refusal costs the rooms
        // their sockets and nothing else: the key goes back on, no file is
        // written and no row is dropped, so the same call works once one of the
        // two engrams has an address of its own.
        let base = {
            let store = self.store.lock().await;
            store.list_engrams(domain, None, None).await?
        };
        if let Some(refusal) = review::collision(domain, &folding, &base) {
            self.write_review_key(domain, Some(crystalline_core::config::ReviewMode::Overlay))?;
            return Err(EngineError::Conflict(format!(
                "{refusal}. The folder changed while this domain's co-editing rooms were being \
                 closed, so nothing was folded and the domain reviews changes again; what those \
                 rooms saved is in the folder now, which `origin status` lists as out-of-band work"
            )));
        }

        let mut folded = Vec::new();
        let mut discarded = Vec::new();
        for held in &drafts {
            match choices.get(&held.actor) {
                Some(FoldChoice::Fold) => {
                    let (mut written, mut deleted) = (0u64, 0u64);
                    if let Some(root) = &root {
                        for draft in &held.entries {
                            let abs = join_rel(root, &draft.path);
                            if draft.tombstone {
                                match std::fs::remove_file(&abs) {
                                    Ok(()) => deleted += 1,
                                    // A deletion of a file that is already gone
                                    // is the state the deletion asked for.
                                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                    Err(source) => {
                                        return Err(EngineError::Io {
                                            path: abs.display().to_string(),
                                            source,
                                        });
                                    }
                                }
                            } else {
                                write_file(&abs, &draft.content)?;
                                written += 1;
                            }
                        }
                    }
                    // And the files, after the rows. A page folds as a file
                    // write and the sync at the end indexes it; an attachment
                    // has no sync pass of its own, so each one goes through the
                    // same pair the upload verb uses - the bytes under this
                    // file's own write lock, and the row built from those bytes
                    // with the modification instant read back off the file.
                    let (wrote, removed) = self.fold_files(domain, held).await?;
                    written += wrote;
                    deleted += removed;
                    folded.push(json!({
                        "actor": held.actor,
                        "written": written,
                        "deleted": deleted,
                    }));
                }
                _ => discarded.push(json!({
                    "actor": held.actor,
                    "entries": held.entry_count(),
                })),
            }
        }

        // Rows and mirror together, for every actor, and over the overlay as it
        // stands NOW rather than as the plan found it: see step 4 of the
        // ordering.
        for held in self.overlay_actor_drafts(domain, domain_id).await? {
            let view = DomainView::for_actor(self, domain, &HashSet::new(), &held.actor)?;
            for draft in &held.entries {
                view.drop(domain_id, &draft.path).await?;
            }
        }
        // The files go with the rows, folded and discarded alike: whichever
        // answer each actor gave, drafting in this domain is over, and a files
        // overlay left behind would be bytes belonging to nobody in a domain
        // that reviews nothing.
        //
        // Swept over the tree itself rather than over the loop above, because
        // the review key came off in step 2: an actor holding only files is not
        // in a listing that asks a domain whether it reviews changes any more,
        // and theirs are exactly the bytes nothing else would ever reach.
        self.sweep_every_actors_files(domain);

        // Every share-link on a draft here, and every session joined to one,
        // ends with the drafts themselves. A grant lasts as long as the thing
        // it grants: a folded draft is in the folder where everybody can read
        // it anyway, and a discarded one is not there at all, so a link that
        // outlived either would name a draft that is not there and a session
        // still joined would be joined to nothing.
        self.end_domain_grants(domain).await;

        // The folds are ordinary file writes, so the ordinary sync is what puts
        // them in the index - and it refreshes the generated folder indexes on
        // the way. The routing cache is not its job, and a folded MANIFEST is
        // the domain's routing, so that one is refreshed here.
        //
        // **The refresh happens whether the sync succeeded or not**, and the
        // error is carried past it rather than returned through it. A failed
        // sync is the one tail this call cannot offer a repeat of: the rows are
        // already dropped by then, so a second call finds an overlay-less
        // domain that is no longer reviewing and the no-op above answers it
        // without doing anything. The files are on disk either way and a later
        // sync picks them up, but the routing cache is in this process's memory
        // and nothing else would ever refresh it, so a folded MANIFEST would go
        // on routing agents by what the domain said before the fold until the
        // daemon restarted.
        let synced = self.sync(Some(domain)).await;
        self.refresh_routing_cache().await;
        self.nudge_embed();
        synced?;

        Ok(review::left_json(domain, folded, discarded, rooms_closed))
    }

    /// Land one folding actor's files in the folder the team shares, answering
    /// `(written, deleted)`.
    ///
    /// Each write is the upload verb's own pair: the bytes under this file's
    /// per-file [`Engine::write_lock`], and the row built from those bytes with
    /// the modification instant read back off the file, so the folded file
    /// costs the next sync walk no re-hash. Each deletion is
    /// [`Engine::attachment_delete`]'s pair the same way - the file and the
    /// row - and a path that is already gone is the state the deletion asked
    /// for rather than a failure, exactly as the engram arm above treats one.
    ///
    /// A virtual domain never reaches this: review mode refuses one, and a
    /// domain with no folder has nowhere for a file to land.
    async fn fold_files(&self, domain: &str, held: &ActorDrafts) -> Result<(u64, u64)> {
        if held.files.is_empty() {
            return Ok((0, 0));
        }
        let state_dir = self.journal_state_dir()?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let ContentSource::File { root } = &source else {
            return Ok((0, 0));
        };
        let (mut written, mut deleted) = (0u64, 0u64);
        for file in &held.files {
            let abs = contained_asset_path(root, &file.path)?;
            if file.tombstone {
                let lock = self.write_lock(&abs);
                let guard = lock.lock().await;
                match std::fs::remove_file(&abs) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                }
                let store = self.store.lock().await;
                store.delete_attachment(domain_id, &file.path).await?;
                drop(store);
                drop(guard);
                deleted += 1;
                continue;
            }
            let bytes = crate::overlay_files::read(&state_dir, domain, &held.actor, &file.path)
                .map_err(|source| EngineError::Io {
                    path: format!("the files overlay of '{domain}' at '{}'", file.path),
                    source,
                })?
                .ok_or_else(|| {
                    EngineError::Conflict(format!(
                        "'{}' drafted {} in domain '{domain}' and its bytes are no longer \
                         there, so the fold has nothing to land",
                        held.actor, file.path
                    ))
                })?;
            // The file lock before the store lock, like every other writer
            // here. See `Engine::write_lock`.
            let lock = self.write_lock(&abs);
            let _guard = lock.lock().await;
            write_bytes(&abs, &bytes)?;
            let row = attachment_row(&file.path, &bytes, asset_modified(&abs))?;
            let store = self.store.lock().await;
            store.upsert_attachment(domain_id, &row).await?;
            written += 1;
        }
        Ok((written, deleted))
    }

    /// Drop every actor's files overlay in one domain, as the last half of
    /// leaving review mode.
    ///
    /// Read off the tree itself rather than off the plan, because the plan was
    /// drawn before the key came off: what is swept is every actor the tree
    /// still names, which is exactly the set that would otherwise be left. The
    /// cost of that is the same one the row drop loop already documents - a
    /// file uploaded between the plan and this line is not in anybody's fold
    /// and goes here - and it is sharper for a file than for a row: a dropped
    /// row is still mirrored in the journal, and these bytes are the only copy
    /// there is. The window is the tail of one verb, behind the domain-admin
    /// lock and the join fence.
    ///
    /// **Nothing at all is swept when any part of the listing could not be
    /// read.** A partial sweep would end exactly the actors whose work could
    /// still be seen while leaving the unreadable one, which is the wrong half
    /// of an answer nobody gave; leaving everything lets a repeat do the whole
    /// thing once the tree can be read. `leave_review_mode` refuses long before
    /// this line in that case ([`Engine::refuse_unlistable_files`]), so this is
    /// the second lock on the same door rather than the first.
    ///
    /// Best effort otherwise, like the journal sweep on the removal path and
    /// for the same reason: by the time this runs the answer has been carried
    /// out - the files are in the folder, or the actor said to end them - and
    /// failing here would report a fold that happened as one that did not. What
    /// is left behind is logged, and a domain's removal sweeps the whole tree
    /// anyway.
    fn sweep_every_actors_files(&self, domain: &str) {
        let Ok(state_dir) = self.journal_state_dir() else {
            return;
        };
        let held = crate::overlay_files::by_actor(&state_dir, domain);
        if held.unlistable().is_some() {
            tracing::warn!(
                domain,
                "the files overlay of '{domain}' could not be fully read while leaving review \
                 mode, so none of it was swept; what is there is nobody's draft now and goes \
                 with the domain if it is ever unregistered"
            );
            return;
        }
        for actor in held.per_actor.keys() {
            if let Err(e) = crate::overlay_files::remove_actor(&state_dir, domain, actor) {
                tracing::warn!(
                    domain,
                    actor = actor.as_str(),
                    "the files '{actor}' drafted in '{domain}' could not be removed after \
                     leaving review mode: {e}"
                );
            }
        }
    }

    /// Write a domain's `review` key through the file config and into the
    /// effective one, the write-lock-first order every config mutation here
    /// follows so no env value bakes into the saved file.
    ///
    /// A domain absent from the file answers [`EngineError::UnknownDomain`],
    /// the same answer [`Engine::domain_remove`] gives it. The file, not the
    /// startup snapshot: a domain another process registered in the config
    /// file after this engine started passed the gates above through
    /// `domain_entry` and used to miss here, since the snapshot never learned
    /// of it. Every config mutation now starts from the file on disk (see
    /// [`Engine::fresh_file_config`]), so the one shape left is a domain the
    /// file really does not hold.
    fn write_review_key(
        &self,
        domain: &str,
        review: Option<crystalline_core::config::ReviewMode>,
    ) -> Result<()> {
        let mut file_guard = self.file_config.write().unwrap();
        let mut file = self.fresh_file_config(&file_guard);
        let Some(entry) = file.domains.get_mut(domain) else {
            return Err(EngineError::UnknownDomain {
                domain: domain.to_string(),
                registered: self.known_domain_names(),
            });
        };
        entry.review = review;
        self.persist_config(&file)?;
        let effective = self.overlay.apply(&file);
        *file_guard = file;
        *self.config.write().unwrap() = effective;
        // A domain discovered after this engine started keeps a cached entry of
        // its own beside the two configs above, and `domain_entry` falls back
        // to it when `self.config` has no such name. The effective config just
        // written does hold the name (it is what this call persisted), so the
        // fallback is not reached and this write changes no answer today; it is
        // here so the cache cannot go on saying a domain reviews changes after
        // this call decided it does not, whichever of the two a later reader
        // happens to reach.
        if let Some(found) = self.discovered_domains.write().unwrap().get_mut(domain) {
            found.review = review;
        }
        Ok(())
    }

    /// Retire the visibility and membership records of a domain that is no
    /// longer registered.
    ///
    /// Best effort, and deliberately not a failure of the removal it follows:
    /// by the time this runs the domain is gone, and answering with an error
    /// would tell the caller their removal did not happen when it did. A
    /// failure is logged and the residue is documented on
    /// [`Engine::unregister_domain`].
    ///
    /// A no-op on an engine with no resolver installed, which is every
    /// installation with no accounts database: privacy is a membership record,
    /// and a machine with no accounts has none.
    pub(super) async fn forget_domain_records(&self, name: &str) {
        let Some(access) = self.domain_access.get() else {
            return;
        };
        if let Err(e) = access.forget_domain(name).await {
            tracing::warn!(
                domain = name,
                error = format!("{e:#}"),
                "domain '{name}' was unregistered but its visibility and membership records \
                 could not be cleared; a domain later re-added under this name will come back \
                 private under its old owner until an admin makes it shared"
            );
        }
    }

    /// Unregister a domain: the config entry goes, the watcher and discovery
    /// forget it and its index rows are cleared so search stops serving it.
    /// Files are never touched - for a file domain they stay on disk exactly
    /// as they are (re-adding the folder re-adopts them); a virtual domain's
    /// rows ARE its truth, so callers should export first and their
    /// confirmation copy must say the knowledge is gone. "Index rows cleared"
    /// means the engram rows only: the store's domain row itself is left in
    /// place (`Store::clear_domain` keeps it by design), so a later re-add of
    /// the same name adopts the same row rather than minting a new one.
    ///
    /// Known race: the config write (name freed) is persisted and both config
    /// locks release before the tail runs `forget_domain` and `clear_domain`.
    /// No lock in this engine currently serializes `domain_remove` against a
    /// concurrent `domain_add_local`/`domain_add_virtual` for the same name
    /// (the daemon spawns each connection independently, and the admin verbs
    /// only hold `file_config`/`config` for their brief mutate-and-persist
    /// step, not the whole call - `origin_lock` exists but serializes only
    /// the origin verbs against each other, not these). A same-name add
    /// racing into that window has its fresh watcher registration dropped and
    /// its freshly-indexed rows wiped by this call's tail, since both resolve
    /// the same `DomainId` by name. Closing this needs the add verbs to take
    /// the same per-name lock this verb would need to hold across its own
    /// tail, which is a cross-verb change out of scope here.
    ///
    /// [`Engine::domain_admin`] narrows the window without closing it, and it
    /// is worth being exact about which half. [`Engine::unregister_domain`] -
    /// the entry point every surface goes through - holds it across this whole
    /// call, and the REST create holds it across its own registration, so those
    /// two cannot interleave whichever surface each arrives on. The bare
    /// `domain_add_local`, `domain_add_virtual` and `origin_add` verbs take no
    /// lock at all, so an add reaching the engine directly can still race a
    /// removal for the same name. That is the residue above, and it is the same
    /// residue as before the lock moved onto this type; what changed is that
    /// the lock is no longer one surface's, so it no longer leaves a second
    /// surface's callers unserialized against each other.
    ///
    /// **This is the registry step alone.** [`Engine::unregister_domain`] is
    /// the entry point: it decides who may end a domain, raises the join fence,
    /// sweeps the co-editing rooms and retires the domain's visibility records
    /// around this call. Reaching for this one directly skips all of that; the
    /// only caller that does so on purpose is the REST create's rollback, which
    /// already holds the lock the entry point would take.
    pub async fn domain_remove(&self, name: &str) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }

        // Same file-lock persist dance as `domain_add_local`: file_config
        // write lock, clone, mutate, persist_config, overlay.apply, write
        // both locks - same order. The miss is classified before persisting
        // anything. Scoped in a block, mirroring the sibling verbs, so the
        // guard is released well before the awaits that follow.
        let removed = {
            let mut file_guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&file_guard);
            let removed = match file.domains.shift_remove(name) {
                Some(entry) => entry,
                None => {
                    // A miss in the file config may be an env-defined domain:
                    // those are immune to `domain_remove` (the variable is
                    // their source of truth), mirroring `cmd::domain_remove`.
                    if let Some(env) = self.overlay.env_domain(name) {
                        return Err(EngineError::Conflict(format!(
                            "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                            env.var
                        )));
                    }
                    return Err(EngineError::UnknownDomain {
                        domain: name.to_string(),
                        registered: self.known_domain_names(),
                    });
                }
            };
            self.persist_config(&file)?;
            let effective = self.overlay.apply(&file);
            *file_guard = file;
            *self.config.write().unwrap() = effective;
            removed
        };
        // Record whether the entry was a file domain before removing it.
        let files_kept = !removed.is_virtual();

        // Tell the runtime: drop it from the discovered overlay and stop
        // watching its root.
        self.forget_domain(name);

        // Index rows: resolve the DomainId the way `reindex(full)` does
        // before it calls `clear_domain` and clear them. The domain row
        // stays either way (idempotent upsert); only the engram rows matter.
        let kind = if removed.is_virtual() {
            DomainKind::Virtual
        } else {
            DomainKind::File
        };
        let path = removed.file_path();
        let path_str = path.as_ref().map(|p| p.to_string_lossy());
        let store = self.store.lock().await;
        let domain_id = store.upsert_domain(name, path_str.as_deref(), kind).await?;
        store.clear_domain(domain_id).await?;
        drop(store);

        self.refresh_routing_cache().await;

        Ok(json!({
            "domain": name,
            "unregistered": true,
            "files_kept": files_kept,
            "index_cleared": true,
        }))
    }

    /// Drop the engram rows of every domain that is no longer registered, and
    /// report every domain considered.
    ///
    /// `domain_remove` clears a domain's rows as it unregisters it, so nothing
    /// this instance removes ever becomes an orphan. This is for the rows that
    /// got past that: a removal on a version that left them behind, a
    /// configuration hand-edited or restored from a backup, an index carried
    /// between machines. They are already unserved (a row whose domain nobody
    /// registered is not a hit, not a count and not a facet value), so this
    /// costs nothing to defer and the grace period is a matter of disk.
    ///
    /// **`grace` is who is asking**, and it is the only difference between the
    /// two callers:
    ///
    /// - `Some(d)` is the **daemon's unattended sweep**. A domain is collected
    ///   only when its `last_registered` stamp says it has been gone longer
    ///   than `d`. `last_registered` reading `None` is *never stamped*, not
    ///   *stamped infinitely long ago*: such a domain has its clock started on
    ///   this sweep and is collected on no sweep that could not already see its
    ///   age, so the first sweep after an upgrade - when every inherited row
    ///   reads `None` - collects nothing.
    /// - `None` is **a person asking** (`doctor --fix`, and `doctor`'s report
    ///   with `dry_run`). Every unregistered domain is collected whatever its
    ///   stamp says, a `None` stamp included. The grace period exists to wait
    ///   for exactly this signal, so waiting past it would be waiting for
    ///   something that has already happened - and an index inherited from a
    ///   version that stranded its rows has nothing but `None` stamps, which
    ///   must clear on first contact rather than a week after it.
    ///
    /// Two conditions hold on both paths and are never waived. The domain is
    /// **not hosted by a live peer**: on a shared database several instances
    /// register different domains against one index, so a domain with another
    /// instance's host lock on it and a heartbeat inside the stale threshold is
    /// another instance's current work, and its registration is a registration
    /// (`kept: "hosted_elsewhere"`). And the domain is
    /// **absent from the configuration**, resolved through
    /// [`Engine::registered_domain_names_checked`] - the three tiers a *named*
    /// lookup resolves through, so a domain the file gained after startup is
    /// registered here as it is everywhere else. A configuration that could not
    /// be read (unparseable, or not there) is not evidence of absence: the
    /// sweep then establishes no registered set, stamps nothing, collects
    /// nothing and says so in `skipped`.
    ///
    /// Every registered domain is stamped *first*, before anything is
    /// considered, which is what makes a week of the machine being off, or of
    /// this process being read-only, cost nothing.
    ///
    /// Domains that are reported and never collected, on either path. A
    /// **virtual** domain's engram rows are not a derived copy of files on
    /// disk, they are the knowledge itself - `domain_remove` refuses to drop
    /// them without an explicit purge, and this answers nobody's confirmation,
    /// so it reports one (`"kept": "virtual"`) and leaves the removal to the
    /// person and that command. A domain **hosted by a live peer** belongs to
    /// that peer. A domain with **no engram rows** has nothing to collect.
    ///
    /// A **read-only** instance keeps every candidate, each reading
    /// `kept: "read_only"`: it reports the age of each one, and deliberately
    /// not a judgement about it, since the judgement is a decision it could not
    /// carry out. It does stamp its registered domains, which is index
    /// maintenance rather than a content write, and on a shared database is the
    /// only thing standing between the domains it serves and a peer's sweep.
    ///
    /// The virtual guard reads `DomainStats::kind`, which is the index's own
    /// column and the only workable source (an orphan is by definition absent
    /// from the configuration). `DomainKind::from_stored` resolves an
    /// unrecognized string to `File`, which for a caller that deletes is the
    /// unsafe direction; it is unreachable while both backends pin the column
    /// `NOT NULL DEFAULT 'file'`, and this is the caller that would notice
    /// first if that ever changed.
    ///
    /// `dry_run` writes nothing whatsoever - no removal and no stamp - and
    /// reports the same set a real run would collect, on both paths. It is the
    /// only argument that silences the stamp.
    ///
    /// The domain row itself always stays, exactly as `domain_remove` leaves
    /// it, so nothing downstream sees a dangling reference. The routing cache
    /// is deliberately not refreshed: it is built from registered domains, and
    /// nothing touched here is one.
    ///
    /// The report:
    ///
    /// ```json
    /// {
    ///   "grace_seconds": 604800,
    ///   "on_demand": false,
    ///   "dry_run": false,
    ///   "read_only": false,
    ///   "stamped": 2,
    ///   "considered": [
    ///     { "domain": "gone", "kind": "file", "engrams": 30,
    ///       "last_registered": "2026-09-01T09:00:00+00:00",
    ///       "age_seconds": 1123200, "age_days": 13, "collected": true },
    ///     { "domain": "vault", "kind": "virtual", "engrams": 12,
    ///       "last_registered": null, "age_seconds": null, "age_days": null,
    ///       "collected": false, "kept": "virtual",
    ///       "reason": "a virtual domain's engram rows are its only copy ..." }
    ///   ],
    ///   "collected": ["gone"],
    ///   "engrams_removed": 30
    /// }
    /// ```
    ///
    /// `considered` holds one row per unregistered domain the index knows - a
    /// registered one is not a candidate and never appears. A row that was kept
    /// carries `kept`, one of `virtual`, `hosted_elsewhere`, `no_rows`,
    /// `grace`, `unstamped` or `read_only`, for a caller that branches on it,
    /// and a `reason` in words
    /// for one that prints. `grace_seconds` is `null` when a person asked, and
    /// `on_demand` says the same thing as a boolean. `skipped` is present only
    /// when the whole sweep declined to collect.
    pub async fn collect_orphaned_domains(
        &self,
        grace: Option<Duration>,
        dry_run: bool,
    ) -> Result<Value> {
        let now = Utc::now();
        // A dry run writes nothing at all: not a removal, and not a stamp
        // either, so a preview cannot move a clock the caller is only asking
        // about. A read-only instance is the other way round: it stamps and it
        // never removes. Stamping is index maintenance, which read-only mode
        // does not gate (see the field's own comment, and the host-lock rows a
        // read-only instance already writes) - and on a shared database it is
        // the only defence a read-only peer has for the domains it registers,
        // since another instance's sweep ages them out otherwise.
        let stamps = !dry_run;
        let removes = !dry_run && !self.read_only;

        let Some(registered) = self.registered_domain_names_checked() else {
            return Ok(json!({
                "grace_seconds": grace.map(|g| g.num_seconds()),
                "on_demand": grace.is_none(),
                "dry_run": dry_run,
                "read_only": self.read_only,
                "stamped": 0,
                "considered": [],
                "collected": [],
                "engrams_removed": 0,
                "skipped": "the configuration could not be read, and a domain cannot be shown \
                            absent from a file nobody can read; nothing was stamped and nothing \
                            collected",
            }));
        };

        // The registered set is stamped FIRST, before a single domain is
        // considered. A registered domain that went unstamped would age like
        // a removed one, and for a caller that collects on the stamp that is
        // data loss.
        let stamped = if stamps {
            let names: Vec<&str> = registered.iter().map(String::as_str).collect();
            let store = self.store.lock().await;
            store.stamp_registered(&names, &now.to_rfc3339()).await?;
            names.len()
        } else {
            0
        };

        let stats = {
            let store = self.store.lock().await;
            store.domain_stats().await?
        };

        let mut considered: Vec<Value> = Vec::new();
        let mut collected: Vec<String> = Vec::new();
        let mut engrams_removed: i64 = 0;
        // Unregistered domains that have never been stamped. They are not in
        // the registered set, so the call above cannot reach them, and without
        // a second one they would read `None` forever and never age at all -
        // which would leave every row an upgrade inherits immortal. This is a
        // clock starting, not a claim that they are registered.
        let mut start_clock: Vec<String> = Vec::new();

        // How many drafts the journal mirrors for one domain. `None` from the
        // state directory reads as nothing mirrored, which is the narrow answer:
        // a sweep that cannot see the journal keeps a domain rather than
        // collecting one.
        let journal_dir = self.journal_state_dir().ok();
        let mirrored = |name: &str| -> (u64, bool) {
            match journal_dir.as_deref() {
                Some(dir) => {
                    let counts = crate::overlay_journal::journal_counts(dir, name);
                    (counts.total, counts.unreadable)
                }
                // No state directory at all is the narrowest answer there is:
                // nothing counted, and nothing known either.
                None => (0, true),
            }
        };

        let mut drafts_swept: u64 = 0;
        for row in stats.iter().filter(|d| !registered.contains(&d.name)) {
            // Once per row, not once per use: this is a directory walk.
            let (drafts, drafts_unknown) = mirrored(&row.name);
            let age = row
                .last_registered
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|seen| now.signed_duration_since(seen.with_timezone(&Utc)));

            // Why this domain was kept, as a word a caller can branch on and a
            // sentence one can print. `None` is the only outcome that deletes.
            // Virtual is tested before read-only, and the order is the
            // message rather than the outcome: neither collects anything. A
            // read-only instance that reported `read_only` for a virtual
            // domain would have the doctor print "this instance is read-only",
            // which is true and useless - the rows are the domain's only copy
            // on every instance, and `domain remove --purge` is the one way
            // out wherever the reader is standing.
            let kept: Option<(&str, &str)> = if matches!(row.kind, DomainKind::Virtual) {
                Some((
                    "virtual",
                    "a virtual domain's engram rows are its only copy; end it with \
                     'domain remove --purge', which asks first",
                ))
            } else if self.read_only {
                Some(("read_only", "this instance is read-only"))
            } else if self.hosted_elsewhere(row, now) {
                Some((
                    "hosted_elsewhere",
                    "another instance holds this domain's host lock and is still \
                     heartbeating; its registration is a registration",
                ))
            } else if row.engrams == 0 && drafts == 0 {
                // `DomainStats::engrams` counts base rows only, so a domain
                // holding nothing but one actor's private drafts reads as empty
                // here. It is not: it holds rows the removal would clear and a
                // mirror that would restore them, so it goes down the same age
                // ladder as any other domain rather than being kept forever as
                // having nothing to collect. Counted from the journal because
                // the row count is the one thing a candidate's id cannot be
                // resolved for without writing, and a dry run writes nothing.
                //
                // A journal that could not be read counts zero and lands here
                // too, and that is the safe direction rather than an accident:
                // this branch KEEPS the domain, so a sweep that cannot see what
                // is mirrored collects nothing instead of deleting rows it
                // cannot account for. The reason says which of the two it was.
                Some((
                    "no_rows",
                    if drafts_unknown {
                        "no engram rows to collect, and the overlay journal could not be read, \
                         so nothing here is collected until it can be"
                    } else {
                        "no engram rows to collect"
                    },
                ))
            } else {
                match grace {
                    // A person asking is the signal the grace period exists to
                    // wait for, so there is nothing left to wait for and no
                    // stamp to consult: an inherited index whose rows all read
                    // `None` clears on first contact rather than a week after.
                    None => None,
                    Some(grace) => match age {
                        None => {
                            start_clock.push(row.name.clone());
                            Some((
                                "unstamped",
                                if stamps {
                                    "never seen registered before; its clock starts now"
                                } else {
                                    "never seen registered before; a real run would start its \
                                     clock now"
                                },
                            ))
                        }
                        Some(age) if age < grace => Some(("grace", "within the grace period")),
                        Some(_) => None,
                    },
                }
            };

            let collect = kept.is_none();
            if collect {
                collected.push(row.name.clone());
                if removes {
                    // The id the way `domain_remove` resolves it, with the
                    // row's own path and kind so the upsert updates nothing:
                    // the domain row must come through this exactly as it
                    // went in.
                    let path = Some(row.path.as_str()).filter(|p| !p.is_empty());
                    let store = self.store.lock().await;
                    let id = store.upsert_domain(&row.name, path, row.kind).await?;
                    store.clear_domain(id).await?;
                    drop(store);
                    engrams_removed += row.engrams;
                    // This path never goes through `unregister_domain`, so the
                    // journal sweep is repeated here rather than inherited. A
                    // mirror left behind for a collected domain is worse than a
                    // leak: the drafts would come back on the next sync for a
                    // domain nobody registers.
                    drafts_swept += self.sweep_domain_journal(&row.name).await;
                    let age_text = match age {
                        Some(age) => format!("last seen registered {} days ago", age.num_days()),
                        None => "never seen registered".to_string(),
                    };
                    tracing::info!(
                        domain = row.name.as_str(),
                        engrams = row.engrams,
                        age = age_text.as_str(),
                        "collected {} engram rows of '{}', {}",
                        row.engrams,
                        row.name,
                        age_text
                    );
                }
            }

            let mut entry = json!({
                "domain": row.name,
                "kind": if matches!(row.kind, DomainKind::Virtual) { "virtual" } else { "file" },
                "engrams": row.engrams,
                // Beside the engram count rather than folded into it: they are
                // different knowledge. `engrams` is what the domain's files
                // say, `drafts` is what people hold privately on top of it, and
                // a domain can have none of the first and some of the second.
                "drafts": drafts,
                // The same honesty `drafts_unknown` carries on the removal
                // preview: a zero that nothing could confirm is not a zero.
                "drafts_unknown": drafts_unknown,
                "last_registered": row.last_registered,
                "age_seconds": age.map(|a| a.num_seconds()),
                "age_days": age.map(|a| a.num_days()),
                "collected": collect,
            });
            if let Some((kept, reason)) = kept {
                entry["kept"] = json!(kept);
                entry["reason"] = json!(reason);
            }
            considered.push(entry);
        }

        if stamps && !start_clock.is_empty() {
            let names: Vec<&str> = start_clock.iter().map(String::as_str).collect();
            let store = self.store.lock().await;
            store.stamp_registered(&names, &now.to_rfc3339()).await?;
        }

        let mut report = json!({
            "grace_seconds": grace.map(|g| g.num_seconds()),
            "on_demand": grace.is_none(),
            "dry_run": dry_run,
            "read_only": self.read_only,
            "stamped": stamped,
            "considered": considered,
            "collected": collected,
            "engrams_removed": engrams_removed,
            "drafts_swept": drafts_swept,
        });
        if self.read_only {
            // Each path says exactly what it did. A read-only instance stamps
            // and never removes, so a real run has stamped by the time this is
            // written - but a preview has written nothing at all, `stamped` is
            // zero on it, and a sentence claiming otherwise is the same lie in
            // the other direction.
            report["skipped"] = json!(if dry_run {
                "this instance is read-only; nothing was changed, and a real run here would \
                 stamp the registered domains and still remove nothing"
            } else {
                "this instance is read-only; the registered domains were stamped and nothing \
                 was removed"
            });
        }
        Ok(report)
    }
}
