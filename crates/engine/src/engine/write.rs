use super::*;

impl Engine {
    // --- write ---------------------------------------------------------------

    /// What a write says about a title that will not be read back the way it
    /// was written.
    ///
    /// Two shapes, both legal and both surprising. A `/` in a title slugifies
    /// into a path separator, so the engram lands nested under a folder the
    /// caller never named. A `:` parses as a cross-domain prefix, so
    /// `[[Murmur: the dispatch pipeline]]` resolves as a title only while no
    /// domain called `Murmur` is registered - and the day one is, every such
    /// link silently means something else. Neither is refused: the title is the
    /// author's. What the receipt owes them is the permalink that came out and
    /// a spelling they can paste.
    ///
    /// Both remedies are literals rather than the name of a parameter, because
    /// the reader is about to type one. And the colon's remedy is deliberately
    /// NOT a `crystalline://` address: [`crystalline_core::engram::LinkTarget`]
    /// splits a wikilink at its own first colon, so `[[crystalline://...]]`
    /// reads `crystalline` as the domain prefix and dangles - advice that would
    /// manufacture the very finding this notice exists to prevent. A permalink
    /// can never hold a colon, so the bare permalink and its `domain:permalink`
    /// twin are the two spellings that always mean what they say.
    ///
    /// Empty when the title holds neither, so the receipt gains no key at all
    /// in the common case.
    fn title_notices(title: &str, domain: &str, permalink: &str) -> Vec<String> {
        let mut notices = Vec::new();
        // The TITLE's own slug, not the permalink: the permalink is nested
        // whenever the caller passed a `folder`, which is the deliberate case
        // and earns no notice, and `slugify` drops a segment that contributes
        // nothing, so a title `TODO/` lands at `todo` and nests nothing.
        //
        // The folder named in the remedy comes off the permalink, so it carries
        // any folder the caller already passed and can be pasted as it stands.
        // The title comes off the TITLE, so the author reads their own words
        // back rather than the slug those words became - but off the last
        // segment that CONTRIBUTES a slug segment, not simply the last one.
        // Splitting keeps an empty, blank or punctuation-only tail that
        // `slugify` threw away, and naming it would hand back either an empty
        // pair of backticks or a title like `!!!` that re-slugifies to a
        // different address than the one this same sentence just quoted. The
        // search cannot come up empty: the guard has already proved the title
        // holds at least two slug-contributing segments.
        if slugify(title).contains('/')
            && let Some((folder, _)) = permalink.rsplit_once('/')
            && let Some(leaf) = title
                .rsplit('/')
                .map(str::trim)
                .find(|segment| !slugify(segment).is_empty())
        {
            notices.push(format!(
                "the title holds a `/`, so this engram landed at the nested permalink \
                 `{permalink}`. To place an engram in a folder on purpose, pass the folder as \
                 its own argument - `folder: \"{folder}\"` over MCP or REST, `--folder {folder}` \
                 on the command line - with the title `{leaf}` and no slash in it."
            ));
        }
        // The three conditions `LinkTarget::parse` applies before it reads a
        // prefix as a domain, spelled the same way here: a title this function
        // stays quiet about is one no link would ever split.
        if let Some((prefix, rest)) = title.split_once(':')
            && !prefix.trim().is_empty()
            && !rest.trim().is_empty()
            && !prefix.trim().contains(char::is_whitespace)
        {
            notices.push(format!(
                "the title holds a `:`, so a link written as `[[{title}]]` reads `{}` as a domain \
                 prefix. It resolves to this engram while no domain of that name is registered, \
                 and stops the day one is. The form that always works is the bare permalink \
                 `[[{permalink}]]` inside {domain}, or `[[{domain}:{permalink}]]` from another \
                 domain: a permalink never holds a colon, so neither is ever read as a prefix.",
                prefix.trim()
            ));
        }
        notices
    }

    /// Where an engram titled `title` under `folder` would be written: the
    /// domain-relative path and the permalink it will answer to.
    ///
    /// One function because two verbs create engrams - [`Engine::write_engram`]
    /// and [`Engine::split_engram`] - and a second copy of these screens is a
    /// second chance to leave one of them out.
    fn engram_destination(folder: Option<&str>, title: &str) -> Result<(String, String)> {
        let folder = folder.map(str::to_string).unwrap_or_default();
        let title_slug = slugify(title);
        if title_slug.is_empty() {
            return Err(EngineError::Invalid(
                "title does not slugify to a permalink; provide a title with letters or digits"
                    .into(),
            ));
        }
        let folder = normalize_rel(&folder);
        let rel = if folder.is_empty() {
            format!("{title_slug}.md")
        } else {
            format!("{folder}/{title_slug}.md")
        };
        // Screened before either reserved check, and before `join_rel` ever
        // sees it: `join_rel` pushes segment by segment, `..` included, so an
        // unscreened folder both places the file outside the domain root and
        // hides the destination from a textual reserved check - `a/../assets`
        // is neither `assets` nor `assets/...` as a string, yet it lands
        // exactly there on disk.
        if !is_within_domain(&rel) {
            return Err(EngineError::Invalid(escapes_root_error(&rel)));
        }
        if crystalline_core::is_reserved_path(&rel) {
            return Err(EngineError::Invalid(reserved_name_error(&rel)));
        }
        // The other reserved shape: the folder attachments live in. Checked on
        // the joined path, so a `folder` of `assets`, `/assets/`, `Assets` or
        // `assets/deep` is refused whichever spelling arrived.
        if is_assets_reserved(&rel) {
            return Err(EngineError::Invalid(assets_reserved_error(&rel)));
        }
        let permalink = slugify(&rel);
        Ok((rel, permalink))
    }

    /// Whether `permalink` already answers for somebody at another path than
    /// `rel`, from this writer's own point of view: the writer's own shadowed
    /// view in review mode (a name another actor is drafting under is free,
    /// and a path this writer has tombstoned is free again), the plain index
    /// otherwise. `None` means free; `Some(path)` names where it is taken.
    ///
    /// Shared by [`Engine::write_engram_present`]'s two collision checks - the
    /// early, unlocked one that answers before `build_markdown` can raise a
    /// different error, and the later one under the write lock that is the
    /// actual race guard - so the two can never drift into two readings of
    /// "taken".
    async fn permalink_taken(
        &self,
        domain: &str,
        rel: &str,
        permalink: &str,
        overlay_draft: Option<(&str, DomainId)>,
    ) -> Result<Option<String>> {
        let store = self.store.lock().await;
        Ok(match overlay_draft {
            Some((actor, domain_id)) => match store.overlay_entry(domain_id, actor, rel).await? {
                Some(entry) if entry.tombstone => None,
                Some(entry) => Some(entry.path),
                None => store
                    .find_engram(domain, permalink)
                    .await?
                    .map(|existing| existing.path),
            },
            None => store
                .find_engram(domain, permalink)
                .await?
                .map(|existing| existing.path),
        })
    }

