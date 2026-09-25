use super::*;

impl Engine {
    // --- edit ----------------------------------------------------------------

    /// Apply a surgical edit to an engram, then reindex it. A file domain edits
    /// the file on disk and reindexes it; a virtual domain reads the current
    /// content from the database, applies the same edit and writes it back under
    /// a compare-and-swap guard so a stale edit is refused rather than silently
    /// clobbering a concurrent change (see `expected_checksum`).
    pub async fn edit_engram(&self, p: &EditParams) -> Result<Value> {
        self.edit_engram_as(p, None, &crate::scope::Scope::Unrestricted)
            .await
    }

    /// [`Engine::edit_engram`] with the editor's identity, resolved by
    /// [`Engine::actor`] and written into the engram's `generated` block. An
    /// engram that still carries the legacy `timestamp` key migrates to
    /// `generated` here, on its next edit.
    ///
    /// `scope` is the acting scope every write verb carries; see
    /// [`Engine::write_engram_as`].
    pub async fn edit_engram_as(
        &self,
        p: &EditParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.edit_engram_joined(p, client, scope, None).await
    }

    /// [`Engine::edit_engram_as`], with the join a session may be holding.
    ///
    /// **The compose verb, and so the one a join is actually for.** A person
    /// invited into somebody's draft is invited to work on that page, and an
    /// agent invited into it edits the page - it does not re-address it, move
    /// it or take it away, which is why those verbs take no join. The two
    /// gates `Engine::save_engram_joined` carries ride here for the same
    /// reason they ride there rather than in the routes, so a second surface
    /// that learns to join inherits both: an edit is into ONE draft, and an
    /// edit at a path this caller was GRANTED but has not joined is refused in
    /// words that name both ways forward.
    ///
    /// `None` is the ordinary edit, which is what every surface but a joined
    /// one passes and what this verb did before joins existed.
    pub async fn edit_engram_joined(
        &self,
        p: &EditParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
    ) -> Result<Value> {
        self.edit_engram_present(p, client, scope, join, None).await
    }

