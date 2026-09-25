use super::*;

impl Engine {
    // --- move ----------------------------------------------------------------

    /// Move an engram to a new path, a new permalink or a new domain, and
    /// rewrite every reference that followed it there.
    ///
    /// A move is a refactoring. Source and destination may each be a file or
    /// virtual domain, so a move carries content between the two truths: a
    /// same-domain move renames the row in place (the id, and with it every
    /// chunk, embedding and bound reference, stays), a cross-domain move reads
    /// the source content and re-indexes it into the destination's source.
    ///
    /// **The permalink** is decided by [`Engine::moved_permalink`]: by default
    /// it follows the move only when it was in step with the old path, so a
    /// deliberate custom permalink survives a re-filing; `MoveParams::permalink`
    /// asks for the path's own, the current one, or a named one. A destination
    /// equal to the current path is the permalink-only rename - the repair for
    /// a permalink that drifted off its folder - and a move that changes
    /// neither path, domain nor permalink is refused as the no-op it is. The
    /// new permalink is written into the frontmatter `permalink:` line
    /// surgically, so the file and the row say the same thing and a later
    /// sync does not quietly put the old one back; `recorded_at` stays, and the
    /// `generated` block records the mover whenever the move changed the text.
    ///
    /// **Every address change rewrites every reference**, whether the domain,
    /// the permalink or both changed: `[[old]]`, `[[domain:old]]`, relation
    /// bullets and `crystalline://domain/old` URLs (see
    /// [`crystalline_core::relink`]), found through the index before the move
    /// and rewritten in each referencing engram's source of truth afterwards.
    /// A link by title is left alone unless the engram changed domain, since
    /// the title did not change and the link still resolves. `update_links`
    /// set to false skips the rewrite, as it always did.
    ///
    /// `scope` bounds the *side effect*, which is the half a surface cannot
    /// gate for itself. Whether this caller may write either end is decided at
    /// the edge (the REST write gate, the MCP domain gate); what only this
    /// function can decide is which other domains it rewrites a link inside.
    /// Those referencing engrams live in domains the mover may never have been
    /// shown. So the rewrite skips a domain this caller may not see: its link
    /// is left as it was - dangling, which its own members see as an
    /// unresolved-reference finding on the next sweep - rather than silently
    /// edited by somebody with no access to it, and the receipt's counts
    /// (`links_rewritten`, the engrams rewritten, and `references_rewritten`,
    /// the references inside them) cover the visible rewrites only, so a
    /// receipt never counts a file its reader may not know exists.
    pub async fn move_engram(&self, p: &MoveParams, scope: &crate::scope::Scope) -> Result<Value> {
        self.move_engram_as(p, None, scope).await
    }