    /// Create or overwrite an engram, then index it. A file domain writes the
    /// markdown file first (files-are-truth) then reindexes it from disk; a
    /// virtual domain builds the markdown in memory and indexes it straight into
    /// the database, touching no filesystem.
    pub async fn write_engram(&self, p: &WriteParams) -> Result<Value> {
        self.write_engram_as(p, None, &crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`Engine::write_engram`] with the writer's identity: `client` is the
    /// caller's own idea of who is writing (an MCP client's
    /// `clientname/version` from the initialize handshake, or the CLI's process
    /// actor), which [`Engine::actor`] resolves against the `identity.actor`
    /// setting before it lands in the engram's `generated.by`.
    ///
    /// `scope` is who is acting, resolved once by whichever surface took the
    /// request (see [`crate::scope::Scope`]). Every write verb carries it, so
    /// that routing a write to one actor's own draft overlay has a single
    /// place to happen rather than one per surface. No verb consults it yet:
    /// the wrappers with no `_as` suffix pass
    /// [`Unrestricted`](crate::scope::Scope::Unrestricted), which is what the
    /// CLI and the control socket are, and what every caller acted with before
    /// the parameter existed.
    pub async fn write_engram_as(
        &self,
        p: &WriteParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.write_engram_joined(p, client, scope, None).await
    }

    /// [`Engine::write_engram_as`], with the join a session may be holding.
    ///
    /// A capture inside a join replaces the granted page with the document the
    /// caller composed, which is the one thing this verb can do inside
    /// somebody's draft: its destination is derived from the title rather than
    /// resolved from a page, so a capture that lands anywhere else is a
    /// capture of the caller's own and has nothing to do with the draft.
    ///
    /// **The path screen therefore runs only when a join actually took**, and
    /// the asymmetry with [`Engine::save_engram_joined`] is deliberate. An
    /// UNJOINED capture at a granted path is the second way forward
    /// [`granted_needs_join`] names out loud - draft your own copy and leave
    /// theirs as it stands - and that is what it has always done here.
    /// Screening it would take that fork away.
    pub async fn write_engram_joined(
        &self,
        p: &WriteParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<Value> {
        self.write_engram_present(p, client, scope, join, None)
            .await
    }

    /// [`Engine::write_engram_joined`], with the agent as a named peer in the
    /// room the capture may land in.
    ///
    /// The bottom rung, and the only one that knows about the strip - the same
    /// shape [`Engine::edit_engram_present`] has, for the same reason: the room
    /// is keyed on the overlay owner, and who that is - this actor's own draft,
    /// the author's draft they were invited into, or the document a direct
    /// domain keeps - is the view's answer and is resolved here.
    ///
    /// `None` is every surface that is not an agent working for somebody: the
    /// CLI, the control socket, the JSON API.
    pub async fn write_engram_present(
        &self,
        p: &WriteParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
        peer: Option<&AgentPeer>,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let source = self.content_source(&p.domain)?;
        let view = DomainView::for_write_joined(self, &p.domain, scope, join).await?;
        let overlay = view.actor();
        let join = join.filter(|_| view.joined().is_some());
        let actor = self.actor_for(client, overlay);
        let engram_type = p
            .engram_type
            .clone()
            .unwrap_or_else(|| "engram".to_string());
        let status = p.status.clone().unwrap_or_else(|| "stable".to_string());
        let tags = p.tags.clone();

        let (rel, permalink) = Self::engram_destination(p.folder.as_deref(), &p.title)?;

        // A file domain's MANIFEST is never a capture's destination, in any
        // letter case. `slugify` lowercases, so a title of "MANIFEST" lands at
        // `manifest.md`, which a case-insensitive filesystem (the macOS and
        // Windows defaults) opens as the very `MANIFEST.md` routing reads: an
        // overwriting write replaced it with an ordinary engram and only then
        // failed on the permalink check, too late to keep the file. There a
        // MANIFEST changes through edit_engram. A virtual domain has no
        // filesystem to fold the case, and a capture titled after its MANIFEST
        // is how one is written there (Ruling I2), so it is left alone.
        if rel.eq_ignore_ascii_case("MANIFEST.md")
            && matches!(self.read_source(&p.domain), ContentSource::File { .. })
        {
            return Err(EngineError::Invalid(
                "a new engram cannot be written at the domain root as MANIFEST.md: in this domain that file is the MANIFEST, which routing reads. Change it with edit_engram, or pick another title or a folder".into(),
            ));
        }

        // A join is into ONE draft: a capture inside one that resolved
        // anywhere else has nowhere to land, and is told so rather than
        // writing into the owner's overlay at a path they never shared.
        if let Some(join) = join {
            self.screen_granted_path(&p.domain, &rel, scope, Some(join))
                .await?;
        }

        // The domain's index id, resolved once here rather than once per
        // collision check below: `overlay` and its domain id can never
        // diverge, so the pair is paired in the type rather than re-paired at
        // every read.
        let overlay_draft = match overlay {
            Some(actor) => Some((actor, self.domain_source(&p.domain).await?.0)),
            None => None,
        };

        // **Ahead of `build_markdown`, deliberately.** A malformed capture at
        // a permalink that is already taken answers "permalink already
        // exists", the more useful of the two messages and the one this verb
        // gave before `build_markdown` was hoisted ahead of the collision
        // check: `build_markdown` can refuse on unbuildable content (a bad
        // date, a malformed `verified` entry), and that refusal must not hide
        // a plainer one this writer could already act on. Unlocked and
        // best-effort - the actual race guard is the second, locked check
        // below, right before the write it authorizes - so a permalink freed
        // or taken between the two still gets the correct, authoritative
        // answer there.
        if !p.overwrite
            && let Some(at) = self
                .permalink_taken(&p.domain, &rel, &permalink, overlay_draft)
                .await?
        {
            return Err(EngineError::Conflict(format!(
                "permalink '{permalink}' already exists in domain '{}' (at {at}); pass overwrite=true to replace",
                p.domain
            )));
        }

        // The document this capture would land, built before any file lock is
        // taken because nothing about it needs one: it is the caller's own
        // arguments plus this instant, and a call that cannot produce a
        // well-formed engram is better refused with no lock in hand.
        let today = chrono::Utc::now().date_naive();
        let now = now_offset();
        // The model the agent reported, held against the actor this write
        // records: a person's write never carries one (`stamped_model`).
        let model = stamped_model(&actor, p.model.as_deref());
        let markdown = build_markdown(
            &engram_type,
            &p.title,
            &permalink,
            &tags,
            &status,
            &today.format("%Y-%m-%d").to_string(),
            &actor,
            model.as_deref(),
            now,
            p.metadata.as_ref(),
            &p.content,
        )?;

        let mut receipt = json!({
            "domain": p.domain,
            "permalink": permalink,
            "path": rel,
            "title": p.title,
            "type": engram_type,
            "status": status,
            "action": if p.overwrite { "written" } else { "created" },
        });
        // Attached here rather than at the end: `write_engram_as` leaves by
        // three exits - the live-room arm, the draft arm and the base arm -
        // and a title reads back the same way whichever one a write took.
        let notices = Self::title_notices(&p.title, &p.domain, &permalink);
        if !notices.is_empty() {
            receipt["notices"] = json!(notices);
        }

        // **The live arm, and it stands ahead of every arm that writes**, the
        // way the edit's does (`Engine::apply_source_edit_staged`). While a
        // co-editing room is open over this permalink the room's text IS the
        // engram: somebody has it on screen, the file and the row are both a
        // save behind, and a capture that replaced the file would be replaced
        // right back by the room's own saver a moment later - with the
        // person's unsaved work gone and nothing to say where it went. So the
        // document is morphed to what this capture would have written, its
        // history and their cursor are kept, and their session is what makes
        // it durable.
        //
        // **Ahead of the FILE LOCK as well, and that ordering is the whole of
        // a deadlock.** The room's saver takes the two the other way round: it
        // holds the session state lock across `Engine::save_engram`, which
        // takes this same path's write lock. An arm that composed into the
        // room while holding the file lock would close a cycle with a save
        // already in flight, and neither side times out - the file lock would
        // be held for ever, so every later write, edit, save and delete of
        // that engram would hang and the unsaved work would never land. The
        // edit's live arm keeps the same discipline by standing above the arms
        // that lock; `no_engine_function_composes_into_a_room_under_a_file_write_lock`
        // pins it for both.
        //
        // **`overwrite` is this arm's own precondition, not the collision
        // check's.** Replacing a whole document somebody is looking at is only
        // ever what a replacement may do, and the check below would not say so:
        // an engram deleted while its room is still open leaves the permalink
        // free - the room learns of the deletion on its next save - so a plain
        // capture would pass the check and morph the open page into a brand
        // new engram with a receipt saying "created". A capture that never
        // asked to replace anything takes the ordinary create path beside the
        // room instead. Nothing is silently clobbered once it does: the room's
        // next save still carries the checksum of the version it read, the CAS
        // against the file this capture just wrote refuses, and the save falls
        // into the external-change path instead (`raise_deleted` when the
        // engram is absent, `merge_external` when it is present - which it now
        // is) - so the person is shown a merge or a conflict against the
        // capture's engram, never an overwrite of it.
        if p.overwrite
            && let Some(rooms) = self.collab_rooms()
            && rooms.has_live_room(&p.domain, &permalink, overlay).await
        {
            // **The one arm on which a capture can replace a MANIFEST**, so
            // the one that reads it. A capture's destination is a slug, which
            // is lowercase, so it never names `MANIFEST.md` itself - but it
            // slugs to the MANIFEST's own permalink, and the room open over
            // that permalink IS the MANIFEST's. What lands there is this
            // capture's document, and the rules are owed their say about it
            // exactly as an edit of the MANIFEST gets it.
            let manifest_text = rel
                .eq_ignore_ascii_case("MANIFEST.md")
                .then(|| markdown.clone());
            let applied = rooms
                .apply_text(&p.domain, &permalink, overlay, markdown, &actor, peer)
                .await
                .map_err(EngineError::Conflict)?;
            if let Some(text) = manifest_text {
                note_manifest_findings(&mut receipt, &text, domain_verify_config(&source).as_ref());
            }
            if overlay.is_some() {
                receipt["draft"] = json!(true);
            }
            if let Some(owner) = view.joined() {
                receipt["joined"] = json!(format!("landed in {owner}'s draft"));
            }
            // Where it went, in the words the live edit says it in: `present`
            // names who is about to watch the page change under them.
            receipt["landed"] = json!("live");
            receipt["present"] = json!(applied.participants);
            // The tails, and which of them this arm owes: none of them. The
            // generated folder indexes, the embedding nudge and - for a
            // virtual domain whose MANIFEST is the one engram whose text is
            // also configuration - the routing cache all describe bytes that
            // are nowhere yet, so all three belong to the room's saver, which
            // runs them when the text lands (`Engine::save_engram`), pinned
            // end to end by
            // `a_virtual_manifest_replaced_in_its_room_reaches_the_routing_cache`.
            return Ok(receipt);
        }

        // The whole existence-check-then-write under one lock: the check and
        // the write it authorizes must be one step, or two creates of one title
        // both find the permalink free, both write, and the second answers
        // "created" over the first's body instead of the conflict that says the
        // name was taken. Which lock depends on where this write lands - the
        // draft's mirror when it lands in an overlay, the file's own path when
        // it lands in the folder - because a draft's writers contend with each
        // other and not with the file nobody is writing. See
        // `Engine::draft_lock` and `Engine::write_lock`. Taken before the store
        // lock, like every other holder.
        let write_lock = match overlay_draft {
            Some((who, _)) => Some(self.draft_lock(&p.domain, who, &rel)?),
            None => match &source {
                ContentSource::File { root } => Some(self.write_lock(&join_rel(root, &rel))),
                ContentSource::Virtual => None,
            },
        };
        let _guard = match &write_lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };

        // Enforce overwrite semantics against what this writer can see there,
        // one more time, now under the write lock: the check above is
        // unlocked and answers only for the nicer message ahead of
        // `build_markdown`, so two concurrent creates of one title racing
        // each other past it must still be caught HERE, atomically with the
        // write below - `overlay_draft` was resolved once, above, and is
        // reused rather than re-paired.
        if !p.overwrite
            && let Some(at) = self
                .permalink_taken(&p.domain, &rel, &permalink, overlay_draft)
                .await?
        {
            return Err(EngineError::Conflict(format!(
                "permalink '{permalink}' already exists in domain '{}' (at {at}); pass overwrite=true to replace",
                p.domain
            )));
        }

        // The third place a write can land, and the reason it comes first: on a
        // domain in review mode the folder and the database both stay as the
        // team left them, so neither arm below may run.
        if let Some((_, domain_id)) = overlay_draft {
            let warning = view.write(domain_id, &rel, &markdown).await?;
            receipt["draft"] = json!(true);
            // Whose draft it landed in, when that is not the caller's own, in
            // the words the joined save and the joined edit both use.
            if let Some(owner) = view.joined() {
                receipt["joined"] = json!(format!("landed in {owner}'s draft"));
            }
            note_unmirrored(&mut receipt, warning);
            return Ok(receipt);
        }

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &rel);
                write_file(&abs, &markdown)?;
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(&p.domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                self.reindex_file(&*store, domain_id, root, &rel).await?;
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(&p.domain, None, DomainKind::Virtual)
                    .await?;
                let stamp = virtual_stamp(&markdown);
                self.index_markdown(&*store, domain_id, &rel, &markdown, stamp, None, true)
                    .await?;
            }
        }