    /// [`Engine::edit_engram_joined`], with the agent as a named peer in the
    /// room the edit may land in.
    ///
    /// The bottom rung, and the only one that knows about the strip. `peer` is
    /// display alone: it changes nothing about what is written or where, and
    /// the provenance the edit records is `client` exactly as it always was.
    /// It is carried this far down rather than resolved from the receipt
    /// because the room is keyed on the overlay owner, and who that is - your
    /// own draft, the author's draft you were invited into, or the document a
    /// direct domain keeps - is the view's answer, resolved here.
    ///
    /// `None` is every surface that is not an agent working for somebody: the
    /// CLI, the control socket, a call nobody authenticated.
    pub async fn edit_engram_present(
        &self,
        p: &EditParams,
        client: Option<&str>,
        scope: &crate::scope::Scope,
        join: Option<&crate::join::Join>,
        peer: Option<&AgentPeer>,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let view = DomainView::for_write_joined(self, &p.domain, scope, join).await?;
        let overlay = view.actor();
        // The join as this view actually took it: one naming another domain,
        // or a domain that has stopped reviewing changes, is not a join into
        // this edit at all and must not gate it.
        let join = join.filter(|_| view.joined().is_some());
        let actor = self.actor_for(client, overlay);
        let (desc, source) = match view.resolve(&p.identifier).await {
            Ok(resolved) => resolved,
            // A name this caller's own view cannot resolve, when they hold a
            // link to a draft that answers to it: the miss IS the rule, since
            // the granted draft is deliberately absent from every ordinary
            // read they make. So the refusal teaches instead, in the same
            // words a save at that name gets.
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
        self.screen_granted_path(&desc.domain, &desc.path, scope, join)
            .await?;
        // An `evolve_ack` assignment is the one set_frontmatter key whose value
        // the server completes rather than takes: the scope comes from running
        // detection over this engram's domain, which needs the store and so
        // cannot happen inside the pure text edit below. Computed before the
        // write lock is taken, so a sweep never runs while a file is held.
        let ack = self.ack_draft(p, &desc, &actor, scope).await?;

        // The staged form rather than the shorthand, for the one thing it
        // reports that this verb says out loud: whether the edit composed into
        // a live co-editing document instead of a file or a row.
        // The model the agent reported, held against the actor this edit
        // records: a person's edit never carries one (`stamped_model`).
        let model = stamped_model(&actor, p.model.as_deref());
        // What the text edit itself has to report, collected from inside the
        // one arm that ran it - the file, the virtual row, a draft or a live
        // room - so every landing path reports the same two things: the
        // heading a section edit dropped, and for a MANIFEST the text that
        // landed, which the MANIFEST rules then read.
        let is_manifest = is_manifest_path(&desc.path);
        let mut heading_stripped: Option<String> = None;
        let mut manifest_text: Option<String> = None;
        let edited = self
            .apply_source_edit_staged(
                &desc,
                &source,
                &view,
                p.expected_checksum.as_deref(),
                &actor,
                model.as_deref(),
                peer,
                |current| {
                    // The REPORTED model here, not the one resolved against
                    // this actor: the block this call stamps is held against
                    // the actor it records (above), and a `verified` entry is
                    // held against the actor IT records, which only the arm
                    // that builds it knows.
                    let (text, stripped) = self.apply_edit(
                        current,
                        p,
                        &desc.permalink,
                        &actor,
                        p.model.as_deref(),
                        ack.as_ref(),
                    )?;
                    heading_stripped = stripped;
                    if is_manifest {
                        manifest_text = Some(text.clone());
                    }
                    Ok(text)
                },
            )
            .await
            .map_err(|failure| failure.error)?;

        let mut response = json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "operation": p.operation,
        });
        if overlay.is_some() {
            response["draft"] = json!(true);
        }
        // Whose draft it landed in, when that is not the caller's own - the
        // one thing a joined edit has to say that an ordinary one does not.
        // Somebody composing inside a colleague's draft is owed a plain
        // sentence about where the words went, in the same words the joined
        // save says it in.
        if let Some(owner) = view.joined() {
            response["joined"] = json!(format!("landed in {owner}'s draft"));
        }
        // Where it went, said only when that is not where an edit ordinarily
        // goes: somebody has this page open, the text is in their document,
        // and their session is what writes it down. `present` names them, so
        // an agent can say whose screen it just appeared on.
        if let Some(live) = &edited.live {
            response["landed"] = json!("live");
            response["present"] = json!(live.participants);
        }
        note_unmirrored(&mut response, edited.warning);
        match &ack {
            Some(AckDraft::Record(entry)) => response["evolve_ack"] = ack_json(entry),
            Some(AckDraft::Remove(rule)) => response["evolve_ack_removed"] = json!(rule),
            None => {}
        }
        note_heading_stripped(&mut response, heading_stripped);
        if let Some(text) = manifest_text {
            note_manifest_findings(&mut response, &text, domain_verify_config(&source).as_ref());
        }
        Ok(response)
    }

    /// Read an engram's source, hand it to `apply`, and write the result back:
    /// the shared body of every edit that rewrites content in place.
    ///
    /// Kind-agnostic and lock-correct, which is why it is one function rather
    /// than repeated per caller. For a file domain the write lock is held
    /// across the read, the compare, the edit and the write; without an
    /// `expected_checksum` that serialization is the whole guarantee, since two
    /// unguarded edits must each apply to what the other wrote rather than
    /// silently dropping it. For a virtual domain the store's own compare and
    /// swap plays that part, with the checksum of what was just read standing
    /// in when the caller presents none.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn apply_source_edit<F>(
        &self,
        desc: &EngramDescriptor,
        source: &ContentSource,
        view: &DomainView<'_>,
        expected_checksum: Option<&str>,
        actor: &str,
        model: Option<&str>,
        apply: F,
    ) -> Result<Option<String>>
    where
        F: FnOnce(&str) -> Result<String>,
    {
        // The live landing is dropped here rather than plumbed on: the callers
        // that reach this shorthand (a retirement, a split's tail, a
        // successor's back-link) build receipts about what they moved rather
        // than about where the bytes went, and every one of them composes into
        // an open room correctly without saying so. The verb that says so is
        // `edit_engram_as`, which calls the staged form for exactly that.
        //
        // The agent peer goes the same way and for the same reason: these
        // verbs carry no peer to name, because none of their surfaces resolves
        // one. The text still composes into the open room; what an author does
        // not get is a chip for the agent that retired the page under them.
        self.apply_source_edit_staged(
            desc,
            source,
            view,
            expected_checksum,
            actor,
            model,
            None,
            apply,
        )
        .await
        .map(|edited| edited.warning)
        .map_err(|failure| failure.error)
    }

    /// [`Engine::apply_source_edit`], reporting whether the source's bytes were
    /// already replaced when it failed.
    ///
    /// One caller needs that, and only one: [`Engine::split_engram_as`] writes a
    /// second engram before this runs and may only take that engram back while
    /// the source is provably untouched. No error kind answers the question -
    /// the reindex that follows the rename reads the file back and raises the
    /// same `Io` a refused write raises - so the stage is reported by the code
    /// that knows it rather than guessed from the error afterwards.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn apply_source_edit_staged<F>(
        &self,
        desc: &EngramDescriptor,
        source: &ContentSource,
        view: &DomainView<'_>,
        expected_checksum: Option<&str>,
        actor: &str,
        model: Option<&str>,
        peer: Option<&AgentPeer>,
        apply: F,
    ) -> std::result::Result<SourceEdited, SourceEditFailure>
    where
        F: FnOnce(&str) -> Result<String>,
    {
        // **The live arm, and it comes before every other one.** While a
        // co-editing room is open over this document, the room's text IS the
        // engram: somebody has it on screen, the file and the row are both
        // behind it, and the room's own saver is what makes anything durable.
        // So an edit composes into the document, the caller's
        // `expected_checksum` is evaluated against the text the document
        // holds, and the write below is not reached at all.
        //
        // Ahead of the overlay arm as well as the file and virtual ones, which
        // is what the arm order has to be rather than what it was written as:
        // the overlay arm returns, so an arm behind it is unreachable in every
        // domain that reviews changes - which is where most rooms are.
        //
        // Nothing here is reachable from a room's own save. A room saves
        // through `Engine::save_engram_in_overlay` and `Engine::save_engram`,
        // neither of which is this function, so a live landing can never
        // recurse into the room that produced it.
        if let Some(rooms) = self.collab_rooms() {
            let overlay = view.actor();
            if let Some(live) = rooms
                .live_text(&desc.domain, &desc.permalink, overlay)
                .await
            {
                // The document's text is what a caller guarding this edit read
                // a moment ago, so it is what the guard compares against - the
                // file's checksum would refuse every guarded edit made while
                // anybody had the page open.
                if let Some(expected) = expected_checksum {
                    let found = sha256_hex(live.as_bytes());
                    if found != expected {
                        return Err(SourceEditFailure::before(EngineError::Conflict(
                            stale_edit_message(expected, &found),
                        )));
                    }
                }
                let edited = apply(&live).map_err(SourceEditFailure::before)?;
                // The same two passes every other arm makes, in the same
                // order. An edit that skipped them would put a document in
                // front of a person that the saver then refuses, minutes
                // later, for a reason nobody watching could connect to this.
                let edited = touch_generated(&edited, actor, model, now_offset());
                let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
                let applied = rooms
                    .apply_text(&desc.domain, &desc.permalink, overlay, edited, actor, peer)
                    .await
                    .map_err(|detail| SourceEditFailure::before(EngineError::Conflict(detail)))?;
                // No mirror warning: nothing was mirrored, because nothing was
                // written. The room's saver owes that warning when it lands.
                return Ok(SourceEdited {
                    warning: None,
                    live: Some(applied),
                });
            }
        }

        // The third arm, and it comes first because it is the one that must
        // reach neither of the others: on a domain in review mode the folder
        // and the database both go on saying what the team reviewed, and the
        // edit joins this actor's own draft instead.
        //
        // Every failure on this arm is a `before`: the row and its chunks go
        // down in one transaction that rolls back whole, so an edit that
        // refuses leaves the draft holding exactly the bytes it held.
        if let Some(who) = view.actor() {
            // Keyed on the draft's own mirror path, which is the file this
            // write actually produces, so two edits of one draft serialize on
            // it exactly as two edits of one engram serialize on its file - and
            // so does a capture, a save, a delete and a move of the same draft.
            // See `Engine::draft_lock`.
            let lock = self
                .draft_lock(&desc.domain, who, &desc.path)
                .map_err(SourceEditFailure::before)?;
            let _guard = lock.lock().await;
            let current = view
                .text_at(source, desc)
                .await
                .map_err(SourceEditFailure::before)?
                .ok_or_else(|| {
                    SourceEditFailure::before(EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        desc.permalink, desc.domain
                    )))
                })?;
            if let Some(expected) = expected_checksum {
                let found = sha256_hex(current.as_bytes());
                if found != expected {
                    return Err(SourceEditFailure::before(EngineError::Conflict(
                        stale_edit_message(expected, &found),
                    )));
                }
            }
            // The test seam, and it sits here because here is the window: the
            // draft has been read and has not been written back yet. See
            // `Engine::hold_next_draft_edit`.
            self.take_draft_hold().await;
            let edited = apply(&current).map_err(SourceEditFailure::before)?;
            let edited = touch_generated(&edited, actor, model, now_offset());
            let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
            if self.take_armed_failure() {
                return Err(SourceEditFailure::before(EngineError::Internal(
                    "reindex failed (test seam)".to_string(),
                )));
            }
            let warning = view
                .write(desc.domain_id, &desc.path, &edited)
                .await
                .map_err(SourceEditFailure::before)?;
            // Neither tail below runs. A draft of the MANIFEST is one actor's
            // proposal about the domain's routing, not the domain's routing,
            // and a draft belongs in no folder's generated index - both of
            // those are properties of what the team reviewed.
            return Ok(SourceEdited {
                warning,
                live: None,
            });
        }

        match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                let current = std::fs::read_to_string(&abs).map_err(|source| {
                    SourceEditFailure::before(EngineError::Io {
                        path: abs.display().to_string(),
                        source,
                    })
                })?;
                // The CAS token, when the caller presents one: compared inside
                // the lock, against the bytes just read, exactly as save_engram
                // compares.
                if let Some(expected) = expected_checksum {
                    let found = sha256_hex(current.as_bytes());
                    if found != expected {
                        return Err(SourceEditFailure::before(EngineError::Conflict(
                            stale_edit_message(expected, &found),
                        )));
                    }
                }
                let edited = apply(&current).map_err(SourceEditFailure::before)?;
                let edited = touch_generated(&edited, actor, model, now_offset());
                let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
                // The last step that can fail with the file as it was:
                // `write_bytes` renames a sibling temp into place, and a rename
                // either happens or does not, so a refusal here leaves the
                // source's bytes untouched.
                write_file(&abs, &edited).map_err(SourceEditFailure::before)?;
                if self.take_armed_failure() {
                    return Err(SourceEditFailure::after(EngineError::Internal(
                        "reindex failed (test seam)".to_string(),
                    )));
                }
                let store = self.store.lock().await;
                self.reindex_file(&*store, desc.domain_id, root, &desc.path)
                    .await
                    .map_err(SourceEditFailure::after)?;
            }
            ContentSource::Virtual => {
                let current = {
                    let store = self.store.lock().await;
                    store
                        .engram_content(desc.domain_id, &desc.path)
                        .await
                        .map_err(|e| SourceEditFailure::before(EngineError::from(e)))?
                        .ok_or_else(|| {
                            SourceEditFailure::before(EngineError::NotFound(format!(
                                "no content stored for '{}' in domain '{}'",
                                desc.permalink, desc.domain
                            )))
                        })?
                };
                let expected = expected_checksum
                    .map(str::to_string)
                    .unwrap_or_else(|| sha256_hex(current.as_bytes()));
                let edited = apply(&current).map_err(SourceEditFailure::before)?;
                let edited = touch_generated(&edited, actor, model, now_offset());
                let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
                let stamp = virtual_stamp(&edited);
                // The seam, on this arm: a token nothing can match, so the
                // store raises its own compare-and-swap conflict and rolls the
                // transaction back. See `Engine::fail_next_source_edit`.
                let expected = if self.take_armed_failure() {
                    "0".repeat(64)
                } else {
                    expected
                };
                let store = self.store.lock().await;
                // Every failure here is a `before`, and that is exact rather
                // than generous: `index_markdown` runs the compare and swap,
                // the chunking and the reference resolution inside one store
                // transaction and rolls it back on any error, so a virtual
                // source that refuses still holds the bytes it held. A
                // concurrent edit therefore comes back as the `Conflict` it is
                // and the caller may undo whatever it wrote first, which is the
                // failure that actually happens in the field.
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    &edited,
                    stamp,
                    Some(&expected),
                    true,
                )
                .await
                .map_err(SourceEditFailure::before)?;
            }
        }

        // A virtual edit may have rewritten this domain's MANIFEST engram, so
        // refresh the routing cache. The store locks above are all released.
        if matches!(source, ContentSource::Virtual) {
            self.refresh_routing_cache().await;
        }
        // An edit can change the title or the description the folder's
        // generated index lists this engram under.
        self.refresh_index_files(&desc.domain).await;
        // One call for both arms, and exactly right there: this tail is
        // reached only when the arm that ran committed its bytes, so a refused
        // edit never schedules a pass for a chunk that was not rewritten.
        self.nudge_embed();
        // A direct write has no mirror to fail, so it has nothing to warn about.
        Ok(SourceEdited {
            warning: None,
            live: None,
        })
    }

    /// Apply one edit operation to an engram's markdown, returning the edited
    /// text. Content-agnostic: the same logic serves file and virtual edits.
    /// `actor` is the resolved editor identity, which `set_frontmatter` stamps
    /// into a verification when the caller names no other one.
    ///
    /// `model` is the model the caller REPORTED, not one already held against
    /// an actor: the only operation that records it here is a verification, and
    /// a verification is held against the actor it names rather than the one
    /// making the call ([`stamped_model`]).
    ///
    /// Beside the text, the heading line a section edit dropped from the top
    /// of its content because it repeated the target's own heading, which
    /// only `replace_section` and `insert_after_section` can report (see
    /// [`crystalline_core::emit::SectionEdit`]); `None` for every other
    /// operation and for content placed as sent.
    #[allow(clippy::too_many_arguments)]
    fn apply_edit(
        &self,
        source: &str,
        p: &EditParams,
        permalink: &str,
        actor: &str,
        model: Option<&str>,
        ack: Option<&AckDraft>,
    ) -> Result<(String, Option<String>)> {
        let text = match p.operation.as_str() {
            "append" => append_body(source, self.require_content(p)?),
            "prepend" => prepend_body(source, self.require_content(p)?),
            "find_replace" => {
                let content = self.require_content(p)?;
                let find = p.find_text.as_deref().ok_or_else(|| {
                    EngineError::Invalid("find_replace requires find_text".into())
                })?;
                if find.is_empty() {
                    return Err(EngineError::Invalid("find_text must not be empty".into()));
                }
                let count = source.matches(find).count();
                if count == 0 {
                    return Err(EngineError::NotFound(format!(
                        "find_text '{find}' not found in '{permalink}'"
                    )));
                }
                if let Some(expected) = p.expected_replacements
                    && expected != count
                {
                    return Err(EngineError::Invalid(format!(
                        "expected {expected} replacements of '{find}' but found {count}"
                    )));
                }
                source.replace(find, content)
            }
            // The two operations whose content lands under a heading that
            // stays: a repeat of that heading is dropped by the core edit, and
            // reported here so the receipt can say so.
            "replace_section" => {
                let content = self.require_content(p)?;
                let section = self.require_section(p)?;
                let edit =
                    replace_section_reporting(source, section, content, p.include_subsections)
                        .map_err(section_err)?;
                return Ok((edit.text, edit.heading_stripped));
            }
            "insert_before_section" => {
                let content = self.require_content(p)?;
                let section = self.require_section(p)?;
                insert_before_section(source, section, content).map_err(section_err)?
            }
            "insert_after_section" => {
                let content = self.require_content(p)?;
                let section = self.require_section(p)?;
                let edit = insert_after_section_reporting(source, section, content)
                    .map_err(section_err)?;
                return Ok((edit.text, edit.heading_stripped));
            }
            "set_frontmatter" => {
                Self::apply_set_frontmatter(source, p, permalink, actor, model, ack)?
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "unknown edit operation '{other}'; expected append, prepend, find_replace, replace_section, insert_before_section, insert_after_section or set_frontmatter"
                )));
            }
        };
        Ok((text, None))
    }

    /// Assign or clear one lifecycle frontmatter field, the `set_frontmatter`
    /// operation. Restricted to [`SETTABLE_FRONTMATTER_KEYS`]: identity,
    /// provenance and index keys are owned by the tools that maintain them, so
    /// rewriting one here is refused rather than silently corrupting the
    /// engram's address or its write history.
    ///
    /// An absent or empty value clears the field, except on `status`, which is
    /// required, and on `verified`, which stamps a verification instead. That
    /// verification carries `model` - the reported one - only where the actor
    /// it names is not a person; see the arm.
    #[allow(clippy::too_many_arguments)]
    fn apply_set_frontmatter(
        source: &str,
        p: &EditParams,
        permalink: &str,
        actor: &str,
        model: Option<&str>,
        ack: Option<&AckDraft>,
    ) -> Result<String> {
        let key = p
            .key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "set_frontmatter requires key, one of {}",
                    settable_keys()
                ))
            })?;
        let value = p.value.as_deref().map(str::trim).filter(|v| !v.is_empty());

        match key {
            "status" => {
                let status = value.ok_or_else(|| {
                    EngineError::Invalid(
                        "status cannot be removed: every engram needs one (verify rule T001). Set a retirement status such as deprecated or superseded instead".into(),
                    )
                })?;
                Ok(set_frontmatter_field(source, "status", status))
            }
            "valid_from" | "valid_to" | "stale_after" | "source_date" => {
                let Some(raw) = value else {
                    // Clearing a bound is how absence - always valid, valid
                    // forever, no review due - is restored. The legacy
                    // `review_after` spelling goes with `stale_after` so the
                    // bound is really gone whichever spelling the file used.
                    let out = remove_frontmatter_field(source, key);
                    return Ok(if key == "stale_after" {
                        remove_frontmatter_field(&out, "review_after")
                    } else {
                        out
                    });
                };
                // Validate through the write contract itself rather than a
                // second parser, so a timestamp, an int or a sentinel bound is
                // answered here exactly as write_engram answers it.
                let mut probe = Frontmatter::default();
                probe
                    .extra
                    .insert(key.to_string(), YamlValue::String(raw.to_string()));
                let dropped = crystalline_core::temporal::normalize_temporal_fields(&mut probe)
                    .map_err(|e| EngineError::Invalid(e.to_string()))?;
                if !dropped.is_empty() {
                    // A sentinel bound: absence is how open-ended validity is
                    // expressed, so the field is cleared rather than written.
                    return Ok(remove_frontmatter_field(source, key));
                }
                let date = match key {
                    "valid_from" => probe.valid_from,
                    "valid_to" => probe.valid_to,
                    "source_date" => probe.source_date,
                    _ => probe.stale_after,
                }
                .expect("a normalized date field is promoted into its typed slot");
                Ok(if key == "stale_after" {
                    // Migrates a legacy `review_after` line in place.
                    set_stale_after(source, date)
                } else {
                    set_frontmatter_field(source, key, &date.format("%Y-%m-%d").to_string())
                })
            }
            "salience" => {
                let Some(raw) = value else {
                    return Ok(remove_frontmatter_field(source, "salience"));
                };
                let n: f64 = raw.parse().map_err(|_| {
                    EngineError::Invalid(format!(
                        "salience must be a number from 0 to 10, got '{raw}'"
                    ))
                })?;
                if !n.is_finite() || !(0.0..=10.0).contains(&n) {
                    return Err(EngineError::Invalid(format!(
                        "salience must be a number from 0 to 10, got {raw}"
                    )));
                }
                Ok(set_frontmatter_number(source, "salience", n))
            }
            "verified" => {
                // A verification is a record of a check that happened, so it is
                // never cleared here: an omitted value names the caller as the
                // verifier instead, which is the common "I re-checked this and
                // it still holds" case.
                let by = value
                    .map(sanitize_actor)
                    .filter(|a| !a.is_empty())
                    .unwrap_or_else(|| actor.to_string());
                // The model is held against the entry's OWN actor rather than
                // the one this call is acting as, because that is the actor the
                // record ends up claiming wrote the check. A caller may name
                // somebody else as the verifier, and where that somebody is a
                // person the entry carries no model, exactly as a person's
                // `generated` block carries none.
                let model = stamped_model(&by, model);
                let entry = crystalline_core::Verified {
                    by,
                    model,
                    at: Some(now_offset()),
                };
                // Keep other actors' verifications and replace this actor's, so
                // the trust record stays a history without growing a line on
                // every sweep.
                let mut entries = parse_engram(source)
                    .map(|e| e.frontmatter.verified)
                    .unwrap_or_default();
                entries.retain(|e| e.by != entry.by);
                entries.push(entry);
                Ok(set_verified(source, &entries))
            }
            EVOLVE_ACK_KEY => {
                // The draft was completed before the lock (see
                // `Engine::ack_draft`), because a record's scope is the sweep's
                // verdict about this engram and no text edit can know it.
                let draft = ack.ok_or_else(|| EngineError::Invalid(ack_value_message()))?;
                let entry = match draft {
                    AckDraft::Record(entry) => entry,
                    // A removal reads the entries the file holds right now,
                    // under the lock, so a concurrent acknowledgment is either
                    // fully there or not there at all when it filters. It
                    // names a rule and never a pair: `remove <rule-id>` is the
                    // whole of the value form an agent writes here, so every
                    // entry the rule has goes.
                    AckDraft::Remove(rule) => {
                        if !has_ack(source, rule, None) {
                            return Err(EngineError::Invalid(format!(
                                "no acknowledgment for {rule} on '{permalink}'; nothing to remove"
                            )));
                        }
                        return guarded_ack_write(
                            without_ack(source, rule, None),
                            "removal",
                            &p.identifier,
                        );
                    }
                };
                // An engram whose frontmatter no longer parses would have its
                // existing entries read as none, and this would write a second
                // `evolve_ack` key beside the one already there - compounding a
                // break instead of reporting it.
                parse_engram(source).map_err(|e| {
                    EngineError::Invalid(format!(
                        "cannot acknowledge a finding on an engram that does not parse ({e}); repair the frontmatter first"
                    ))
                })?;
                // Defense in depth: whatever the note carried, the bytes about
                // to be persisted have to be readable.
                guarded_ack_write(
                    set_evolve_ack(source, &merged_acks(source, entry.clone())),
                    "acknowledgment",
                    &p.identifier,
                )
            }
            other => Err(EngineError::Invalid(format!(
                "set_frontmatter cannot set '{other}'; the settable keys are {}",
                settable_keys()
            ))),
        }
    }

    fn require_content<'a>(&self, p: &'a EditParams) -> Result<&'a str> {
        p.content.as_deref().ok_or_else(|| {
            EngineError::Invalid(format!("operation '{}' requires content", p.operation))
        })
    }

    fn require_section<'a>(&self, p: &'a EditParams) -> Result<&'a str> {
        p.section
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid(format!("operation '{}' requires a section", p.operation))
            })
    }

    /// Enforce the temporal write contract on post-edit markdown: reject a date
    /// field or a `verified` entry left malformed and surgically drop sentinel
    /// or null bounds,
    /// matching write_engram and import. Post-edit rather than per-argument
    /// because find_replace can rewrite frontmatter text directly. A parse
    /// failure passes through unchanged; indexing reports it.
    fn enforce_temporal(edited: String) -> Result<String> {
        let Ok(engram) = parse_engram(&edited) else {
            return Ok(edited);
        };
        let mut fm = engram.frontmatter;
        let dropped = crystalline_core::temporal::normalize_temporal_fields(&mut fm)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        crystalline_core::temporal::normalize_verified(&mut fm)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let mut out = edited;
        for field in dropped {
            out = remove_frontmatter_field(&out, field);
        }
        Ok(out)
    }
}