    /// [`Engine::move_engram`] with the mover's identity, resolved by
    /// [`Engine::actor`] (or, for a draft, the composed identity) and written
    /// into the moved engram's `generated` block when the move changes its
    /// text. The engrams whose references are rewritten record Crystalline
    /// itself instead: the mover did not author them.
    pub async fn move_engram_as(
        &self,
        p: &MoveParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        // Resolved once, before anything is written, and used twice below: to
        // look the destination up, and to bound the inbound rewrite.
        let hidden = self.hidden_for(scope).await?;
        let view = DomainView::for_write(self, &p.domain, scope).await?;
        let overlay = view.actor();
        let (src, src_source) = view.resolve(&p.identifier).await?;
        let dest_domain = p
            .destination_domain
            .clone()
            .unwrap_or_else(|| p.domain.clone());
        // Scoped, and that is load bearing rather than tidy. This lookup raises
        // the one error that names every registered domain, and a surface gate
        // above it can only check the spelling it normalizes: a padded or empty
        // `destination_domain` passes a gate that trims or skips it and arrives
        // here verbatim. Resolving it against the caller's own visible set
        // closes that for every spelling, present and future, instead of asking
        // one more pair of normalizers to agree.
        let dest_source = self.content_source_scoped(&dest_domain, &hidden)?;
        let dest_rel = normalize_md(&p.destination);
        if dest_rel.is_empty() {
            return Err(EngineError::Invalid("destination path is empty".into()));
        }
        // As on the create path: `normalize_md` drops empty segments but keeps
        // `..`, so containment is decided here, before either reserved check
        // reads the destination as text and before `join_rel` builds a path
        // from it.
        if !is_within_domain(&dest_rel) {
            return Err(EngineError::Invalid(escapes_root_error(&dest_rel)));
        }
        if crystalline_core::is_reserved_path(&dest_rel) {
            return Err(EngineError::Invalid(reserved_name_error(&dest_rel)));
        }
        if is_assets_reserved(&dest_rel) {
            return Err(EngineError::Invalid(assets_reserved_error(&dest_rel)));
        }
        let cross = dest_domain != p.domain;
        let new_permalink = Self::moved_permalink(&src, &dest_rel, p.permalink.as_deref())?;
        let in_place = !cross && dest_rel == src.path;
        if in_place && new_permalink == src.permalink {
            return Err(EngineError::Invalid(format!(
                "'{}' already sits at '{dest_rel}' and answers to '{new_permalink}', so this \
                 move changes nothing; to rename it in place pass a permalink (\"path\" for the \
                 destination path's own)",
                src.permalink
            )));
        }
        // Whether the engram answers to a different address afterwards, which
        // is what every reference to it has to follow.
        let readdressed = cross || new_permalink != src.permalink;

        // The third place a move can land, and it is two writes rather than
        // one: a tombstone where the team's file is, so this actor stops
        // seeing the engram there, and an entry at the destination, so they
        // see it where they moved it to. The folder itself does not move,
        // which is the whole of review mode - the rename is reviewed like any
        // other change.
        if let Some(who) = overlay {
            // Two writes - a tombstone where the team's file is, an entry at
            // the destination - so two draft locks, taken in key order so two
            // moves that cross each other cannot each hold what the other
            // wants. `move_within` refuses a cross-domain move outright, so
            // both paths are this actor's own drafts in this one domain. See
            // `Engine::draft_lock`.
            let (first, second) = match src.path <= dest_rel {
                true => (src.path.as_str(), dest_rel.as_str()),
                false => (dest_rel.as_str(), src.path.as_str()),
            };
            let low = self.draft_lock(&p.domain, who, first)?;
            let _low = low.lock().await;
            // A move onto its own path is the permalink-only rename (a no-op
            // one was refused above), and its one path is taken once rather
            // than twice: one path is one lock.
            let high = match first == second {
                true => None,
                false => Some(self.draft_lock(&p.domain, who, second)?),
            };
            let _high = match &high {
                Some(lock) => Some(lock.lock().await),
                None => None,
            };
            // A draft's permalink changes only when the caller names one. The
            // default rule stays out of review mode on purpose: a moved draft
            // is paired with the team's engram it came from by the address it
            // carries (the fold and the pull's rename-follow both key on it),
            // so a draft that quietly took the path's slug would stop reading
            // as a move of that engram and start reading as a new one.
            let asked = p
                .permalink
                .as_deref()
                .filter(|asked| !asked.trim().is_empty())
                .map(|_| new_permalink.as_str());
            let mover = self.actor_for(client, overlay);
            return view
                .move_within(p, &src, &src_source, &dest_rel, cross, asked, &mover)
                .await;
        }

        // After the overlay branch, so a move OUT of a reviewing domain keeps
        // its own refusal ("share the change first"), and before anything is
        // read or written, so a refusal costs nothing. This is the move INTO
        // one: the view above is the SOURCE domain's, so a move from a domain
        // that takes changes directly would otherwise write the destination's
        // folder - the engram and every attachment it carries.
        if cross {
            self.refuse_write_into_reviewed_folder(
                &dest_domain,
                "a move from another domain cannot land there",
            )?;
        }

        // Destination collision checks, all of them before the first write so a
        // refusal leaves nothing half done: the path on disk or in the
        // database (an in-place rename is its own destination), then the
        // address, which the unique index would otherwise refuse only at the
        // reindex - after the file had already landed.
        if !in_place {
            self.ensure_dest_free(&dest_source, &dest_domain, &dest_rel)
                .await?;
        }
        if readdressed {
            self.refuse_permalink_held(&dest_domain, &new_permalink, &src, cross)
                .await?;
            self.refuse_readdress_under_live_room(&p.domain, &src.permalink)
                .await?;
        }

        // Gather the referencing engrams before the move, while every bound
        // reference's `to_id` still points at the source row.
        let update_links = p.update_links.unwrap_or(true);
        let linkers = if readdressed && update_links {
            self.move_linkers(&src, &hidden).await?
        } else {
            Vec::new()
        };
        let spec = Relink {
            from_domain: &p.domain,
            from_permalink: &src.permalink,
            title: &src.title,
            to_domain: &dest_domain,
            to_permalink: &new_permalink,
        };

        // Read once, stricter than the read path on purpose: a move re-emits
        // the file at the destination, and a file domain's stored `content`
        // column holds the body only, so falling back to it would strip the
        // frontmatter off the engram that lands. Failing loudly is the only
        // honest option; the store is the source of truth for a virtual
        // domain, so only that kind reads from it.
        let original = match &src_source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &src.path);
                std::fs::read_to_string(&abs).map_err(|e| {
                    EngineError::NotFound(format!(
                        "the source file for '{}' at {} is unreadable ({e}); resync '{}' and retry the move",
                        src.permalink,
                        abs.display(),
                        p.domain
                    ))
                })?
            }
            ContentSource::Virtual => self.load_content(&src_source, &src).await?,
        };
        // The text the engram lands with, when the move has to change it: the
        // permalink line, and its own references to itself. `None` is the
        // plain rename, which carries the bytes across untouched.
        let moved_text = Self::readdressed_text(
            &original,
            &dest_rel,
            &new_permalink,
            (readdressed && update_links).then_some(&spec),
            &self.actor(client),
        )?;

        // What the move carries besides the engram: the attachments it
        // references or claims. Filled in the cross-domain branch and acted on
        // once every store lock that branch takes has been released. Beside it,
        // the attachments the plan could not carry, which ride out in the
        // receipt so a caller who never sees the daemon's trace still learns
        // what stayed behind.
        let mut carried: Vec<AttachmentCarry> = Vec::new();
        let mut attachment_warnings: Vec<String> = Vec::new();

        if cross {
            // Index the content into the destination source, then remove the
            // source.
            let mut content = moved_text.clone().unwrap_or_else(|| original.clone());
            // Resolved before the write, since an attachment that has to be
            // renamed at the destination changes the very text being written:
            // the engram lands already pointing at the name its file took.
            (carried, attachment_warnings) = self
                .plan_attachment_carry(&src, &dest_domain, &content)
                .await;
            let renames: BTreeMap<String, String> = carried
                .iter()
                .filter(|carry| carry.to != carry.from)
                .map(|carry| (carry.from.clone(), carry.to.clone()))
                .collect();
            if !renames.is_empty() {
                content = rewrite_carried_refs(&content, &renames);
            }
            match &dest_source {
                ContentSource::File { root } => {
                    let dest_abs = join_rel(root, &dest_rel);
                    write_file(&dest_abs, &content)?;
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(
                            &dest_domain,
                            Some(&root.to_string_lossy()),
                            DomainKind::File,
                        )
                        .await?;
                    self.reindex_file(&*store, dest_id, root, &dest_rel).await?;
                }
                ContentSource::Virtual => {
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(&dest_domain, None, DomainKind::Virtual)
                        .await?;
                    let stamp = virtual_stamp(&content);
                    self.index_markdown(&*store, dest_id, &dest_rel, &content, stamp, None, true)
                        .await?;
                }
            }
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                if let Err(e) = std::fs::remove_file(&src_abs) {
                    tracing::warn!(
                        "could not remove moved source {}: {e}; leaving it in place",
                        src_abs.display()
                    );
                }
            }
            let store = self.store.lock().await;
            store.delete_engram(src.domain_id, &src.path).await?;
        } else {
            // Same-domain rename: move the file when file-backed, then give the
            // row its new path and permalink in place, keeping its id. A move
            // that did not change the text needs no reparse; one that did is
            // reindexed from what was written, so the row reads the file.
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                let dest_abs = join_rel(root, &dest_rel);
                match &moved_text {
                    Some(text) => write_file(&dest_abs, text)?,
                    None => write_bytes(&dest_abs, original.as_bytes())?,
                }
                if !in_place {
                    std::fs::remove_file(&src_abs).map_err(|source| EngineError::Io {
                        path: src_abs.display().to_string(),
                        source,
                    })?;
                }
            }
            let store = self.store.lock().await;
            store
                .readdress_engram(src.domain_id, &src.path, &dest_rel, &new_permalink)
                .await?;
            if let Some(text) = &moved_text {
                match &src_source {
                    ContentSource::File { root } => {
                        self.reindex_file(&*store, src.domain_id, root, &dest_rel)
                            .await?;
                    }
                    ContentSource::Virtual => {
                        self.index_markdown(
                            &*store,
                            src.domain_id,
                            &dest_rel,
                            text,
                            virtual_stamp(text),
                            None,
                            true,
                        )
                        .await?;
                    }
                }
            } else if readdressed {
                // A reference somebody wrote ahead of time to the new address
                // binds now rather than at the next sync. The reindex arm above
                // resolves inside its own transaction.
                store.resolve_pending_relations(src.domain_id).await?;
                store.resolve_pending_links(src.domain_id).await?;
            }
        }

        // The attachments follow the engram, now that the engram itself has
        // landed and the branch above has released its store lock. A
        // same-domain move carries nothing: an `assets/` reference is
        // domain-root relative, so a rename inside one domain leaves every one
        // of them valid as written.
        if !carried.is_empty() {
            self.carry_attachments(&src.domain, &dest_domain, &carried)
                .await;
        }

        // Rewrite every reference that followed the engram. The referencing
        // engrams were not authored by whoever asked for the move, so their
        // refreshed `generated.by` records Crystalline itself (or the
        // configured `identity.actor`), not the moving client. A referencing
        // engram that cannot be rewritten is logged and skipped rather than
        // failing the call: the move is committed by this line, and an error
        // now would report a move that happened as one that did not.
        let linker_actor = self.actor(None);
        let mut rewritten: Vec<Value> = Vec::new();
        let mut references = 0usize;
        for linker in &linkers {
            match self.relink_engram(linker, &spec, &linker_actor).await {
                Ok(Some((count, permalink))) => {
                    references += count;
                    rewritten.push(json!({ "domain": linker.domain, "permalink": permalink }));
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    domain = linker.domain.as_str(),
                    path = linker.path.as_str(),
                    "the move could not rewrite the references in this engram: {e}"
                ),
            }
        }

        // When either end of the move is a virtual domain, a MANIFEST engram
        // may have moved into or out of it, so refresh the routing cache. Every
        // store lock taken above is released by here.
        if matches!(src_source, ContentSource::Virtual)
            || matches!(dest_source, ContentSource::Virtual)
        {
            self.refresh_routing_cache().await;
        }
        // A move empties one folder and fills another, so both ends need their
        // index files back in step; a same-domain move refreshes once.
        self.refresh_index_files(&p.domain).await;
        if cross {
            self.refresh_index_files(&dest_domain).await;
        }

        // The address the engram answers to at its destination, asked rather
        // than assumed, exactly as `save_engram` asks: a permalink that was
        // derived from the path follows the move (the store's rename says so
        // in as many words), so a receipt repeating the one it went in with
        // would name a permalink that no longer resolves on the very calls
        // that changed it. The move is committed by this line, so the asking is
        // tolerant: a missing row and a failing lookup alike fall back to the
        // name it went in with rather than failing a done move - see
        // [`receipt_permalink`].
        let dest_permalink = {
            let store = self.store.lock().await;
            let found = store
                .list_engrams(&dest_domain, Some(&dest_rel), None)
                .await
                .map_err(EngineError::from)
                .map(|rows| {
                    rows.into_iter()
                        .find(|found| found.path == dest_rel)
                        .map(|found| found.permalink)
                });
            receipt_permalink(found, new_permalink.clone())
        };

        // `links_rewritten` counts engrams, as it always has; the references
        // inside them are `references_rewritten`, and `rewritten` names the
        // engrams so a caller can re-read what changed under it.
        Ok(json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": dest_domain, "permalink": dest_permalink, "path": dest_rel },
            "cross_domain": cross,
            "links_rewritten": rewritten.len(),
            "references_rewritten": references,
            "rewritten": rewritten,
            "attachment_warnings": attachment_warnings,
        }))
    }

    /// The permalink a moved engram answers to at `dest_rel`.
    ///
    /// Omitted, the permalink follows the move only when it was **in step**
    /// with the old path - equal to the slug the old path derives, which is
    /// also what an engram with no `permalink:` line answers to - and stays
    /// otherwise, since a permalink that differs from its path was either
    /// chosen on purpose or drifted, and only the caller knows which. `"path"`
    /// takes the destination path's own, `"keep"` keeps the current one
    /// whatever it is, and any other value is validated as a permalink and
    /// taken as given. The two keywords are why neither can be asked for as a
    /// literal permalink; an engram that should answer to `path` is written
    /// to `path.md` and moved with `"path"`.
    fn moved_permalink(
        src: &EngramDescriptor,
        dest_rel: &str,
        asked: Option<&str>,
    ) -> Result<String> {
        let from_path = crystalline_core::path_permalink(dest_rel);
        let chosen = match asked.map(str::trim) {
            None | Some("") => {
                if src.permalink == crystalline_core::path_permalink(&src.path) {
                    from_path
                } else {
                    src.permalink.clone()
                }
            }
            Some("path") => from_path,
            Some("keep") => src.permalink.clone(),
            Some(named) => {
                crystalline_core::validate_permalink(named).map_err(EngineError::Invalid)?;
                named.to_string()
            }
        };
        if chosen.is_empty() {
            return Err(EngineError::Invalid(format!(
                "the destination '{dest_rel}' does not slugify to a permalink; use a path with \
                 letters or digits, or pass a permalink"
            )));
        }
        Ok(chosen)
    }

    /// The text a moved engram lands with, or `None` when the move leaves its
    /// bytes exactly as they are.
    ///
    /// Two things can change it. The `permalink:` line, rewritten surgically
    /// (or added) whenever what the text would answer to at `dest_rel` - its
    /// own `permalink:` line, or the destination path's slug when it has none -
    /// is not `new_permalink`: that is what makes `"keep"` hold for an engram
    /// whose permalink was only ever path-derived, and what makes the new
    /// address survive the next sync. And, when `relink` is given, the engram's
    /// references to itself, which follow it like everybody else's. Either
    /// change touches `generated` with the mover; `recorded_at` is never
    /// touched, because the knowledge was not recorded again.
    fn readdressed_text(
        original: &str,
        dest_rel: &str,
        new_permalink: &str,
        relink: Option<&Relink<'_>>,
        mover: &str,
    ) -> Result<Option<String>> {
        let engram = parse_engram(original).map_err(|e| {
            EngineError::Invalid(format!(
                "the engram does not parse ({e}); fix it before moving it"
            ))
        })?;
        let implied = engram
            .frontmatter
            .permalink
            .filter(|permalink| !permalink.is_empty())
            .unwrap_or_else(|| crystalline_core::path_permalink(dest_rel));
        let mut text = original.to_string();
        if implied != new_permalink {
            text = set_frontmatter_field(&text, "permalink", new_permalink);
        }
        if let Some(spec) = relink {
            text =
                crystalline_core::relink::relink(&text, spec.from_domain, spec.to_domain, spec).0;
        }
        Ok((text != original).then(|| touch_generated(&text, mover, None, now_offset())))
    }

    /// Refuse a move onto a permalink another engram in the destination domain
    /// already answers to, naming the holder, before anything is written.
    ///
    /// Asked by permalink alone: [`Store::find_engram`] also matches a title,
    /// and an engram titled like the new permalink does not hold the address.
    /// The moved engram itself is not a holder on a same-domain move, since
    /// its row is the one being readdressed.
    async fn refuse_permalink_held(
        &self,
        dest_domain: &str,
        new_permalink: &str,
        src: &EngramDescriptor,
        cross: bool,
    ) -> Result<()> {
        let holder = {
            let store = self.store.lock().await;
            store
                .find_engram(dest_domain, new_permalink)
                .await?
                .filter(|found| found.permalink == new_permalink)
                .filter(|found| cross || found.path != src.path)
        };
        match holder {
            Some(holder) => Err(EngineError::Conflict(format!(
                "permalink '{new_permalink}' is already held by '{}' in domain '{dest_domain}'; \
                 pick another permalink, or move that engram first",
                holder.path
            ))),
            None => Ok(()),
        }
    }

    /// Refuse to change the address of an engram somebody has open in the
    /// co-editing editor.
    ///
    /// A room is keyed on the permalink it was opened under and saves back to
    /// that permalink, so a move that changed it underneath would turn the
    /// editor's next save into a write to an address that no longer exists -
    /// shown to the person typing as the engram having been deleted - and a
    /// later engram taking the old name would receive it. A path-only move
    /// keeps the permalink and needs no refusal. Base rooms only: a move in
    /// review mode never reaches this, since it moves a draft.
    async fn refuse_readdress_under_live_room(&self, domain: &str, permalink: &str) -> Result<()> {
        let Some(rooms) = self.collab_rooms() else {
            return Ok(());
        };
        if rooms.has_live_room(domain, permalink, None).await {
            return Err(EngineError::Conflict(format!(
                "'{permalink}' is open in the editor right now, and moving it would change the \
                 address the editor saves to; close the editor and move it again"
            )));
        }
        Ok(())
    }

    /// The engrams whose references a move rewrites: every base engram with a
    /// relation or link that points at `src`, plus every one whose content
    /// holds its `crystalline://` URL (no edge table records those), in
    /// domains this caller may see, each once, `src` itself excluded - its own
    /// references travel with its text.
    pub(super) async fn move_linkers(
        &self,
        src: &EngramDescriptor,
        hidden: &HashSet<String>,
    ) -> Result<Vec<MoveLinker>> {
        let url = format!(
            "{}{}/{}",
            crystalline_core::address::SCHEME,
            src.domain,
            src.permalink
        );
        let (refs, mentions) = {
            let store = self.store.lock().await;
            (
                store
                    .inbound_refs(src.id, src.domain_id, &src.permalink, &src.title)
                    .await?,
                store.engrams_mentioning(&url).await?,
            )
        };
        let candidates = refs
            .into_iter()
            .map(|r| (r.src_domain, r.src_domain_id, r.src_path))
            .chain(
                mentions
                    .into_iter()
                    .map(|m| (m.domain, m.domain_id, m.path)),
            );
        let mut seen: HashSet<(i64, String)> = HashSet::new();
        let mut linkers = Vec::new();
        for (domain, domain_id, path) in candidates {
            if hidden.contains(&domain) || (domain_id == src.domain_id && path == src.path) {
                continue;
            }
            if seen.insert((domain_id.0, path.clone())) {
                linkers.push(MoveLinker {
                    domain,
                    domain_id,
                    path,
                });
            }
        }
        Ok(linkers)
    }

    /// Rewrite the references to a moved engram inside one referencing engram,
    /// in its source of truth, then reindex it. Answers how many references
    /// changed and the engram's permalink, or `None` when its text held none
    /// after all - a content hit inside code, or a longer permalink.
    pub(super) async fn relink_engram(
        &self,
        linker: &MoveLinker,
        spec: &Relink<'_>,
        actor: &str,
    ) -> Result<Option<(usize, String)>> {
        let source = self.read_source(&linker.domain);
        let text = match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &linker.path);
                std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })?
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                match store.engram_content(linker.domain_id, &linker.path).await? {
                    Some(text) => text,
                    None => return Ok(None),
                }
            }
        };
        let (relinked, count) =
            crystalline_core::relink::relink(&text, &linker.domain, &linker.domain, spec);
        if count == 0 {
            return Ok(None);
        }
        let replaced = touch_generated(&relinked, actor, None, now_offset());
        let store = self.store.lock().await;
        match &source {
            ContentSource::File { root } => {
                write_file(&join_rel(root, &linker.path), &replaced)?;
                self.reindex_file(&*store, linker.domain_id, root, &linker.path)
                    .await?;
            }
            ContentSource::Virtual => {
                self.index_markdown(
                    &*store,
                    linker.domain_id,
                    &linker.path,
                    &replaced,
                    virtual_stamp(&replaced),
                    None,
                    true,
                )
                .await?;
            }
        }
        let permalink = parse_engram(&replaced)
            .ok()
            .and_then(|engram| engram.frontmatter.permalink)
            .filter(|permalink| !permalink.is_empty())
            .unwrap_or_else(|| crystalline_core::path_permalink(&linker.path));
        Ok(Some((count, permalink)))
    }

    /// Rename a tag to `new`, or (with `merge`) fold it into an existing `new`,
    /// across every engram that carries it, optionally scoped to one domain.
    /// Each affected file is rewritten string-surgically by
    /// [`crystalline_core::retag`]: only the tag tokens change, every other byte
    /// (including the `generated` provenance block) is preserved, so a hygiene
    /// rename never
    /// reflows a file or looks like a fresh edit. Files are the source of truth,
    /// so each rewrite writes the file (or the virtual row) then reindexes it,
    /// which is where the index picks up the new tag identity.
    ///
    /// The two verbs differ only in a precheck: a `rename` refuses when `new`
    /// already exists (that would silently merge, so it points at `tags merge`),
    /// and a `merge` refuses when `new` does not exist yet. `dry_run` reports the
    /// affected engrams without writing anything.
    ///
    /// A non-dry-run `merge` with `record_alias` also records the fold as a tag
    /// alias: it appends `- old -> new` to each affected domain's MANIFEST
    /// `## Tag Aliases` section (creating the section when absent), so a later
    /// search for the old name still finds its engrams through the alias. The
    /// recording is idempotent and permissive: when the pair is already present
    /// the append no-ops and the domain still counts as recorded (a re-merge of a
    /// tag that reappeared must work, never be refused), and a domain with no
    /// MANIFEST lands in `alias_skipped` rather than erroring. When `old` is
    /// already aliased to a different canonical, first-wins parsing keeps the
    /// existing mapping, so a fresh bullet would be inert: the MANIFEST is left
    /// untouched and the domain is surfaced in `alias_conflict` rather than a
    /// false `alias_recorded`. The merge's tag rewrites still proceed either way;
    /// only the recording is skipped. A `rename` records nothing.
    pub async fn retag(
        &self,
        old: &str,
        new: &str,
        domain: Option<&str>,
        merge: bool,
        dry_run: bool,
        record_alias: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let old_f = old.trim().to_lowercase();
        let new_f = new.trim().to_lowercase();
        // Only the target name must be a canonical lowercase-with-hyphens tag:
        // the whole point of a rename or merge is to move a non-canonical `old`
        // (an underscore or separator variant that the cluster detection flags)
        // onto a clean name, so `old` only has to be a non-empty folded tag.
        if old_f.is_empty() {
            return Err(EngineError::Invalid(
                "the tag to rename or merge is empty".into(),
            ));
        }
        if !is_lower_hyphen(&new_f) {
            return Err(EngineError::Invalid(format!(
                "target tag '{new_f}' is not a lowercase-with-hyphens tag"
            )));
        }
        if old_f == new_f {
            return Err(EngineError::Invalid(
                "the old and new tag are the same".into(),
            ));
        }

        // What this instance has no registration for is not part of this verb's
        // world: not a tag that exists, not an engram to list, not a file to
        // rewrite. Resolved once, before the first store lock (the lock is not
        // reentrant), and threaded through both reads below.
        //
        // The privacy half of `hidden_for` is deliberately absent: renaming a
        // tag is `Scope::Unrestricted` only, so there is no caller here with a
        // narrower view than the machine's own.
        let unregistered = self.unregistered_domains().await?;

        // Precheck against the vocabulary in scope: a rename must not collide
        // with an existing tag, a merge must land on one.
        let new_exists = {
            let vocab = self.scoped_vocabulary(domain, &unregistered).await?;
            vocab.tags.iter().any(|t| t.name == new_f)
        };
        let scope = match domain {
            Some(d) => format!(" in domain '{d}'"),
            None => String::new(),
        };
        if merge {
            if !new_exists {
                return Err(EngineError::NotFound(format!(
                    "cannot merge into '{new_f}': no engram carries it{scope}"
                )));
            }
        } else if new_exists {
            return Err(EngineError::Conflict(format!(
                "tag '{new_f}' already exists{scope}; use `crystalline tags merge` to combine them"
            )));
        }

        // The engrams carrying the old tag, ordered by domain then path, with
        // every unregistered domain's dropped here at the single point where
        // the list is built: `listed`, the dry run, the rewrite loop, the count
        // and the alias recording all read from it, so one filter covers all of
        // them and no later addition can forget it.
        let targets = {
            let store = self.store.lock().await;
            let mut found = store.engrams_with_tag(&old_f, domain).await?;
            drop(store);
            found.retain(|d| !unregistered.contains(&d.domain));
            found
        };
        let listed: Vec<Value> = targets
            .iter()
            .map(|d| json!({ "domain": d.domain, "permalink": d.permalink, "path": d.path }))
            .collect();

        if dry_run {
            return Ok(json!({
                "old": old_f, "new": new_f, "merge": merge, "dry_run": true,
                "engrams": listed, "rewritten": targets.len(),
            }));
        }

        // Rewrite each engram, mirroring move_engram's file-vs-virtual branches.
        let mut rewritten = 0usize;
        for desc in &targets {
            match self.read_source(&desc.domain) {
                ContentSource::File { root } => {
                    let abs = join_rel(&root, &desc.path);
                    let Ok(text) = std::fs::read_to_string(&abs) else {
                        continue;
                    };
                    let Some((edited, _)) = crystalline_core::retag(&text, &old_f, &new_f) else {
                        continue;
                    };
                    write_file(&abs, &edited)?;
                    let store = self.store.lock().await;
                    self.reindex_file(&*store, desc.domain_id, &root, &desc.path)
                        .await?;
                    rewritten += 1;
                }
                ContentSource::Virtual => {
                    let current = {
                        let store = self.store.lock().await;
                        store.engram_content(desc.domain_id, &desc.path).await?
                    };
                    let Some(text) = current else { continue };
                    let Some((edited, _)) = crystalline_core::retag(&text, &old_f, &new_f) else {
                        continue;
                    };
                    let stamp = virtual_stamp(&edited);
                    let store = self.store.lock().await;
                    self.index_markdown(
                        &*store,
                        desc.domain_id,
                        &desc.path,
                        &edited,
                        stamp,
                        None,
                        true,
                    )
                    .await?;
                    rewritten += 1;
                }
            }
        }

        let mut response = json!({
            "old": old_f, "new": new_f, "merge": merge, "dry_run": false,
            "engrams": listed, "rewritten": rewritten,
        });

        // Record the fold as a tag alias in each affected domain's MANIFEST, once
        // per distinct domain in first-seen order. Only a merge records, and only
        // when the caller did not opt out.
        if merge && record_alias {
            let mut domains: Vec<(String, DomainId)> = Vec::new();
            for desc in &targets {
                if !domains.iter().any(|(name, _)| *name == desc.domain) {
                    domains.push((desc.domain.clone(), desc.domain_id));
                }
            }

            let mut alias_recorded: Vec<String> = Vec::new();
            let mut alias_skipped: Vec<String> = Vec::new();
            let mut alias_conflict: Vec<String> = Vec::new();
            let mut virtual_manifest_changed = false;
            for (name, domain_id) in &domains {
                match self.read_source(name) {
                    ContentSource::File { root } => {
                        let abs = join_rel(&root, "MANIFEST.md");
                        let Ok(text) = std::fs::read_to_string(&abs) else {
                            alias_skipped.push(name.clone());
                            continue;
                        };
                        match decide_alias_record(&text, &old_f, &new_f) {
                            AliasRecord::Recorded(edited) => {
                                write_file(&abs, &edited)?;
                                let store = self.store.lock().await;
                                self.reindex_file(&*store, *domain_id, &root, "MANIFEST.md")
                                    .await?;
                                alias_recorded.push(name.clone());
                            }
                            AliasRecord::AlreadyPresent => alias_recorded.push(name.clone()),
                            AliasRecord::Conflict => alias_conflict.push(name.clone()),
                        }
                    }
                    ContentSource::Virtual => {
                        let current = {
                            let store = self.store.lock().await;
                            store.engram_content(*domain_id, "MANIFEST.md").await?
                        };
                        let Some(text) = current else {
                            alias_skipped.push(name.clone());
                            continue;
                        };
                        match decide_alias_record(&text, &old_f, &new_f) {
                            AliasRecord::Recorded(edited) => {
                                let stamp = virtual_stamp(&edited);
                                let store = self.store.lock().await;
                                self.index_markdown(
                                    &*store,
                                    *domain_id,
                                    "MANIFEST.md",
                                    &edited,
                                    stamp,
                                    None,
                                    true,
                                )
                                .await?;
                                virtual_manifest_changed = true;
                                alias_recorded.push(name.clone());
                            }
                            AliasRecord::AlreadyPresent => alias_recorded.push(name.clone()),
                            AliasRecord::Conflict => alias_conflict.push(name.clone()),
                        }
                    }
                }
            }

            // A rewritten virtual MANIFEST may have changed its routing bullets,
            // so refresh the cache once, after every store lock is released.
            if virtual_manifest_changed {
                self.refresh_routing_cache().await;
            }

            if let Value::Object(map) = &mut response {
                map.insert("alias_recorded".to_string(), json!(alias_recorded));
                map.insert("alias_skipped".to_string(), json!(alias_skipped));
                map.insert("alias_conflict".to_string(), json!(alias_conflict));
            }
        }

        Ok(response)
    }

    /// Refuse a move whose destination path is already taken, checking disk for
    /// a file domain and the database for a virtual one.
    async fn ensure_dest_free(
        &self,
        dest_source: &ContentSource,
        dest_domain: &str,
        dest_rel: &str,
    ) -> Result<()> {
        let taken = match dest_source {
            ContentSource::File { root } => join_rel(root, dest_rel).exists(),
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let dest_id = store
                    .upsert_domain(dest_domain, None, DomainKind::Virtual)
                    .await?;
                store.engram_content(dest_id, dest_rel).await?.is_some()
            }
        };
        if taken {
            return Err(EngineError::Conflict(format!(
                "destination '{dest_rel}' already exists in domain '{dest_domain}'"
            )));
        }
        Ok(())
    }
}
