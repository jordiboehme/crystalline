use super::*;

impl Engine {
    // --- attachments ---------------------------------------------------------
    //
    // The byte seam every attachment surface goes through: the REST file
    // routes, the archive and the MCP resource reads. Above it nothing knows
    // which kind of domain it is addressing; below it a file domain keeps plain
    // files under its root (so a git team domain carries them like any other
    // tracked file) and a virtual domain keeps the bytes in the index beside
    // the row. The metadata row is identical either way, and it is written
    // here rather than left to the next walker pass, so a surface that just
    // uploaded a file can list it immediately.

    /// Every attachment a domain carries, metadata only, ordered by path.
    ///
    /// Bytes are never loaded: a listing of a domain full of slide decks costs
    /// one query.
    ///
    /// **The substrate, not the projection.** This answers what the domain
    /// holds, the same for everybody, which is what the machinery that carries
    /// no caller's scope needs - a cross-domain move's carry, the split's
    /// screen, an archive export. A surface answering a person asks
    /// [`Engine::attachment_list_as`], which lays that caller's own drafted
    /// files over it.
    pub async fn attachment_list(&self, domain: &str) -> Result<Vec<AttachmentRow>> {
        let (domain_id, _) = self.domain_source(domain).await?;
        let store = self.store.lock().await;
        Ok(store.list_attachments(domain_id).await?)
    }

