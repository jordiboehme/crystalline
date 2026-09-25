use super::*;

impl Engine {
    // --- delete --------------------------------------------------------------

    /// Delete an engram and its index rows. A file domain also removes the file
    /// on disk; a virtual domain only drops the database rows.
    ///
    /// An `assets/` identifier deletes that attachment instead - the row plus
    /// the file or the blob - which is what completes an orphaned-attachment
    /// finding without a second write verb existing. The two are one verb
    /// because they are one act from the caller's side ("remove this thing from
    /// the domain"), and the identifier says which thing without ambiguity: an
    /// engram can never live under the reserved `assets/` folder.
    pub async fn delete_engram(&self, p: &DeleteParams) -> Result<Value> {
        self.delete_engram_as(p, None, &crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`Engine::delete_engram`] with the deleting identity and the acting
    /// scope, the pair every other write verb takes.
    ///
    /// `scope` decides where the deletion lands. On a direct domain it removes
    /// the file and the row, as it always has. On a domain in review mode it
    /// writes this actor's tombstone: the file stays, the base row stays, and
    /// the path reads as absent for its author and for nobody else, until the
    /// deletion is reviewed like any other change.
    ///
    /// `client` is still read by neither arm. A delete stamps no `generated`
    /// block - there is no document left to stamp - and a tombstone stands
    /// under the base row's own identity rather than under a new one.
    pub async fn delete_engram_as(
        &self,
        p: &DeleteParams,
        _client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write(self, &p.domain, scope).await?;
        let overlay = view.actor();
        if let Some(path) = attachment_identifier(&p.identifier) {
            // Refused rather than ignored: `expected_checksum` is a promise
            // about markdown a caller read, and an attachment's bytes are not
            // that. Accepting it silently would let a caller believe a delete
            // was guarded when nothing compared anything.
            if p.expected_checksum.is_some() {
                return Err(EngineError::Invalid(format!(
                    "expected_checksum guards an engram edit and has no meaning for the attachment '{path}'; delete it without one"
                )));
            }
            // Through the view this verb already built, so an attachment
            // delete lands where an engram delete lands: in review mode as
            // this actor's own deletion, with the folder untouched.
            let draft = self.attachment_delete_in(&view, &path).await?;
            let mut receipt = json!({
                "domain": p.domain,
                "path": path,
                "attachment": true,
                "deleted": true,
            });
            if draft {
                receipt["draft"] = json!(true);
            }
            return Ok(receipt);
        }
        let (desc, source) = view.resolve(&p.identifier).await?;
        // **The open document outranks both substrates, and the probe is above
        // the lock.** A caller's `expected_checksum` came from a read, and a
        // read of a page somebody has open answers the live text and its
        // checksum - so comparing against the file here would refuse the
        // caller who did exactly what the read told them to, for as long as
        // the person kept typing, with a re-read that answers the same value
        // again. The deletion itself still takes the file or the row: a delete
        // does not compose into a document, it ends the engram the document is
        // of, and the room is closed by the removal that follows. See
        // `Engine::live_text_at` for why this cannot move below the lock.
        let live = self.live_text_at(&desc, &view).await;
        // Held across the comparison and the removal, so a guarded delete
        // cannot check a text that a concurrent save then rewrites underneath
        // it - the draft's own lock when this deletion lands in an overlay, the
        // file's when it lands in the folder. See `Engine::draft_lock` and
        // `Engine::write_lock`.
        let write_lock = match overlay {
            Some(who) => Some(self.draft_lock(&p.domain, who, &desc.path)?),
            None => match &source {
                ContentSource::File { root } => Some(self.write_lock(&join_rel(root, &desc.path))),
                ContentSource::Virtual => None,
            },
        };
        let _guard = match &write_lock {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        // The text the guard compares against is the one this caller read: the
        // open document where there is one, and in review mode their own draft
        // where they hold one.
        let visible = match &live {
            Some(live) => Some(live.clone()),
            None => match overlay {
                Some(_) => view.text_at(&source, &desc).await?,
                None => Some(self.load_content(&source, &desc).await?),
            },
        };
        if let Some(expected) = &p.expected_checksum {
            let current = visible.clone().ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no engram '{}' in domain '{}'",
                    p.identifier, p.domain
                ))
            })?;
            let found = sha256_hex(current.as_bytes());
            if &found != expected {
                return Err(EngineError::Conflict(stale_edit_message(expected, &found)));
            }
        }

        // The third place a delete can land, and neither arm below runs for
        // it: the file the team reviewed stays where it is, and this actor's
        // deletion of it stands beside it as a draft.
        if let Some(who) = overlay {
            if visible.is_none() {
                return Err(EngineError::NotFound(format!(
                    "no engram '{}' in domain '{}'",
                    p.identifier, p.domain
                )));
            }
            let mut warning = None;
            // A draft of a path no file holds is this actor's alone, so
            // deleting it takes the draft and its mirror away rather than
            // standing a tombstone over a base row that was never there.
            // By PATH, not by permalink: a tombstone stands over the base row
            // at a path, so "is there one to stand over" is a question about
            // that path. Asking by permalink would take the tombstone branch
            // for a draft whose permalink happens to match a base row
            // somewhere else, and then read a file that path does not have.
            let base = {
                let store = self.store.lock().await;
                store
                    .list_engrams(&desc.domain, Some(&desc.path), None)
                    .await?
                    .into_iter()
                    .find(|found| found.path == desc.path)
            };
            match base {
                None => {
                    view.drop(desc.domain_id, &desc.path).await?;
                }
                Some(_) => {
                    // The tombstone stands under the BASE row's own identity
                    // and its own text, never under the draft it replaces:
                    // that is the shape the journal restore rebuilds after a
                    // wipe, and the two have to be one shape or a restored
                    // deletion says something different from a written one.
                    let base_text = self.load_content(&source, &desc).await?;
                    warning = self
                        .write_overlay_tombstone(&desc.domain, who, &desc, &base_text)
                        .await?;
                }
            }
            // Either way the draft that stood here is over - dropped outright,
            // or replaced by this actor's deletion of the team's page - so
            // every link on it and every session inside it ends with it.
            self.end_draft_grants(&desc.domain, who, &desc.path).await;
            let mut receipt = json!({
                "domain": desc.domain,
                "permalink": desc.permalink,
                "path": desc.path,
                "deleted": true,
                "draft": true,
            });
            note_unmirrored(&mut receipt, warning);
            return Ok(receipt);
        }

        if let ContentSource::File { root } = &source {
            let abs = join_rel(root, &desc.path);
            std::fs::remove_file(&abs).map_err(|source| EngineError::Io {
                path: abs.display().to_string(),
                source,
            })?;
        }
        let store = self.store.lock().await;
        store.delete_engram(desc.domain_id, &desc.path).await?;
        // Deleting a MANIFEST removes its `## Tag Aliases` declarations, so clear
        // the domain's derived alias rows: the content is already gone, so the
        // refresh folds to no pairs and replaces the rows with nothing.
        if desc.path == "MANIFEST.md" {
            crystalline_index::refresh_tag_aliases(&*store, desc.domain_id).await?;
        }
        drop(store);

        // Deleting a virtual domain's MANIFEST engram empties its routing
        // bullets, so refresh the cache once the store lock is released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        // The deleted engram must leave its folder's generated index, and an
        // emptied folder loses the index file altogether.
        self.refresh_index_files(&desc.domain).await;

        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "deleted": true,
        }))
    }

    /// What [`Engine::delete_engram`] would remove, without removing any of it.
    ///
    /// Written for the confirmation round the MCP layer opens on a peer that
    /// can put a question to its user: the question has to name what dies, and
    /// naming it means resolving the identifier first. Every refusal the delete
    /// itself would raise on the way to the file - an unknown domain, an
    /// identifier that resolves to nothing or to two things, a read-only
    /// server, an `expected_checksum` on an attachment - is raised here too, so
    /// a call that cannot succeed fails before a human is asked to approve it
    /// rather than after.
    ///
    /// Two shapes, one per branch of the delete. An `assets/` identifier
    /// previews `{domain, path, size, attachment: true}`; anything else
    /// previews `{domain, permalink, title, path, attachments}`, where
    /// `attachments` is the stored attachments **only this engram references**.
    /// Those files are not deleted with it - `delete_engram` removes the
    /// markdown and its rows and nothing else - so what the list says is which
    /// attachments the delete leaves with no referent at all.
    ///
    /// `attachments` is `null` rather than a list on a domain past
    /// [`MAX_PREVIEW_SCAN_ENGRAMS`], where enumerating them would read the
    /// whole domain to build one sentence. The delete is the same delete
    /// either way; only the question is worded differently, and `null` says
    /// nobody looked where `[]` says somebody looked and found none.
    ///
    /// The `expected_checksum` comparison is deliberately not repeated here:
    /// the file can change between the two rounds, so the guard is worth
    /// nothing unless it runs in the round that actually deletes, which is
    /// where it already runs.
    ///
    /// One narrow divergence the other way, stated so it is not discovered:
    /// the engram branch loads the engram's content unconditionally, to see
    /// what it references, where the delete loads it only when a checksum is
    /// being compared. A row whose content cannot be loaded therefore fails
    /// the preview while the plain delete of it would succeed - reachable on a
    /// virtual domain holding a row with no stored content. It is a miss the
    /// caller can act on rather than a silent one, and the alternative is
    /// answering "attachments: none" for an engram nobody could read.
    pub async fn delete_preview(&self, p: &DeleteParams) -> Result<Value> {
        self.delete_preview_as(p, &crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`Engine::delete_preview`] under the acting scope, the pair
    /// [`Engine::delete_engram_as`] takes.
    ///
    /// The scope is what keeps round one honest about round two on a domain
    /// that reviews changes. An attachment only this caller's own overlay holds
    /// is a file the delete would really remove, so a preview built on the
    /// folder alone would refuse a delete that was going to succeed - and a
    /// path this caller has already deleted is one the delete would miss, so a
    /// preview built on the folder would promise bytes that are not theirs to
    /// take. Both directions are the same rule: **a preview must never be
    /// stricter than the act it previews**, and it must not be laxer either.
    pub async fn delete_preview_as(
        &self,
        p: &DeleteParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let Some(path) = attachment_identifier(&p.identifier) {
            if p.expected_checksum.is_some() {
                return Err(EngineError::Invalid(format!(
                    "expected_checksum guards an engram edit and has no meaning for the attachment '{path}'; delete it without one"
                )));
            }
            let hidden = self.hidden_for(scope).await?;
            let size = DomainView::for_read(self, &p.domain, &hidden, scope)?
                .attachment_delete_size(&path)
                .await?;
            return Ok(json!({
                "domain": p.domain,
                "path": path,
                "size": size,
                "attachment": true,
            }));
        }
        let (desc, source) = self.resolve_in(&p.identifier, &p.domain).await?;
        let content = self.load_content(&source, &desc).await?;
        let attachments = self.previewable_attachments(&desc, &content).await;
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "title": desc.title,
            "path": desc.path,
            "attachments": attachments,
        }))
    }

    /// [`Engine::sole_referent_attachments`] under
    /// [`MAX_PREVIEW_SCAN_ENGRAMS`], and [`None`] above it.
    ///
    /// The two answers are different facts and the shape says which is which:
    /// an array is "these are the attachments the delete orphans", empty
    /// included, and `null` is "nobody looked". The caller that renders the
    /// question reads the difference and words the clause accordingly; nothing
    /// here changes what the delete removes.
    async fn previewable_attachments(
        &self,
        desc: &EngramDescriptor,
        content: &str,
    ) -> Option<Vec<String>> {
        if !self.within_preview_scan_bound(&desc.domain).await {
            return None;
        }
        Some(self.sole_referent_attachments(desc, content).await)
    }

    /// Whether `domain` is small enough for the preview to enumerate, one
    /// metadata query and no bodies.
    ///
    /// **A count that cannot be taken answers `true`.** The bound exists to
    /// keep a question cheap, not to give round one a new way to fail, and a
    /// preview that is stricter than the delete it previews is the one thing
    /// this whole surface must never be. It costs nothing in practice either:
    /// the listing this failed on is the same listing the enumeration opens
    /// with, so it fails there immediately and resolves the safe way, which
    /// names no attachments at all.
    async fn within_preview_scan_bound(&self, domain: &str) -> bool {
        let counted = {
            let store = self.store.lock().await;
            store.list_engrams(domain, None, None).await
        };
        match counted {
            Ok(rows) => count_within_preview_bound(rows.len()),
            Err(e) => {
                tracing::warn!(
                    "the engrams of '{domain}' could not be counted ({e}); the delete preview enumerates attachments as it would on a small domain"
                );
                true
            }
        }
    }

    /// How many bytes [`Engine::attachment_delete`] would remove, or
    /// [`EngineError::NotFound`] when it would remove nothing.
    ///
    /// **Deliberately not [`Engine::attachment_read`], and the difference is a
    /// bug rather than a preference.** That read refuses a file over
    /// [`crystalline_core::MAX_ATTACHMENT_BYTES`] and, on a virtual domain,
    /// insists on both a row and a blob. The delete does neither: it reads no
    /// bytes and succeeds when either half is there. A preview built on the
    /// read would therefore fail round one - and so refuse the delete outright
    /// for a peer that gets asked - on exactly the files this verb is the
    /// escape hatch for: the stray oversized file the walker skipped and so
    /// never gave a row, and the half-present pair a hand-edited domain leaves
    /// behind. **A preview must never be stricter than the act it previews.**
    ///
    /// So the size is looked up rather than measured: the file's own metadata
    /// where there is a file, the recorded row where the file is already gone
    /// and only the row stands. No bytes are read and nothing is written -
    /// unlike the read, which heals the row it serves, so round one no longer
    /// mutates the derived layer at all.
    pub(crate) async fn attachment_delete_size(&self, domain: &str, path: &str) -> Result<u64> {
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let row = {
            let store = self.store.lock().await;
            store.get_attachment(domain_id, path).await?
        };
        // Whichever half the delete would find, in the order that gives the
        // truest number: a row can be stale about a file that is right there,
        // and a file cannot be stale about itself.
        if let ContentSource::File { root } = &source {
            let abs = contained_asset_path(root, path)?;
            match std::fs::metadata(&abs) {
                Ok(meta) => return Ok(meta.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    });
                }
            }
        }
        row.map(|row| row.size)
            .ok_or_else(|| EngineError::NotFound(missing_attachment(domain, path)))
    }

    /// The stored attachments `src` refers to that nothing else in its domain
    /// refers to.
    ///
    /// Counted by the same pair the cross-domain move counts with
    /// ([`referenced_asset_paths`] and [`Engine::shared_asset_paths`]), so
    /// "only this engram uses it" means one thing across the crate, including
    /// the part where a count that fails resolves to shared and therefore
    /// names nothing.
    ///
    /// Screened against the domain's attachment rows at the end, one metadata
    /// query and no bytes: a reference to a file the domain does not hold is a
    /// dangling reference the sweep already reports, and a delete does not
    /// orphan something that was never there.
    pub(super) async fn sole_referent_attachments(
        &self,
        src: &EngramDescriptor,
        content: &str,
    ) -> Vec<String> {
        let candidates = referenced_asset_paths(content);
        if candidates.is_empty() {
            return Vec::new();
        }
        let shared = self.shared_asset_paths(src, &candidates).await;
        let mut sole: Vec<String> = candidates
            .into_iter()
            .filter(|path| !shared.contains(path))
            .collect();
        if sole.is_empty() {
            return sole;
        }
        let stored: HashSet<String> = match self.attachment_list(&src.domain).await {
            Ok(rows) => rows.into_iter().map(|row| row.path).collect(),
            Err(e) => {
                // The screen is a refinement, not the decision. When it cannot
                // run, the unscreened list is still every path this engram is
                // the last referent of, which is the honest answer minus one
                // filter.
                tracing::warn!(
                    "the attachments of '{}' could not be listed ({e}); the delete preview names every path the engram is the last referent of",
                    src.domain
                );
                return sole;
            }
        };
        sole.retain(|path| stored.contains(path));
        sole
    }
}