        // A virtual write may have landed or replaced this domain's MANIFEST
        // engram, the source of its routing bullets, so refresh the cache the
        // sync `routing_text` reads. The store locks above are all released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        // The new engram belongs in its folder's generated index.
        self.refresh_index_files(&p.domain).await;
        self.nudge_embed();

        Ok(receipt)
    }

    /// Whether a capture would replace a document somebody has open, and who
    /// is in there: the two things `write_engram` needs before it asks.
    ///
    /// Asked of the engine rather than worked out by the caller because both
    /// halves of the room key are the engine's - the permalink is derived from
    /// the title exactly as the write derives it, and the overlay owner is the
    /// view's answer, so a draft's room and the team's room over one name are
    /// never confused for each other.
    ///
    /// `None` when no room is open over the destination, which is nearly every
    /// capture, and for a destination this caller cannot resolve at all - a
    /// domain nobody registered, a reserved title: there is no document to ask
    /// about, and the write itself raises the real error a moment later rather
    /// than having it guessed at here.
    ///
    /// The agent stands in the room while the question is put, the way a read
    /// of a live document stands it there: somebody deciding whether to let
    /// their page be replaced is owed the name of who is asking. Its own slot
    /// is left out of the names, which answer "who is in there with you". The
    /// chip stands even when the write is then refused or never confirmed, and
    /// that is the true thing to draw: an agent that reached for somebody's
    /// open page was in there, whatever came of it.
    ///
    /// A read-only instance answers `None` rather than a question: the write
    /// refuses with [`EngineError::ReadOnly`] a moment later, and asking
    /// somebody to authorize what the server will refuse anyway is a question
    /// in the wrong words.
    ///
    /// **This answers for a REPLACEMENT, and says nothing about whether the
    /// call in front of it is one.** [`Engine::write_engram_present`]'s live
    /// arm only takes a found room when `p.overwrite` is also true - that is
    /// the arm's own precondition, not a room-existence question - and this
    /// preview tests nothing of the kind. `mcp.rs` asks it for any capture
    /// that resolves onto an open document, wholesale or not, and a yes
    /// becomes `overwrite = true` on the call that lands, which is what makes
    /// the arm's precondition hold; a caller that took a yes without setting
    /// `overwrite` would be handed a question about a capture that never
    /// takes the live arm.
    pub async fn live_write_target(
        &self,
        p: &WriteParams,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
        peer: Option<&AgentPeer>,
    ) -> Option<LiveWriteTarget> {
        if self.read_only {
            return None;
        }
        let rooms = self.collab_rooms()?;
        let view = DomainView::for_write_joined(self, &p.domain, scope, join)
            .await
            .ok()?;
        let overlay = view.actor();
        let (rel, permalink) = Self::engram_destination(p.folder.as_deref(), &p.title).ok()?;
        // The same screen the write runs, run before anybody is named: a
        // grantee working inside one draft who aims a capture at another path
        // in the owner's overlay is refused by the write, and a question that
        // named who is in the room over that page would have disclosed it
        // ahead of the gate that refuses.
        if let Some(join) = join.filter(|_| view.joined().is_some()) {
            self.screen_granted_path(&p.domain, &rel, scope, Some(join))
                .await
                .ok()?;
        }
        if !rooms.has_live_room(&p.domain, &permalink, overlay).await {
            return None;
        }
        let mine = match peer {
            Some(peer) => {
                rooms
                    .touch_agent_presence(&p.domain, &permalink, overlay, peer)
                    .await
            }
            None => None,
        };
        let present = rooms
            .participants(&p.domain, &permalink, overlay, mine)
            .await;
        Some(LiveWriteTarget { permalink, present })
    }

    /// Save an engram's complete markdown text verbatim, guarded by the
    /// checksum of the version the caller read.
    ///
    /// The full-document counterpart of [`Engine::edit_engram`], for the HTTP
    /// PUT: the client edited the whole file, so the whole file is what lands.
    /// Nothing is rebuilt and `generated` is not touched - a save of what was
    /// read must be byte-identical, which is the editor's fidelity contract,
    /// and the text already carries whatever provenance its author put there.
    ///
    /// `expected_checksum` is enforced on BOTH storage kinds. File domains get
    /// the comparison here (read, hash, compare, write), virtual domains get
    /// it in the store's compare-and-swap (`upsert_engram_checked`); both
    /// failure paths speak the store's own "stale edit" language so the HTTP
    /// layer classifies them as one conflict.
    ///
    /// The `permalink` in the receipt is the one the engram answers to *after*
    /// the write, which is not always the one it was addressed by: writing the
    /// document verbatim means an author may have edited the `permalink` line
    /// in the frontmatter, and the index takes the permalink from the file. A
    /// caller that saved a rename is told where its engram went.
    ///
    /// `scope` is the acting scope every write verb carries; see
    /// [`Engine::write_engram_as`].
    pub async fn save_engram(&self, p: &SaveParams, scope: &crate::scope::Scope) -> Result<Value> {
        self.save_engram_joined(p, scope, None).await
    }

    /// [`Engine::save_engram`], with the join a session may be holding.
    ///
    /// The same verb, and the join is the only difference: a caller working
    /// inside somebody else's draft (see [`crate::join`]) saves into the
    /// OWNER's overlay rather than into their own, and the receipt says whose
    /// draft it landed in. `None` is the ordinary save, which is what
    /// [`Engine::save_engram`] passes and what every surface but the HTTP one
    /// has.
    ///
    /// Two gates ride here rather than in the routes, so a second surface that
    /// learns to join inherits both. A join is into ONE draft, so a save
    /// addressed at anything else is refused rather than landing in the
    /// owner's overlay at a path they never shared. And a save at a path this
    /// caller was GRANTED but has not joined is refused too, in words that
    /// name both ways forward: visibility and editing are two states, and a
    /// save that silently forked the grantee's own copy would have decided
    /// that for them.
    pub async fn save_engram_joined(
        &self,
        p: &SaveParams,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write_joined(self, &p.domain, scope, join).await?;
        let overlay = view.actor();
        // The join as this view actually took it: one naming another domain,
        // or a domain that has stopped reviewing changes, is not a join into
        // this write at all and must not gate it.
        let join = join.filter(|_| view.joined().is_some());
        refuse_not_an_engram(&p.content)?;
        let (desc, source) = match view.resolve(&p.identifier).await {
            Ok(resolved) => resolved,
            // A name this caller's own view cannot resolve, when they are
            // holding a link to a draft that answers to it: the miss IS the
            // rule, since the granted draft is deliberately absent from every
            // ordinary read they make, and answering "no such engram" to
            // somebody who was handed that very page to read would be true and
            // useless. So the refusal teaches instead, in the same words a
            // save at a path they CAN resolve gets. Nothing about the draft is
            // revealed that the link did not already hand over.
            Err(EngineError::NotFound(missing)) => {
                return match self
                    .teach_granted_miss(&p.domain, &p.identifier, scope)
                    .await?
                {
                    Some(teaching) => Err(EngineError::Refused(teaching)),
                    None => Err(EngineError::NotFound(missing)),
                };
            }
            Err(e) => return Err(e),
        };
        // A reserved name never resolves to an engram today (sync skips both),
        // so this is defence in depth rather than a reachable branch: the
        // generated `index.md` is derived from its folder and would be
        // overwritten on the next refresh, and `log.md` is reserved beside it.
        // Checked on the resolved path, which is the authority on what would
        // actually be written.
        if crystalline_core::is_reserved_path(&desc.path) {
            return Err(EngineError::Invalid(reserved_name_error(&desc.path)));
        }
        // Defence in depth for the same reason: the walk never indexes
        // anything under `assets/`, so a resolved engram cannot sit there
        // today, and a row left over from before the prefix was reserved must
        // not become a way to write into the attachment folder.
        if is_assets_reserved(&desc.path) {
            return Err(EngineError::Invalid(assets_reserved_error(&desc.path)));
        }
        self.screen_granted_path(&desc.domain, &desc.path, scope, join)
            .await?;

        // The third place a save can land: this actor's own draft.
        if overlay.is_some() {
            return self.save_into_overlay(&view, p, &desc, &source).await;
        }

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                // Held across the comparison and the write. See
                // `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                let current = match std::fs::read_to_string(&abs) {
                    Ok(text) => text,
                    // Indexed but absent from this machine's disk: the file was
                    // removed behind the index, or this instance is not the
                    // domain's host and only ever saw the database rows. Either
                    // way the engram the caller asked to save is not here to
                    // save, which is a miss rather than a server fault - the
                    // same reading `save_manifest` takes of its own file.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(format!(
                            "engram '{}' in domain '{}' has no file at {}",
                            desc.permalink,
                            desc.domain,
                            abs.display()
                        )));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                let found = sha256_hex(current.as_bytes());
                if found != p.expected_checksum {
                    return Err(EngineError::Conflict(stale_edit_message(
                        &p.expected_checksum,
                        &found,
                    )));
                }
                write_file(&abs, &p.content)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, desc.domain_id, root, &desc.path)
                    .await?;
            }
            ContentSource::Virtual => {
                let stamp = virtual_stamp(&p.content);
                let store = self.store.lock().await;
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    &p.content,
                    stamp,
                    Some(&p.expected_checksum),
                    true,
                )
                .await?;
            }
        }

        // Where the engram now answers. Read back after the reindex rather than
        // echoed from the resolution that preceded it: the index takes an
        // engram's permalink from its frontmatter, so an author who edited that
        // line has just moved the address, and a receipt naming the old one
        // would send its caller to a permalink nothing resolves. The saved
        // content is the truth here, so the truth is what is asked. Asked
        // tolerantly, though: the save is committed by this line, so neither a
        // missing row nor a failing lookup may turn it into a reported error -
        // see [`receipt_permalink`], which answers both with the resolved name.
        let permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(&desc.domain, Some(&desc.path), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == desc.path)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, desc.permalink.clone())
        };

        // A save can rewrite the MANIFEST engram of a virtual domain or the
        // titles a folder index lists, same as an edit.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        self.refresh_index_files(&desc.domain).await;
        self.nudge_embed();

        Ok(json!({
            "domain": desc.domain,
            "permalink": permalink,
            "path": desc.path,
            "checksum": sha256_hex(p.content.as_bytes()),
        }))
    }

    /// Save a co-editing room's text into the overlay document the room is a
    /// room over.
    ///
    /// The room's counterpart of [`Engine::save_engram`], and the difference
    /// between them is where the actor comes from. A request carries a scope
    /// and the write is routed from it; a room carries a view it built from
    /// the key it was opened under, which names the owner whose draft this
    /// room IS. Everything after that is the same write, through the same
    /// function, with the same compare-and-swap.
    ///
    /// **`expected_path` is the path the room is a room over, and a save that
    /// resolves anywhere else is refused.** A room addresses its saves by the
    /// permalink its own text carries, and that line is typed by whoever is in
    /// the room, so without this screen a person invited into one page holds a
    /// write over every page in its author's overlay: the address ladder
    /// answers a TITLE as well as a permalink
    /// ([`DomainView::resolve_draft`], and `find_engram` on the base side),
    /// while the address check in front of a draft write deliberately does not
    /// treat a title as an address - so a document that renames its own
    /// permalink to another engram's title stands at the granted path and
    /// resolves to the other one from the next save on. This is the room's
    /// counterpart of [`Engine::screen_granted_path`], which refuses the same
    /// move for a request carrying a join, and it speaks the same sentence.
    ///
    /// The refusal is an ordinary save refusal: the room stays open, the text
    /// stays in the document, and the author is told why - which is the right
    /// outcome for "this document now claims to be a different page".
    ///
    /// One gate a request meets is deliberately absent, because it is not
    /// about this caller: the teaching refusal for a name only a share-link
    /// resolves (a room resolves through the owner's own view, where the page
    /// is simply there). The grant-and-join decision is the door's, made once
    /// at the upgrade rather than four times a second - and the screen below
    /// is what keeps that decision true for the life of the room.
    ///
    /// The receipt is the overlay one: `draft: true`, and the permalink the
    /// draft answers to after the write, which an author who edited the
    /// frontmatter line has just moved.
    pub(crate) async fn save_engram_in_overlay(
        &self,
        view: &DomainView<'_>,
        p: &SaveParams,
        expected_path: &str,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        refuse_not_an_engram(&p.content)?;
        let (desc, source) = view.resolve(&p.identifier).await?;
        if desc.path != expected_path {
            return Err(EngineError::Refused(joined_write_is_elsewhere(
                view.writing_actor()?,
                expected_path,
                &desc.path,
            )));
        }
        // The same two reserved screens the request-driven save makes, on the
        // resolved path, which is the authority on what would be written.
        if crystalline_core::is_reserved_path(&desc.path) {
            return Err(EngineError::Invalid(reserved_name_error(&desc.path)));
        }
        if is_assets_reserved(&desc.path) {
            return Err(EngineError::Invalid(assets_reserved_error(&desc.path)));
        }
        self.save_into_overlay(view, p, &desc, &source).await
    }

    /// The overlay arm of a save: the whole document into the view's own
    /// actor's draft of `desc.path`, verbatim, checked against the version the
    /// caller read - which in review mode is their own draft where they hold
    /// one, so a second save does not conflict against the first.
    ///
    /// One function rather than one per surface, because the CAS under the
    /// mirror lock, the permalink re-derivation and the receipt are one
    /// agreement about what landing in a draft means. Its callers are the
    /// request-driven save above and the co-editing room's saver, which reaches
    /// it with a view it built itself over the owner of the document the room
    /// is a room over.
    ///
    /// Everything a caller must decide BEFORE this is deliberately not here:
    /// whose view it is, whether the document parses, whether the path is
    /// writable, and - for a request - whether a grant or a join routes it.
    /// This function writes.
    async fn save_into_overlay(
        &self,
        view: &DomainView<'_>,
        p: &SaveParams,
        desc: &EngramDescriptor,
        source: &ContentSource,
    ) -> Result<Value> {
        let who = view.writing_actor()?;
        // Written directly rather than through `apply_source_edit`, and
        // that is the save's own contract rather than an omission: the
        // shared edit path stamps `generated`, and a save of what was read
        // has to land byte-identical. The compare and the write are held
        // apart from every other writer of the same draft by the one lock
        // they all take, keyed on the draft's own mirror path. See
        // `Engine::draft_lock`.
        let lock = self.draft_lock(&desc.domain, who, &desc.path)?;
        let _guard = lock.lock().await;
        let current = view.text_at(source, desc).await?.ok_or_else(|| {
            EngineError::NotFound(format!(
                "no engram '{}' in domain '{}'",
                p.identifier, desc.domain
            ))
        })?;
        let found = sha256_hex(current.as_bytes());
        if found != p.expected_checksum {
            return Err(EngineError::Conflict(stale_edit_message(
                &p.expected_checksum,
                &found,
            )));
        }
        let warning = view.write(desc.domain_id, &desc.path, &p.content).await?;
        // Where the draft now answers, derived exactly as the row's own
        // permalink is: an author who edited the frontmatter's permalink
        // line has just moved the address, and the receipt has to say so.
        let permalink = parse_engram(&p.content)
            .map(|engram| {
                EngramRecord::from_engram(&engram, &desc.path, virtual_stamp(&p.content)).permalink
            })
            .unwrap_or_else(|_| desc.permalink.clone());
        let mut receipt = json!({
            "domain": desc.domain,
            "permalink": permalink,
            "path": desc.path,
            "checksum": sha256_hex(p.content.as_bytes()),
            "draft": true,
        });
        // Whose draft it landed in, when that is not the caller's own. The
        // one thing a joined save has to say that an ordinary one does
        // not: somebody typing inside a colleague's draft is owed a
        // receipt that names whose work they just changed.
        if let Some(owner) = view.joined() {
            receipt["joined"] = json!(format!("landed in {owner}'s draft"));
        }
        note_unmirrored(&mut receipt, warning);
        Ok(receipt)
    }

    /// Write an engram file back into existence with this exact content, then
    /// reindex it: the resolution path for "externally deleted while a collab
    /// session held unsaved work". [`Engine::save_engram`] refuses a missing
    /// file by design (a save of something that is not there is a miss, not a
    /// create), so a room whose author keeps their text needs this verb.
    ///
    /// Same parse gate as a save and the same receipt shape, addressed by
    /// PATH rather than by identifier: the engram is gone from the index, so
    /// there is nothing left to resolve. No CAS token either, for the same
    /// reason - there is no stored version to compare against.
    ///
    /// `scope` is the acting scope every write verb carries; see
    /// [`Engine::write_engram_as`].
    pub async fn restore_engram(
        &self,
        domain: &str,
        path: &str,
        content: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write(self, domain, scope).await?;
        self.restore_engram_in_view(&view, domain, path, content)
            .await
    }

    /// [`Engine::restore_engram`] through a view somebody already built.
    ///
    /// The split exists for the co-editing room, which has a view of its own -
    /// the overlay document it is a room over, or the one a direct domain
    /// keeps - and no scope to derive one from. A request reaches it through
    /// the verb above, with the view its scope routed to; both write exactly
    /// what they always wrote.
    pub(crate) async fn restore_engram_in_view(
        &self,
        view: &DomainView<'_>,
        domain: &str,
        path: &str,
        content: &str,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let overlay = view.actor();
        refuse_not_an_engram(content)?;
        // Normalized and screened before the two reserved checks read it, the
        // same order the create and move paths use. A stored path is already in
        // this shape, so nothing a caller sends today changes.
        let normalized = normalize_rel(path);
        let path = normalized.as_str();
        if !is_within_domain(path) {
            return Err(EngineError::Invalid(escapes_root_error(path)));
        }
        if crystalline_core::is_reserved_path(path) {
            return Err(EngineError::Invalid(reserved_name_error(path)));
        }
        if is_assets_reserved(path) {
            return Err(EngineError::Invalid(assets_reserved_error(path)));
        }
        let (domain_id, source) = self.domain_source(domain).await?;
        // The third place a restore can land: in review mode the recovered
        // document is this actor's draft of the path, never a file written
        // back into what the team reviewed.
        if overlay.is_some() {
            let warning = view.write(domain_id, path, content).await?;
            let permalink = parse_engram(content)
                .map(|engram| {
                    EngramRecord::from_engram(&engram, path, virtual_stamp(content)).permalink
                })
                .unwrap_or_else(|_| path.trim_end_matches(".md").to_string());
            let mut receipt = json!({
                "domain": domain,
                "permalink": permalink,
                "path": path,
                "checksum": sha256_hex(content.as_bytes()),
                "draft": true,
            });
            note_unmirrored(&mut receipt, warning);
            return Ok(receipt);
        }
        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, path);
                // Held across the write and the reindex, like every other
                // file write. See `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                write_file(&abs, content)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, domain_id, root, path).await?;
            }
            ContentSource::Virtual => {
                let stamp = virtual_stamp(content);
                let store = self.store.lock().await;
                self.index_markdown(&*store, domain_id, path, content, stamp, None, true)
                    .await?;
            }
        }
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        self.refresh_index_files(domain).await;

        // Read back after the reindex, exactly as a save does: the index takes
        // the permalink from the restored frontmatter, which need not match
        // the path slug. Tolerantly, for the same reason a save asks
        // tolerantly: the restore is committed by this line, so a missing row
        // and a failing lookup alike fall back to the path-derived name rather
        // than reporting a done write as failed - see [`receipt_permalink`].
        let permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(domain, Some(path), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == path)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, path.trim_end_matches(".md").to_string())
        };
        self.nudge_embed();
        Ok(json!({
            "domain": domain,
            "permalink": permalink,
            "path": path,
            "checksum": sha256_hex(content.as_bytes()),
        }))
    }

    /// The teaching sentence for a write that named a draft this caller holds
    /// a link to but cannot resolve, or `None` when the name is nothing of the
    /// sort.
    ///
    /// Asked only when an ordinary resolution has already missed, and only for
    /// a caller holding at least one live link in this domain - which is
    /// almost nobody, almost never. The name is matched against what the link
    /// actually opens: the draft's own address, its path, and the path with
    /// the suffix off, which are the three spellings the editor and the API
    /// address an engram by.
    pub(super) async fn teach_granted_miss(
        &self,
        domain: &str,
        identifier: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Option<String>> {
        Ok(self
            .granted_draft_named(domain, identifier, None, scope)
            .await?
            .map(|(owner, path)| granted_needs_join(&owner, &path)))
    }

    /// Present a share-link and open a join on the draft it names.
    ///
    /// **The agent's door into somebody else's draft, and it is the same door
    /// a browser goes through.** The two REST steps a person takes in one
    /// call: accepting the link, which binds it to their account for good, and
    /// joining it, which is the second and separate decision to type into
    /// somebody's work. One call, because an agent that was handed a link and
    /// passed it to a verb has decided both. The join it opens belongs to the
    /// HOLDER that presented it - a browser session, an MCP session, a process,
    /// or a token identity that has no session at all and is ended by idleness
    /// instead - and ends when that holder does; the key is what the caller
    /// holds, and nothing else in this process hands it out.
    ///
    /// The order is the REST route's order and it is load bearing: the domain
    /// screen comes BEFORE the redemption, because redeeming binds the link
    /// irreversibly and an account that may not read the domain would
    /// otherwise burn it for the person it was meant for.
    ///
    /// Every way a link fails to open anything is one refusal, deliberately:
    /// an invented link, a revoked one, an expired one, one already bound to
    /// somebody else and one into a domain this caller may not read are five
    /// different facts and none of them is this caller's to learn.
    pub async fn open_share_link(
        &self,
        token: &str,
        scope: &crate::scope::Scope,
        holder: &crate::join::Holder,
    ) -> Result<OpenedLink> {
        let Some(account) = crate::scope::overlay_actor(scope) else {
            return Err(EngineError::Refused(
                "a draft share-link binds to an account, and this session has none: sign in \
                 before presenting one"
                    .to_string(),
            ));
        };
        let Some(access) = self.domain_access.get() else {
            // Every install that serves no accounts: a one-shot command, the
            // embedded stdio stack, a daemon with the web surface off. There
            // are no share-links there to present, and saying so is better
            // than the dead-link sentence, which would suggest this one had
            // simply expired.
            return Err(EngineError::Refused(
                "this instance serves no accounts, so it mints and opens no draft share-links: \
                 drafts here belong to whoever runs it"
                    .to_string(),
            ));
        };
        let dead = || {
            EngineError::NotFound(
                "this draft link opens nothing: it may have been revoked, it may have expired, \
                 it may already belong to somebody else, or the draft it was for may have been \
                 folded or discarded. Ask whoever shared it for a fresh one."
                    .to_string(),
            )
        };
        let named = access
            .overlay_grant_domain(token)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?
            .ok_or_else(dead)?;
        // The same screen every other read of a domain makes, on the grantee:
        // a link is its author's word about one draft and never about a
        // domain, so a private domain this account is not a member of stays a
        // domain it has never heard of.
        let hidden = self.hidden_for(scope).await?;
        if self.domain_entry_scoped(&named, &hidden).is_err() {
            return Err(dead());
        }
        let crate::scope::RedeemedLink {
            domain,
            owner,
            path,
            expires_at,
        } = access
            .redeem_overlay_grant(token, &account)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?
            .ok_or_else(dead)?;
        // A grant lasts exactly as long as the thing it grants, and a join
        // into a draft that is gone is a join to nothing.
        if self
            .overlay_draft_at(&domain, &owner, &path)
            .await?
            .is_none()
        {
            return Err(EngineError::NotFound(format!(
                "this link was for {owner}'s draft of '{path}', and that draft is no longer \
                 there: it was folded into the domain, discarded, or moved somewhere else. Ask \
                 for a fresh link, or look for the page in the domain itself."
            )));
        }
        // Seeing a draft and editing it are two states, and this is the one
        // that needs the right: an account that may only read the domain opens
        // the link and reads the draft, and is told why it may not type in it.
        let right = access
            .write_right(scope, &domain)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        if right < crate::scope::DomainRight::Write {
            // Bound, readable, and not joined - which is a whole answer rather
            // than a failure. A read crossed the grant and got what the grant
            // is for; only a write needed the second step, and only a write is
            // refused by this sentence.
            return Ok(OpenedLink::ReadOnly(format!(
                "you may read {owner}'s draft of '{path}' and not edit it: your access on \
                 '{domain}' is {}, and editing somebody's draft needs the same editor access \
                 that writing anything else here needs. Suggest changes to whoever shared it, \
                 or ask for editor access on the domain.",
                crate::scope::member_level_word(right)
            )));
        }
        let join = crate::join::Join {
            account,
            // Which of this account's callers is inside the draft, and what
            // ending ends it. Decided by the surface rather than here: only it
            // knows whether this request is a browser session, an MCP session,
            // a process or a stateless peer with a token and no session at
            // all. See [`crate::join::Holder`].
            holder: holder.clone(),
            domain,
            path,
            owner,
            // The link's own window, stamped on once: an expiry is the one way
            // a grant ends that nobody announces, so the join carries the
            // moment rather than the saver re-reading the row.
            expires_at: crate::join::grant_deadline(expires_at.as_deref()),
        };
        // A cap met is the same shape as the read-only answer above and for
        // the same reason: the link bound, the draft is readable, and the join
        // is what could not be opened.
        let key = match self.joins().open(join.clone()) {
            Ok(key) => key,
            Err(crate::join::JoinRefusal::AccountFull) => {
                return Ok(OpenedLink::ReadOnly(
                    "you are already working inside as many drafts as this instance keeps open \
                     for one account: leave one of them and this one will open"
                        .to_string(),
                ));
            }
            Err(crate::join::JoinRefusal::InstanceFull) => {
                return Ok(OpenedLink::ReadOnly(
                    "this instance is already holding as many drafts open as it will hold at \
                     once, across everybody: leave one of yours, or try again shortly"
                        .to_string(),
                ));
            }
        };
        Ok(OpenedLink::Joined { key, join })
    }

    /// The draft this caller holds a live link to that `identifier` names, as
    /// `(owner, path)`, or `None` when the name is nothing of the sort.
    ///
    /// `owner` narrows it to one author's, which is what a caller asking to
    /// open a room over somebody's document needs: it names whose, and the
    /// answer has to be about that person rather than about whoever this
    /// account happens to hold a link from. `None` asks about any of them,
    /// which is what the teaching refusal above needs.
    ///
    /// Asked only where an answer would change what a caller is told, and only
    /// for a caller holding at least one live link in this domain - which is
    /// almost nobody, almost never. The name is matched against what the link
    /// actually opens: the draft's own address, its path, and the path with
    /// the suffix off, which are the three spellings the editor and the API
    /// address an engram by. A link whose draft has gone matches nothing, so a
    /// dead link teaches nothing and opens nothing.
    #[doc(hidden)]
    pub async fn granted_draft_named(
        &self,
        domain: &str,
        identifier: &str,
        owner: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Option<(String, String)>> {
        let Some(account) = crate::scope::overlay_actor(scope) else {
            return Ok(None);
        };
        let Some(access) = self.domain_access.get() else {
            return Ok(None);
        };
        let held = access
            .overlay_grants_held(&account, domain)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        for (path, held_owner) in held {
            if held_owner == account || owner.is_some_and(|want| want != held_owner) {
                continue;
            }
            let Some(draft) = self.overlay_draft_at(domain, &held_owner, &path).await? else {
                continue;
            };
            let names = [
                draft.permalink.as_str(),
                path.as_str(),
                path.trim_end_matches(".md"),
            ];
            if names.contains(&identifier) {
                return Ok(Some((held_owner, path)));
            }
        }
        Ok(None)
    }

    /// What a join may do to the OWNER's files, and what it may not.
    ///
    /// A join is into one page. The files overlay is not that page, so without
    /// a rule here a session joined to one draft would hold a write capability
    /// over every attachment its owner has - able to overwrite a picture staged
    /// for a different draft of theirs, or to stage their deletion of a file the
    /// team reviewed, to be folded later under their name. That is the one
    /// cross-account write capability in the system, so it is bounded to the
    /// work that was actually shared.
    ///
    /// **A join carries the granted engram and the attachments that engram
    /// references**, and the two things that follow are the whole of the rule:
    ///
    /// * a path that stands nowhere - not in the folder the team reviewed and
    ///   not in the owner's own overlay - may be created, because adding an
    ///   illustration to the page you were invited into is the reason a join
    ///   reaches the files at all;
    /// * a path the granted draft references **at this moment** may be
    ///   overwritten or deleted, because a page and the pictures it shows are
    ///   one piece of work.
    ///
    /// Everything else is refused, in words that name the file and the two ways
    /// forward. "At this moment" is deliberate and is why the draft is read
    /// here rather than at join time: the reference set is whatever the shared
    /// page says now, so a joiner who adds a reference and then uploads to it
    /// is inside the rule, and one whose reference was removed by the author is
    /// outside it again.
    ///
    /// A join whose draft has gone carries no references at all, so only the
    /// create arm stays open - which is the same answer the freshness check
    /// gives everywhere else, reached by the same reasoning.
    pub(super) async fn screen_joined_attachment(
        &self,
        domain: &str,
        path: &str,
        join: &crate::join::Join,
        deleting: bool,
    ) -> Result<()> {
        let referenced = match self
            .overlay_draft_at(domain, &join.owner, &join.path)
            .await?
        {
            Some(draft) => parse_engram(&draft.content)
                .map(|engram| crystalline_core::find_asset_refs(&engram.body))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        if referenced.iter().any(|reference| reference == path) {
            return Ok(());
        }
        // A deletion has no create arm: there is nothing to make at a path
        // nothing stands at, and `attachment_delete_in` answers that miss on
        // its own.
        if !deleting
            && !self
                .anybody_holds_attachment(domain, &join.owner, path)
                .await?
        {
            return Ok(());
        }
        Err(EngineError::Refused(joined_files_are_the_drafts(
            &join.owner,
            &join.path,
            path,
        )))
    }

    /// Whether anything stands at one attachment path as far as a join is
    /// concerned: the folder the team reviewed, or the owner's own files
    /// overlay.
    ///
    /// Both halves, because either one makes the path somebody else's work. A
    /// path the owner has DELETED in their overlay still counts as standing,
    /// since the file is in the folder and their deletion of it is a draft
    /// change of theirs - which is exactly the kind of decision a join into a
    /// different page must not reach around.
    async fn anybody_holds_attachment(
        &self,
        domain: &str,
        owner: &str,
        path: &str,
    ) -> Result<bool> {
        match self.attachment_delete_size(domain, path).await {
            Ok(_) => return Ok(true),
            Err(EngineError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let state_dir = self.journal_state_dir()?;
        match crate::overlay_files::held(&state_dir, domain, owner, path) {
            Ok(crate::overlay_files::Held::Nothing) => Ok(false),
            Ok(_) => Ok(true),
            // A path this substrate refuses is one the write verb refuses a
            // line later in words the caller already knows, so this screen
            // says nothing about it and lets that refusal happen.
            Err(_) => Ok(false),
        }
    }

    /// Refuse a write that is inside the wrong draft, or inside one this
    /// caller may see and has not joined.
    ///
    /// Two refusals in one place, because they are two halves of one rule:
    /// **a share-link grants visibility, and editing is a second, explicit
    /// step.** The one caller who has taken that step gets a routed write at
    /// exactly the path they took it for; everybody else who can see a draft
    /// is told, in the same words, what their two ways forward are.
    ///
    /// Nothing happens at all for the common case - no join, no grant - and
    /// the grant lookup is skipped entirely for a caller with no account,
    /// since a link binds to an account and nobody else can hold one.
    pub(super) async fn screen_granted_path(
        &self,
        domain: &str,
        path: &str,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<()> {
        if let Some(join) = join {
            if join.path != path {
                // A join whose own draft has gone is not a join to a different
                // path, it is a join to nothing: its author renamed it, folded
                // it or took it back, and a refusal naming the page they joined
                // would be a sentence about somewhere that is not there.
                if self
                    .overlay_draft_at(domain, &join.owner, &join.path)
                    .await?
                    .is_none()
                {
                    return Err(EngineError::Refused(joined_draft_is_gone(
                        &join.owner,
                        &join.path,
                    )));
                }
                return Err(EngineError::Refused(joined_write_is_elsewhere(
                    &join.owner,
                    &join.path,
                    path,
                )));
            }
            return Ok(());
        }
        let Some(account) = crate::scope::overlay_actor(scope) else {
            return Ok(());
        };
        let Some(owner) = self.granted_owner(&account, domain, path).await? else {
            return Ok(());
        };
        if owner == account {
            return Ok(());
        }
        // **Only while there is still a draft to join.** A link outlives the
        // draft it was for whenever its author takes that draft away without
        // the domain leaving review mode - a deletion, a move, a rename - and
        // a grantee still told to join it could neither join (the link answers
        // that the draft is gone) nor write. A dead row would have taken a
        // path away from somebody it was never about, so it takes nothing: the
        // write goes back to being their own, which is what it always was.
        if self.overlay_draft_at(domain, &owner, path).await?.is_none() {
            return Ok(());
        }
        Err(EngineError::Refused(granted_needs_join(&owner, path)))
    }

    /// A registered domain's row id and content source, upserting the row the
    /// way a create does. The domain-addressed half of what
    /// [`Engine::resolve`] does for an identifier, for a write path whose
    /// engram is not in the index to resolve.
    pub(crate) async fn domain_source(&self, domain: &str) -> Result<(DomainId, ContentSource)> {
        let source = self.content_source(domain)?;
        let store = self.store.lock().await;
        let domain_id = match &source {
            ContentSource::File { root } => {
                store
                    .upsert_domain(domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?
            }
            ContentSource::Virtual => {
                store
                    .upsert_domain(domain, None, DomainKind::Virtual)
                    .await?
            }
        };
        Ok((domain_id, source))
    }
}