    /// [`Engine::attachment_list`] as this caller sees it.
    ///
    /// The name-addressed verb above is the **substrate**: the folder's own
    /// rows, the same for everybody, which is what the machinery that holds no
    /// scope needs. This one is the **projection**: on a domain that reviews
    /// changes a file the caller uploaded is listed for them alone and a file
    /// they deleted is absent for them, exactly as their drafted pages are.
    /// Every surface that answers a person asks this one.
    pub async fn attachment_list_as(
        &self,
        domain: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Vec<AttachmentRow>> {
        let hidden = self.hidden_for(scope).await?;
        DomainView::for_read(self, domain, &hidden, scope)?
            .attachments()
            .await
    }

    /// One attachment's bytes and its metadata row.
    ///
    /// A file domain reads the file under its root; a virtual domain reads the
    /// stored blob. Either way an absent attachment is
    /// [`EngineError::NotFound`], the same miss an absent engram reports.
    ///
    /// The file arm heals the row it serves. A file can arrive behind the index
    /// (a `git pull`, an editor, a domain whose first sync has not run) and can
    /// change behind it the same way, so when the recorded row does not match
    /// the file's own size and modification instant the bytes just read are
    /// hashed and the row is refreshed through the same upsert the walker uses.
    /// That keeps the sha a caller caches on describing exactly what it
    /// received. The match itself is the walker's stat prefilter, so the common
    /// case costs no hashing at all.
    pub async fn attachment_read(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<(Vec<u8>, AttachmentRow)> {
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        match &source {
            ContentSource::File { root } => {
                let abs = contained_asset_path(root, path)?;
                // The stat comes first so an over-cap file is refused without
                // ever being read: the ceiling is enforced by the walker (which
                // skips such a file, so it has no row) and by the write, and a
                // read that hashed one anyway would both spend the memory and
                // mint a row the next full scan deletes again.
                let meta = match std::fs::metadata(&abs) {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(missing_attachment(domain, path)));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                if meta.len() > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(path, meta.len())));
                }
                let bytes = match std::fs::read(&abs) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(missing_attachment(domain, path)));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                // The bytes that were actually read decide, like the walker's
                // post-read check: a file that grew past the ceiling between
                // the stat and the read is caught here rather than served.
                if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(
                        path,
                        bytes.len() as u64,
                    )));
                }
                let modified = asset_modified(&abs);
                let store = self.store.lock().await;
                if let Some(row) = store.get_attachment(domain_id, path).await?
                    && row.size == bytes.len() as u64
                    && row.modified == modified
                {
                    return Ok((bytes, row));
                }
                let row = attachment_row(path, &bytes, modified)?;
                store.upsert_attachment(domain_id, &row).await?;
                Ok((bytes, row))
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let row = store
                    .get_attachment(domain_id, path)
                    .await?
                    .ok_or_else(|| EngineError::NotFound(missing_attachment(domain, path)))?;
                let bytes = store
                    .read_attachment_blob(domain_id, path)
                    .await?
                    .ok_or_else(|| EngineError::NotFound(missing_attachment(domain, path)))?;
                Ok((bytes, row))
            }
        }
    }

    /// [`Engine::attachment_read`] as this caller sees it: their own bytes
    /// where they hold some, a miss where they have deleted the path, and the
    /// folder's answer otherwise. The projection to
    /// [`Engine::attachment_read`]'s substrate, as
    /// [`Engine::attachment_list_as`] is to [`Engine::attachment_list`].
    pub async fn attachment_read_as(
        &self,
        domain: &str,
        path: &str,
        scope: &crate::scope::Scope,
    ) -> Result<(Vec<u8>, AttachmentRow)> {
        let hidden = self.hidden_for(scope).await?;
        DomainView::for_read(self, domain, &hidden, scope)?
            .attachment_bytes(path)
            .await
    }

    /// Create or replace one attachment, returning the row that now describes
    /// it.
    ///
    /// Every gate runs before a byte is stored: the path rules and the
    /// extension allowlist ([`crystalline_core::validate_asset_path`]) and the
    /// size ceiling. A file domain then writes the bytes atomically under its
    /// root, with the joined path proven to stay inside it, and takes the same
    /// per-file lock every other file write takes so a concurrent replace
    /// cannot leave the row describing the loser's bytes. A virtual domain
    /// writes the row and then the blob, in that order, since the store keeps a
    /// blob without a row an error rather than an orphan; a blob write that
    /// fails takes the row back out with it, so a failure leaves the domain
    /// exactly as it found it rather than listing a path with no bytes.
    ///
    /// Both kinds mark the domain pending in the maintenance state afterwards:
    /// a human just added something the agent has not read yet, which is
    /// exactly what a consolidation sweep is for.
    pub async fn attachment_write(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<AttachmentRow> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        validate_attachment_path(path)?;
        if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
            return Err(EngineError::Invalid(over_cap_error(
                path,
                bytes.len() as u64,
            )));
        }
        let (domain_id, source) = self.domain_source(domain).await?;
        let row = match &source {
            ContentSource::File { root } => {
                let abs = contained_asset_path(root, path)?;
                // Held across the write and the row upsert, the file lock
                // before the store lock like every other writer here. See
                // `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                write_bytes(&abs, &bytes)?;
                // The modification instant is read back off the file rather
                // than taken from the clock, so it is the same value the sync
                // walker's stat prefilter compares against and an upload costs
                // no re-hash on the next scan.
                let row = attachment_row(path, &bytes, asset_modified(&abs))?;
                let store = self.store.lock().await;
                store.upsert_attachment(domain_id, &row).await?;
                row
            }
            ContentSource::Virtual => {
                let row = attachment_row(path, &bytes, Utc::now().to_rfc3339())?;
                let store = self.store.lock().await;
                // What was there before this write, so a failure can put it
                // back rather than approximate it.
                let previous = store.get_attachment(domain_id, path).await?;
                store.upsert_attachment(domain_id, &row).await?;
                // A failed blob write must leave the domain exactly as it found
                // it, which is two different things depending on what was
                // there. A replace: the upsert above moved the row's metadata
                // only - the blob belongs to the row and still holds the older
                // bytes - so restoring the recorded row restores the whole
                // attachment, and deleting instead would destroy an attachment
                // this write never got to replace. A create: nothing was there,
                // so the row this write inserted goes with it, or a listing
                // would advertise bytes that were never stored. Best effort
                // either way (the next write, or a scan of the file domain's
                // twin, reconciles), and the original error is what the caller
                // sees. A file domain needs none of this: the temp file is
                // renamed into place only on success, so a failed write leaves
                // the old file untouched.
                if let Err(e) = store.write_attachment_blob(domain_id, path, &bytes).await {
                    let undone = match &previous {
                        Some(prior) => store.upsert_attachment(domain_id, prior).await.err(),
                        None => store.delete_attachment(domain_id, path).await.err(),
                    };
                    if let Some(failed) = undone {
                        tracing::warn!(
                            "attachment '{path}' in '{domain}' could not be rolled back after a failed blob write: {failed}"
                        );
                    }
                    return Err(e.into());
                }
                row
            }
        };
        crate::maintenance::record_pending(domain);
        Ok(row)
    }

    /// Remove one attachment: the file or the blob, and the row.
    ///
    /// [`EngineError::NotFound`] when neither was there, so a caller can answer
    /// a miss. A file that is gone while its row stands (or the reverse, after
    /// a hand-edited domain) still counts as a delete: whichever half existed
    /// is removed and the pair ends up consistent.
    pub async fn attachment_delete(&self, domain: &str, path: &str) -> Result<()> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let mut file_removed = false;
        if let ContentSource::File { root } = &source {
            let abs = contained_asset_path(root, path)?;
            let lock = self.write_lock(&abs);
            let _guard = lock.lock().await;
            match std::fs::remove_file(&abs) {
                Ok(()) => file_removed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    });
                }
            }
        }
        let row_removed = {
            let store = self.store.lock().await;
            store.delete_attachment(domain_id, path).await?
        };
        if !file_removed && !row_removed {
            return Err(EngineError::NotFound(missing_attachment(domain, path)));
        }
        crate::maintenance::record_pending(domain);
        Ok(())
    }

    /// What an attachment write landed as: the row that now describes it, and
    /// whether it is this actor's draft rather than the domain's own file.
    ///
    /// Create or replace one attachment as this caller sees the domain.
    ///
    /// The routing decision review mode turns an upload into, in one place. A
    /// domain that takes changes directly runs
    /// [`Engine::attachment_write`] byte for byte, and its receipt carries no
    /// `draft` at all. A domain that reviews changes lands the bytes in this
    /// actor's files overlay instead: **no write in review mode reaches the
    /// folder**, which is the rule the whole mode rests on, and the receipt
    /// says `draft: true`.
    ///
    /// **The write right and the identity refusal are the view's.**
    /// [`DomainView::for_write`] screens the registered set, then refuses a
    /// caller with no identity with [`OVERLAY_NEEDS_IDENTITY`] - the same
    /// sentence, in the same words, an engram write is refused with, because it
    /// is the same question: there is no identity for a draft to belong to.
    /// The surface gate in front of it (REST `require_domain_write`) still
    /// stands where it always did.
    pub(crate) async fn attachment_write_in(
        &self,
        view: &DomainView<'_>,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<WrittenAttachment> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        validate_attachment_path(path)?;
        if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
            return Err(EngineError::Invalid(over_cap_error(
                path,
                bytes.len() as u64,
            )));
        }
        if let Some(actor) = view.actor() {
            let row = view.put_file(path, &bytes).await?;
            // The write IS the resolution, for a file. A conflict resolution
            // settles an engram's markdown and `origin_resolve` says so to
            // anybody who names an attachment path; what settles a file is
            // uploading it again to keep your version or deleting it to take
            // the team's, so both of those take the path out of the recorded
            // conflicts on the way out.
            self.settle_file_convergence(view.domain(), actor, path)
                .await;
            crate::maintenance::record_pending(view.domain());
            return Ok(WrittenAttachment { row, draft: true });
        }
        let row = self.attachment_write(view.domain(), path, bytes).await?;
        Ok(WrittenAttachment { row, draft: false })
    }

    /// The view-taking write above under the acting scope, for the surfaces
    /// that hold a scope rather than a view.
    pub async fn attachment_write_as(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
        scope: &crate::scope::Scope,
    ) -> Result<WrittenAttachment> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.attachment_write_joined(domain, path, bytes, scope, None)
            .await
    }

    /// [`Engine::attachment_write_as`], with the join a session may be
    /// holding.
    ///
    /// **Files follow the join**, which is the whole of the rule: an upload
    /// made while working inside somebody else's draft lands in the OWNER's
    /// files overlay, through
    /// [`crate::overlay_files::target_actor`], and is staged, folded and
    /// discarded with that draft. Anything else would put an image in one
    /// overlay and the page that references it in another, so folding the
    /// draft would land a page pointing at a file nobody folded.
    ///
    /// The path screen is the save's, for the same reason and in the same
    /// words: a caller who can see a draft but has not joined it is told what
    /// their two ways forward are rather than having one chosen for them.
    pub async fn attachment_write_joined(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<WrittenAttachment> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write_joined(self, domain, scope, join).await?;
        // Two different screens, because a join and a grant bound two
        // different things. An attachment does not stand at the draft's path -
        // it stands beside it, in the files overlay - so the path equality the
        // save enforces would refuse every upload made inside a join, which is
        // the one thing a join is supposed to make possible. What bounds a
        // JOINED write is which files the granted page carries; see
        // [`Engine::screen_joined_attachment`]. What bounds an unjoined one is
        // the grant, exactly as it bounds a save.
        match (view.joined(), join) {
            (Some(_), Some(join)) => {
                self.screen_joined_attachment(domain, path, join, false)
                    .await?
            }
            _ => self.screen_granted_path(domain, path, scope, None).await?,
        }
        self.attachment_write_in(&view, path, bytes).await
    }

    /// Remove one attachment as this caller sees the domain, answering whether
    /// the deletion landed as a draft.
    ///
    /// A direct domain removes the file or the blob and the row, as it always
    /// has. In review mode the folder is not touched: a reviewed file is hidden
    /// behind this actor's own deletion marker until the deletion is reviewed
    /// like any other change, and a file only this actor holds simply goes,
    /// since a marker over a base nothing holds is exactly what convergence
    /// would clear again.
    pub(crate) async fn attachment_delete_in(
        &self,
        view: &DomainView<'_>,
        path: &str,
    ) -> Result<bool> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        validate_attachment_path(path)?;
        if let Some(actor) = view.actor() {
            view.tombstone_file(path).await?;
            // The other half of what settles a diverged file: see
            // [`Engine::attachment_write_in`].
            self.settle_file_convergence(view.domain(), actor, path)
                .await;
            crate::maintenance::record_pending(view.domain());
            return Ok(true);
        }
        self.attachment_delete(view.domain(), path).await?;
        Ok(false)
    }

    /// The view-taking delete above under the acting scope.
    pub async fn attachment_delete_as(
        &self,
        domain: &str,
        path: &str,
        scope: &crate::scope::Scope,
    ) -> Result<bool> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.attachment_delete_joined(domain, path, scope, None)
            .await
    }

    /// [`Engine::attachment_delete_as`], with the join a session may be
    /// holding. The deletion follows the join exactly as the upload does, and
    /// for the same reason: see [`Engine::attachment_write_joined`].
    pub async fn attachment_delete_joined(
        &self,
        domain: &str,
        path: &str,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<bool> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write_joined(self, domain, scope, join).await?;
        // The same pair of screens the upload makes, and ahead of the delete
        // rather than inside it: the deletion marker is what a fold would
        // carry out, so a refused deletion that had staged one anyway would be
        // a deletion nobody refused.
        match (view.joined(), join) {
            (Some(_), Some(join)) => {
                self.screen_joined_attachment(domain, path, join, true)
                    .await?
            }
            _ => self.screen_granted_path(domain, path, scope, None).await?,
        }
        self.attachment_delete_in(&view, path).await
    }

    // --- attachments a cross-domain move carries ------------------------------
    //
    // An `assets/` reference is domain-root relative, so it survives a rename
    // inside its own domain untouched and means nothing at all in another
    // domain. A cross-domain move therefore has to bring the files with the
    // engram, which is three questions asked before a byte moves: which
    // attachments does the moving engram actually use, does anything else in
    // the source still need them (copy) or not (move), and is the name they
    // arrive under free at the destination.

    /// The most `-N` suffixes a colliding attachment name is offered before
    /// the move leaves it where it is. A destination holding ninety-nine
    /// different files under one name is a domain with a problem an automatic
    /// rename would only deepen.
    const MAX_ASSET_SUFFIX: usize = 99;

    /// What the cross-domain move owes each attachment the moving engram uses:
    /// where it lands, whether the bytes are already there, and whether the
    /// source copy stays behind.
    ///
    /// Settled before anything is written, because the destination names it
    /// picks are what the engram's body references and `analyzes` claim are
    /// rewritten to, and that rewrite has to travel in the same write that
    /// lands the engram at its destination.
    ///
    /// Every miss is quiet rather than fatal: a reference to a file that is not
    /// there is already a dangling reference, and a move is not the verb that
    /// should refuse over one. Quiet is not silent, though - each miss is
    /// returned beside the plan as a sentence the move receipt carries, so the
    /// caller learns which attachment stayed behind without having to read the
    /// daemon's trace.
    pub(super) async fn plan_attachment_carry(
        &self,
        src: &EngramDescriptor,
        dest_domain: &str,
        content: &str,
    ) -> (Vec<AttachmentCarry>, Vec<String>) {
        let candidates = referenced_asset_paths(content);
        if candidates.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut warnings: Vec<String> = Vec::new();

        // The bytes are read and dropped here: what the plan needs is the
        // sha256, and the read is what makes the row's sha describe the file
        // that is actually on disk. Reading them again in the carry itself
        // costs one more read of one attachment and keeps the peak at a single
        // attachment rather than at everything the engram references.
        let mut present: Vec<(String, String)> = Vec::new();
        for path in candidates {
            match self.attachment_read(&src.domain, &path).await {
                Ok((_, row)) => present.push((path, row.sha256)),
                Err(e) => {
                    // Loud enough to answer "why did my screenshot not
                    // travel": the move went through, but something the engram
                    // points at did not come with it. The trace keeps the store
                    // error, which is an operator's detail; the receipt gets
                    // the same sentence without it.
                    let warning = attachment_missing_warning(&path, &src.permalink, &src.domain);
                    tracing::warn!("{warning} ({e})");
                    warnings.push(warning);
                }
            }
        }
        if present.is_empty() {
            return (Vec::new(), warnings);
        }

        let paths: Vec<String> = present.iter().map(|(path, _)| path.clone()).collect();
        let shared = self.shared_asset_paths(src, &paths).await;
        let mut claimed: HashSet<String> = HashSet::new();
        let mut plan = Vec::new();
        for (from, sha) in present {
            let Some((to, reuse)) = self
                .free_asset_destination(dest_domain, &from, &sha, &claimed)
                .await
            else {
                // The file is whole and still in the source domain, which is
                // the part that matters; what the caller cannot see without
                // being told is that the reference travelling with the engram
                // now points at whatever the destination happens to hold under
                // that name.
                warnings.push(attachment_not_carried_warning(&from, dest_domain));
                continue;
            };
            claimed.insert(to.clone());
            plan.push(AttachmentCarry {
                shared: shared.contains(&from),
                from,
                to,
                reuse,
            });
        }
        (plan, warnings)
    }

    /// The attachment paths another engram in the source domain still
    /// references or claims, with every failure resolved the safe way by
    /// [`resolve_shared`].
    ///
    /// Counted across live and retired engrams alike, the way the
    /// consolidation sweep counts referents: a deprecated engram showing a
    /// screenshot needs the file exactly as much as a current one does, so its
    /// reference is what turns a move into a copy.
    pub(super) async fn shared_asset_paths(
        &self,
        src: &EngramDescriptor,
        candidates: &[String],
    ) -> HashSet<String> {
        resolve_shared(
            self.count_shared_asset_paths(src, candidates).await,
            candidates,
        )
    }

    /// The counting itself: which candidates another engram in `src`'s domain
    /// still references or claims, or the first failure that stopped the count
    /// from answering.
    ///
    /// Engrams are read one at a time rather than in one batch, because a
    /// domain can hold multi-megabyte engrams and this runs on an ordinary
    /// move; the screen keeps the parse to the engrams that could possibly
    /// match, and the scan stops as soon as every candidate is accounted for.
    ///
    /// The screen tests the path *below* the reserved folder (`shot.png` for
    /// `assets/shot.png`) rather than the whole path, which makes it strictly
    /// wider than the decision it protects: a body reference must spell the
    /// folder as `assets/` to be a reference at all, and a claim is folded to
    /// that spelling when it is read, so an engram claiming `Assets/shot.png`
    /// (the same folder on APFS and NTFS) is screened in and then decided
    /// exactly. A narrower screen would let a live claim lose its file.
    ///
    /// Nothing here answers "not referenced" on a failure. A store error is
    /// returned, and text that will not parse marks every candidate the screen
    /// matched in it as referenced: the engram plainly mentions the path and
    /// the only reading that cannot delete something in use is that it uses
    /// it.
    async fn count_shared_asset_paths(
        &self,
        src: &EngramDescriptor,
        candidates: &[String],
    ) -> Result<HashSet<String>> {
        let mut shared: HashSet<String> = HashSet::new();
        let source = self.content_source(&src.domain)?;
        let others = {
            let store = self.store.lock().await;
            store.list_engrams(&src.domain, None, None).await?
        };
        for other in others {
            if shared.len() == candidates.len() {
                break;
            }
            if other.path == src.path {
                continue;
            }
            let Some(text) = self
                .peer_engram_text(&source, src.domain_id, &other.path)
                .await?
            else {
                // Whatever the listing knew about, its text is not there any
                // more, and text that is gone references nothing. Only a real
                // read failure counts as not knowing, and that is an `Err`.
                continue;
            };
            let screened: Vec<&String> = candidates
                .iter()
                .filter(|path| text.contains(asset_tail(path)))
                .collect();
            if screened.is_empty() {
                continue;
            }
            let Ok(engram) = parse_engram(&text) else {
                for candidate in screened {
                    shared.insert(candidate.clone());
                }
                continue;
            };
            let refs = crystalline_core::find_asset_refs(&engram.body);
            let claim = asset_claim(&engram.frontmatter);
            for candidate in screened {
                if refs.contains(candidate) || claim.as_deref() == Some(candidate.as_str()) {
                    shared.insert(candidate.clone());
                }
            }
        }
        Ok(shared)
    }

    /// One peer engram's whole text, frontmatter included, for the referent
    /// count.
    ///
    /// Deliberately not [`Store::engram_content`] alone. The index keeps only
    /// the *body* for a file domain (`EngramRecord::from_engram` sets
    /// `content` to `engram.body`; the virtual write path is the one that
    /// stores the whole source), and an `analyzes` claim lives in the
    /// frontmatter - so counting off the index alone would never see a claim
    /// in a file domain and would delete a claimed attachment out from under
    /// the engram that claimed it. A file domain is therefore read from its
    /// files, which is where its frontmatter actually is, and a virtual domain
    /// from the database, which is where its whole engram actually is.
    ///
    /// That is one file read per engram in the domain, on a cross-domain move
    /// that carries attachments and on the confirmation preview of a delete
    /// whose engram references one ([`Engine::sole_referent_attachments`]). It
    /// is the price of counting claims at all, both verbs are rare, and an
    /// engram that references no attachment pays none of it.
    ///
    /// `None` when the text is genuinely absent (a row the index still lists
    /// for a file that is gone): text that is not there references nothing. A
    /// read that fails for any other reason is an `Err`, which
    /// [`resolve_shared`] turns into "still referenced".
    pub(super) async fn peer_engram_text(
        &self,
        source: &ContentSource,
        domain_id: DomainId,
        path: &str,
    ) -> Result<Option<String>> {
        match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, path);
                match std::fs::read_to_string(&abs) {
                    Ok(text) => Ok(Some(text)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(source) => Err(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    }),
                }
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                Ok(store.engram_content(domain_id, path).await?)
            }
        }
    }

    /// The path an attachment takes at the destination, and whether the
    /// destination already holds exactly these bytes there.
    ///
    /// Its own name when nothing holds it; its own name with nothing to write
    /// when the file already there has the same sha256 (same name, same bytes,
    /// same file); `name-2.ext`, `name-3.ext` and so on when something
    /// different holds it, since a move must never overwrite a file the
    /// destination domain already had. Every suffixed name is put back through
    /// [`crystalline_core::validate_asset_path`], so the name the engram's
    /// references are rewritten to is always a name a write will accept.
    ///
    /// `None` when no free, valid name could be settled or the destination
    /// could not be inspected. That is the fallback that keeps everything
    /// referenced: the attachment is simply not carried, so nothing is written
    /// at the destination, nothing is deleted at the source and the moving
    /// engram's references keep the spelling they had. The whole file stays in
    /// the source domain under the name that still addresses it there, and the
    /// destination is left with a plain dangling reference for the sweep to
    /// report - the same outcome an already-missing attachment produces, and
    /// the one thing that cannot happen is a reference rewritten to a name
    /// nothing will ever accept.
    async fn free_asset_destination(
        &self,
        dest_domain: &str,
        path: &str,
        sha: &str,
        claimed: &HashSet<String>,
    ) -> Option<(String, bool)> {
        for attempt in 1..=Self::MAX_ASSET_SUFFIX {
            let candidate = if attempt == 1 {
                path.to_string()
            } else {
                // A stem that cannot be shortened into a valid name at all
                // cannot be shortened into a valid longer-suffixed one either,
                // so this ends the search rather than skipping an attempt.
                suffixed_asset_path(path, attempt)?
            };
            // A name another attachment in this same move already took is
            // occupied even though nothing is written there yet.
            if claimed.contains(&candidate) {
                continue;
            }
            match self.attachment_read(dest_domain, &candidate).await {
                Ok((_, row)) if row.sha256 == sha => return Some((candidate, true)),
                Ok(_) => {}
                Err(EngineError::NotFound(_)) => return Some((candidate, false)),
                Err(e) => {
                    // The attachment stays whole where it is, so this is a
                    // note for whoever is reading the trace rather than
                    // something the user lost.
                    tracing::debug!(
                        "'{candidate}' in '{dest_domain}' could not be inspected ({e}); '{path}' stays where it is"
                    );
                    return None;
                }
            }
        }
        tracing::warn!(
            "'{path}' collides with {} different files in '{dest_domain}'; it stays where it is",
            Self::MAX_ASSET_SUFFIX
        );
        None
    }

    /// Carry out the planned attachment moves and copies.
    ///
    /// Runs after the engram itself has landed and never turns a failure into
    /// a failed move: the engram is already where it was asked to be, and
    /// every failure here leaves the source copy in place, so the worst
    /// outcome is a reference the destination cannot resolve yet - which the
    /// consolidation sweep reports as a dangling attachment rather than
    /// something a move should have refused over. Both domains are marked
    /// pending by the writes and deletes themselves.
    pub(super) async fn carry_attachments(
        &self,
        src_domain: &str,
        dest_domain: &str,
        plan: &[AttachmentCarry],
    ) {
        for carry in plan {
            if !carry.reuse {
                let bytes = match self.attachment_read(src_domain, &carry.from).await {
                    Ok((bytes, _)) => bytes,
                    Err(e) => {
                        tracing::warn!(
                            "attachment '{}' could not be read out of '{src_domain}' for the move: {e}",
                            carry.from
                        );
                        continue;
                    }
                };
                if let Err(e) = self.attachment_write(dest_domain, &carry.to, bytes).await {
                    tracing::warn!(
                        "attachment '{}' could not be written into '{dest_domain}' as '{}': {e}",
                        carry.from,
                        carry.to
                    );
                    continue;
                }
            }
            // Only now, with the bytes proven to be at the destination, does
            // the source copy go - and only when nothing there still uses it.
            if !carry.shared
                && let Err(e) = self.attachment_delete(src_domain, &carry.from).await
            {
                tracing::warn!(
                    "attachment '{}' was carried into '{dest_domain}' but could not be removed from '{src_domain}': {e}",
                    carry.from
                );
            }
        }
    }

    /// The retirement statuses [`Engine::retire_engram`] accepts. Any other
    /// status is this verb's business to refuse, not a global rule: the
    /// ordinary save and edit paths accept any status string.
    const RETIREMENT_STATUSES: [&str; 3] = ["deprecated", "superseded", "archived"];

    /// Guided retirement: set a retirement `status`, optionally close out
    /// `valid_to`, and, for `superseded`, wire the supersede pair as body
    /// relations so verify's T005 and the evolve sweep see a reciprocal link
    /// rather than a dangling one.
    pub async fn retire_engram(&self, p: &RetireParams) -> Result<Value> {
        self.retire_engram_as(p, None, &crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`Engine::retire_engram`] with the retiring identity, resolved by
    /// [`Engine::actor`] and stamped into both engrams' `generated` block.
    ///
    /// Everything is validated and resolved before anything is written: the
    /// status is checked against [`Self::RETIREMENT_STATUSES`], the
    /// successor rule (required for `superseded`, refused otherwise) is
    /// enforced, `valid_to` is parsed and, when a successor is named, it is
    /// resolved before the target is touched, so a missing successor is
    /// `NotFound` rather than a half-written pair. That resolution goes
    /// through [`Engine::resolve_in`], which is what actually holds the
    /// successor to this domain: the absolute `crystalline://` form overrides
    /// a domain hint wherever it is accepted, so "the same domain" is a rule
    /// enforced there rather than a property of passing the name in. The
    /// target is then written first, and only then the successor's reciprocal
    /// `- supersedes [[..]]` line
    /// (appended only when not already present, so a repeat call is
    /// idempotent). A failure on the successor write leaves the target
    /// retired with a one-sided pair; nothing here rolls that back, since the
    /// evolve sweep already flags a `superseded_by` with no matching
    /// `supersedes` as its own finding.
    ///
    /// `scope` is the acting scope every write verb carries; see
    /// [`Engine::write_engram_as`].
    pub async fn retire_engram_as(
        &self,
        p: &RetireParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if !Self::RETIREMENT_STATUSES.contains(&p.status.as_str()) {
            return Err(EngineError::Invalid(format!(
                "retire_engram accepts status deprecated, superseded or archived, got '{}'; \
                 use edit_engram's set_frontmatter operation for any other status",
                p.status
            )));
        }
        match (p.status.as_str(), p.successor.is_some()) {
            ("superseded", false) => {
                return Err(EngineError::Invalid(
                    "status superseded needs a successor to wire the supersede pair, \
                     or verify rule T005 flags the result as a dangling retirement"
                        .into(),
                ));
            }
            (other, true) if other != "superseded" => {
                return Err(EngineError::Invalid(format!(
                    "successor is only accepted when status is superseded, not '{other}'"
                )));
            }
            _ => {}
        }
        let valid_to = p
            .valid_to
            .as_deref()
            .map(|raw| {
                NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
                    EngineError::Invalid(format!(
                        "valid_to must be a plain ISO date (YYYY-MM-DD), got '{raw}'"
                    ))
                })
            })
            .transpose()?;

        let view = DomainView::for_write(self, &p.domain, scope).await?;
        let overlay = view.actor();
        let actor = self.actor_for(client, overlay);
        let (desc, source) = view.resolve(&p.identifier).await?;

        // Resolved before the target is touched: a missing successor must
        // never leave the target half-retired. Through the same view, so a
        // retirement in review mode can name a successor that only exists as
        // this actor's draft.
        let successor = match &p.successor {
            Some(identifier) => Some(view.resolve(identifier).await?),
            None => None,
        };
        // A successor that resolves to the target itself would append a
        // supersedes-self relation: no deadlock (the target's lock is
        // released before the successor's is taken), just a nonsense pair
        // that verify would then have to make sense of. Refused before
        // anything is written rather than left to produce that pair.
        if let Some((succ_desc, _)) = &successor
            && succ_desc.id == desc.id
        {
            return Err(EngineError::Invalid(format!(
                "successor '{}' resolves to the same engram being retired; \
                 a retirement needs a different engram to supersede it",
                p.successor.as_deref().unwrap_or_default()
            )));
        }
        // The permalink, not the title. A title is prose and may carry a
        // colon, which `[[...]]` parses as a cross-domain prefix (issue #65);
        // a permalink is the stable identity and never carries one. Fluid goes
        // on rendering the title, which it reads off the engram the link lands
        // on rather than off the bracket text.
        //
        // The title comes along only to recognize the bullet a previous
        // retirement wrote in the older spelling, so re-retiring an engram
        // that already declares its successor by title appends nothing.
        let successor_permalink = successor.as_ref().map(|(d, _)| d.permalink.clone());
        let successor_title = successor.as_ref().map(|(d, _)| d.title.clone());

        // -- target: status, optional valid_to, optional superseded_by line --
        //
        // The retirement itself, as one closure the shared edit path applies to
        // whatever text it reads: a draft in review mode, the open document
        // while somebody has the page up, the file or the row otherwise. One
        // copy of the edit and one path to write it back is what keeps a
        // retired draft the same shape as an edited one.
        let retire_target = |current: &str| -> Result<String> {
            Ok(Self::build_retirement_edit(
                current,
                &p.status,
                valid_to,
                successor_permalink.as_deref(),
                successor_title.as_deref(),
                &actor,
            ))
        };
        // **One arm, for every kind of domain and whoever is in the room.** The
        // shared edit path reads this actor's own text - their draft in review
        // mode, the open document while somebody has the page up, the file or
        // the row otherwise - applies the retirement to it and writes it back
        // where it came from. A direct-mode retirement used to be a raw
        // read-edit-write of the file beside the room, so a person typing had
        // the page retired underneath them and their next save came back as a
        // three-way merge or a conflict they had to settle by hand. Now a
        // retirement composes like every other in-place rewrite, and
        // `a_retirement_in_a_direct_domain_composes_into_the_open_room` says so.
        let mut warning = self
            .apply_source_edit(&desc, &source, &view, None, &actor, None, retire_target)
            .await?;

        // -- successor: reciprocal supersedes line, appended once --
        if let Some((succ_desc, succ_source)) = &successor {
            let line = format!("- supersedes [[{}]]", desc.permalink);
            // Recognized in either spelling, for the reason `declares` gives:
            // a successor wired by an older retirement carries the title form.
            let already = |current: &str| {
                Self::declares(current, "supersedes", &desc.permalink, Some(&desc.title))
            };
            // The successor's side of the pair goes through the same one arm,
            // for the same reasons: in review mode nothing this verb writes
            // belongs in the folder the team reviewed, and a successor
            // somebody has open is a document rather than a file. The text the
            // "already said this" test reads is that same text, so a
            // re-retirement appends nothing twice whichever of the three the
            // successor is living in at that moment.
            let current = match self.live_text_at(succ_desc, &view).await {
                Some(live) => live,
                None => match overlay {
                    Some(_) => view.text_at(succ_source, succ_desc).await?.ok_or_else(|| {
                        EngineError::NotFound(format!(
                            "no engram '{}' in domain '{}'",
                            succ_desc.permalink, succ_desc.domain
                        ))
                    })?,
                    None => self.load_content(succ_source, succ_desc).await?,
                },
            };
            if !already(&current) {
                let succ_warning = self
                    .apply_source_edit(succ_desc, succ_source, &view, None, &actor, None, |c| {
                        Ok(append_body(c, &line))
                    })
                    .await?;
                warning = warning.or(succ_warning);
            }
        }
        self.nudge_embed();

        let mut receipt = json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "status": p.status,
            "successor": successor.map(|(d, _)| d.permalink),
        });
        if overlay.is_some() {
            receipt["draft"] = json!(true);
        }
        note_unmirrored(&mut receipt, warning);
        Ok(receipt)
    }

    /// Build the target engram's retirement edit: set `status`, set
    /// `valid_to` when given, append the `superseded_by` relation when a
    /// successor title is given and the line is not already there, then
    /// stamp `generated` provenance. Shared by the file and virtual arms of
    /// [`Engine::retire_engram_as`]. The `contains` guard, checked against
    /// `current` rather than the frontmatter-edited text (the two never
    /// disagree on body content), matches the successor side's guard so a
    /// retry after a timeout, say, retires idempotently instead of
    /// duplicating the relation.
    fn build_retirement_edit(
        current: &str,
        status: &str,
        valid_to: Option<NaiveDate>,
        successor_permalink: Option<&str>,
        successor_title: Option<&str>,
        actor: &str,
    ) -> String {
        let mut edited = set_frontmatter_field(current, "status", status);
        if let Some(date) = valid_to {
            edited =
                set_frontmatter_field(&edited, "valid_to", &date.format("%Y-%m-%d").to_string());
        }
        if let Some(permalink) = successor_permalink {
            let line = format!("- superseded_by [[{permalink}]]");
            if !Self::declares(current, "superseded_by", permalink, successor_title) {
                edited = append_body(&edited, &line);
            }
        }
        touch_generated(&edited, actor, None, now_offset())
    }

    /// Whether the text already declares this relation to this engram, in
    /// either spelling.
    ///
    /// The engine writes the permalink form now and wrote the title form
    /// before, so an archive holds both and a re-retirement must recognize the
    /// one it finds rather than appending a second bullet saying what the first
    /// already says. Exact on both, because both are spellings the engine
    /// itself produced: this recognizes its own past output, it does not try to
    /// parse what a person may have typed.
    pub(super) fn declares(
        current: &str,
        rel_type: &str,
        permalink: &str,
        title: Option<&str>,
    ) -> bool {
        let mut forms = vec![format!("- {rel_type} [[{permalink}]]")];
        if let Some(title) = title {
            forms.push(format!("- {rel_type} [[{title}]]"));
        }
        forms.iter().any(|line| current.contains(line.as_str()))
    }

    /// Move part of an engram into a new one, in a single guided step: the
    /// selected observations and sections leave the source, land in a new
    /// engram carrying the source's tags and a `stable` status, and the two are
    /// wired together with `derived_from` on the new engram and `split_into` on
    /// the source.
    ///
    /// The verb behind "split before you retire". Validity is set per engram
    /// rather than per bullet, so an engram that bundles facts with different
    /// lifecycles has to give up its still-valid facts when the one fact that
    /// expired retires the file. Doing that by hand is a write, two edits and a
    /// pair of links, with every step a chance to lose a bullet; this is that
    /// sequence as one call, and `V010` is the sweep rule that finds the
    /// engrams needing it.
    pub async fn split_engram(&self, p: &SplitParams) -> Result<Value> {
        self.split_engram_as(p, None, &crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`Engine::split_engram`] with the splitting identity, resolved by
    /// [`Engine::actor`] and stamped into both engrams' `generated` block.
    ///
    /// **Everything that can be refused is refused before anything is
    /// written**: the source resolves inside the domain the request named (see
    /// [`Engine::resolve_in`]), its checksum is compared, every selected line is
    /// checked to be an observation the source really carries, every section
    /// path is resolved, and the remainder is measured against verify's `Q001`
    /// minimum so a split can never quietly empty an engram. Only then does the
    /// new engram get written, and only then the source edited.
    ///
    /// **The new engram goes first, and a failed source edit takes it back out
    /// only while the source is untouched.** First because the failure that
    /// leaves the knowledge in two places is survivable and the one that leaves
    /// it in none is not. The source edit carries the checksum of the text this
    /// call planned against, so a concurrent edit refuses it rather than
    /// dropping somebody's work; a refusal before the source's bytes change -
    /// that conflict, a read that fails, a write the filesystem refuses -
    /// deletes the new engram again and hands the caller the failure with the
    /// archive exactly as it was.
    ///
    /// **Once the source has been rewritten, nothing is undone**, and that is
    /// the invariant rather than an omission: the source no longer holds the
    /// moved observations, so deleting the engram that does hold them is the
    /// one outcome this verb must never produce. `apply_source_edit_staged`
    /// reports which side of the write it failed on
    /// ([`SourceEditFailure::wrote`]), and on the far side both engrams are
    /// kept and the error names them and says the source's index row may be
    /// stale. What is left then is a correct pair with a stale index row for
    /// the source, which a sync, a watcher tick or `reindex` repairs.
    ///
    /// **Only a file domain can reach that state.** A virtual source is edited
    /// inside one store transaction that rolls back on any error, so a failure
    /// there is always the untouched case: a concurrent edit comes back as the
    /// `Conflict` it is and the new engram is taken back out, with the stored
    /// bytes exactly as they were.
    ///
    /// **A moved section takes its relation bullets with it**, since a section
    /// moves as text. That can leave a relation the source declared one-sided;
    /// the evolve sweep raises it as `V103` and the fix is one append.
    ///
    /// **What the new engram inherits, and what it does not.** The moved
    /// content, the source's tags and the source's `type` carry over, because
    /// splitting a guide into two guides is what a reader expects. The
    /// lifecycle does not: the new engram is `stable` with no validity window,
    /// since the facts being moved out are the ones that still hold. Nothing
    /// else from the source's frontmatter follows it.
    ///
    /// `scope` is the acting scope every write verb carries; see
    /// [`Engine::write_engram_as`].
    pub async fn split_engram_as(
        &self,
        p: &SplitParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write(self, &p.domain, scope).await?;
        let overlay = view.actor();
        let actor = self.actor_for(client, overlay);
        let (desc, source) = view.resolve(&p.identifier).await?;
        // The text the split moves observations out of is what this actor sees
        // there: the open document when somebody has this page up, their own
        // draft when they hold one, the reviewed file otherwise. Splitting the
        // base under a draft would move lines the splitter is not looking at,
        // and splitting the file under an open room would plan against a
        // version the room has already moved past - the staged edit below
        // composes into that room and compares this very checksum against it,
        // so a plan made from the stored text could never land while anybody
        // was typing. The probe stands here, above every lock this verb reaches.
        // See `Engine::live_text_at`.
        let stored = || async {
            match overlay {
                Some(_) => view.text_at(&source, &desc).await?.ok_or_else(|| {
                    EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        p.identifier, p.domain
                    ))
                }),
                None => self.load_content(&source, &desc).await,
            }
        };
        let content = match self.live_text_at(&desc, &view).await {
            Some(live) => live,
            None => stored().await?,
        };
        let checksum = sha256_hex(content.as_bytes());
        if let Some(expected) = p.expected_checksum.as_deref()
            && expected != checksum
        {
            return Err(EngineError::Conflict(stale_edit_message(
                expected, &checksum,
            )));
        }
        let engram = parse_engram(&content).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let plan = Self::plan_split(&content, &engram, p, &desc.permalink)?;

        // One trimmed title everywhere: the heading, the link the source gets
        // and the receipt all name the engram the same way.
        let title = p.title.trim().to_string();

        // The new engram, written through the ordinary capture path so the
        // permalink screens, the collision refusal and the provenance stamp are
        // the ones every other new engram gets.
        let body = format!(
            "# {title}\n\n{}\n\n- derived_from [[{}]]",
            plan.moved, desc.permalink
        );
        let created = self
            .write_engram_as(
                &WriteParams {
                    domain: p.domain.clone(),
                    title: title.clone(),
                    content: body,
                    folder: p.folder.clone(),
                    engram_type: Some(engram.frontmatter.engram_type.clone()),
                    tags: engram.frontmatter.tags.clone(),
                    status: Some("stable".to_string()),
                    metadata: None,
                    overwrite: false,
                    // A split writes the splitter's own new engram, which is
                    // nobody's shared draft: a link presented on the split
                    // would be a link to the page being split, not to this.
                    share_link: None,
                    model: None,
                },
                client,
                // The splitter's own scope: the new engram is written by
                // whoever asked for the split, wherever their writes land.
                scope,
            )
            .await?;
        // Where the capture path put it, which is also what the rollback below
        // has to address. Absent means the receipt shape changed under this
        // code: the rollback is skipped and said out loud rather than run
        // against an empty identifier, which would delete nothing and report
        // nothing.
        let new_permalink = created["permalink"].as_str().map(str::to_string);
        let new_path = created["path"].as_str().unwrap_or_default().to_string();

        // By permalink, for the reason `derived_from` above is: a title is
        // prose and may carry a colon that `[[...]]` reads as a cross-domain
        // prefix (issue #65). The fallback for the one case with no permalink
        // to name - a receipt whose shape changed under this code, which the
        // rollback below reports rather than acts on - is the slug of the
        // title, which is what the capture path would have derived anyway, and
        // never the title itself: that would write the very shape this change
        // is about.
        let back_link = new_permalink
            .clone()
            .unwrap_or_else(|| crystalline_core::slugify(&title));
        let remaining = append_body(&plan.remaining, &format!("- split_into [[{back_link}]]"));
        let edited = self
            .apply_source_edit_staged(
                &desc,
                &source,
                &view,
                Some(&checksum),
                &actor,
                // A split moves words it did not write, so its tail records
                // the agent without a model, exactly as it records the actor.
                None,
                None,
                move |_| Ok(remaining),
            )
            .await;
        let source_warning = match edited {
            Ok(warning) => warning,
            Err(failure) => {
                if failure.wrote {
                    // The source's bytes are the edited ones, so the moved
                    // observations live in the new engram and nowhere else.
                    // Deleting it here is the one thing that would lose them.
                    return Err(EngineError::Internal(format!(
                        "the split wrote both engrams but the index update for '{}' failed: {}. Both are kept and neither was undone ({} and {}); the index row for '{}' may be stale until the next sync or reindex picks it up",
                        desc.permalink, failure.error, desc.path, new_path, desc.permalink
                    )));
                }
                // The source is untouched, so the new engram is knowledge the
                // archive now holds twice. Take it back, and report the underlying
                // failure rather than the cleanup: what the caller has to act on is
                // that the source moved under them.
                match new_permalink {
                    Some(permalink) => {
                        let _ = self
                            .delete_engram_as(
                                &DeleteParams {
                                    identifier: permalink,
                                    domain: p.domain.clone(),
                                    expected_checksum: None,
                                },
                                client,
                                // The splitter's own scope again: in review mode
                                // the engram to take back is in the splitter's
                                // draft, and the owner wrapper would look for it
                                // in the owner's.
                                scope,
                            )
                            .await;
                    }
                    None => tracing::warn!(
                        receipt = %created,
                        "split rollback skipped: the capture receipt named no permalink"
                    ),
                }
                return Err(failure.error);
            }
        };

        let mut receipt = json!({
            "domain": desc.domain,
            "source": {
                "permalink": desc.permalink,
                "path": desc.path,
                "title": desc.title,
            },
            "new": {
                "permalink": created["permalink"],
                "path": created["path"],
                "title": title,
            },
            "moved_observations": plan.observations,
            "moved_sections": plan.sections,
        });
        // A split is two writes, and in review mode both of them are drafts:
        // the engram it created and the source it edited. Its receipt says so
        // like every other routed verb's, or a caller reads a split of the
        // folder the team reviewed.
        if overlay.is_some() {
            receipt["draft"] = json!(true);
        }
        // Either write's mirror can fail on its own, and the create's warning
        // is already on the receipt this verb built its own from.
        note_unmirrored(
            &mut receipt,
            source_warning.warning.or_else(|| {
                created
                    .get("draft_warning")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        );
        Ok(receipt)
    }

    /// Work out what a split would move and what it would leave, or refuse.
    /// Pure text over the parsed source, so every refusal happens before the
    /// first write.
    pub(super) fn plan_split(
        content: &str,
        engram: &Engram,
        p: &SplitParams,
        permalink: &str,
    ) -> Result<SplitPlan> {
        let mut moving: BTreeSet<usize> = BTreeSet::new();
        // Both counts answer the same question - how many distinct things
        // moved - so both are collected as sets: a line named twice moves once,
        // and two paths that resolve to the same heading move one section.
        let mut observations: BTreeSet<usize> = BTreeSet::new();
        let mut sections: BTreeSet<(usize, usize)> = BTreeSet::new();
        for line in &p.observations {
            if !engram.observations.iter().any(|o| o.line == *line) {
                return Err(EngineError::Invalid(format!(
                    "line {line} is not an observation bullet on '{permalink}'; \
                     read_engram reports the line of every observation it carries"
                )));
            }
            moving.insert(*line);
            observations.insert(*line);
        }
        for path in &p.sections {
            let (start, end) =
                crystalline_core::emit::section_line_range(content, path).map_err(section_err)?;
            moving.extend(start..end);
            sections.insert((start, end));
        }
        if moving.is_empty() {
            return Err(EngineError::Invalid(
                "split_engram needs something to move: pass observations (the line numbers \
                 read_engram reports) or sections (heading paths such as '## Notes')"
                    .into(),
            ));
        }

        let mut moved: Vec<&str> = Vec::new();
        let mut kept: Vec<&str> = Vec::new();
        for (i, line) in content.split('\n').enumerate() {
            if moving.contains(&(i + 1)) {
                moved.push(line);
            } else {
                kept.push(line);
            }
        }
        let remaining = kept.join("\n");
        // Measured on the knowledge that would be left, before the
        // `split_into` line is appended: a bookkeeping relation is not what
        // makes an engram worth keeping.
        let left = parse_engram(&remaining)
            .map(|e| crystalline_index::content_line_count(&e.body))
            .unwrap_or(0);
        if left < crystalline_index::MIN_CONTENT_LINES {
            return Err(EngineError::Invalid(format!(
                "that selection would leave '{permalink}' with {left} content line(s), under the \
                 {} verify rule Q001 requires; move less, or retire the whole engram instead of \
                 splitting it",
                crystalline_index::MIN_CONTENT_LINES
            )));
        }

        Ok(SplitPlan {
            moved: moved.join("\n").trim_matches('\n').to_string(),
            remaining,
            observations: observations.len(),
            sections: sections.len(),
        })
    }
}
