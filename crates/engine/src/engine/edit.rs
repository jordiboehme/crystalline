use super::evolve::FOLDER_PAIR_RULE;
use super::*;
use crate::collab::session::{Absorbed, ComposeRefused, RoomDoc};

/// How many times an edit without an `expected_checksum` is applied to a
/// virtual engram before a race with another writer is reported: the first
/// try and two more, each against the text stored at that moment. Three
/// writers landing on one row inside the same few milliseconds three times in
/// a row is not a race an agent can win by retrying faster either, so the
/// last refusal comes back as the `Conflict` a guarded edit would get. A
/// move's rewrite of a virtual linker takes the same budget
/// ([`Engine::relink_engram`]).
pub(super) const UNGUARDED_EDIT_ATTEMPTS: usize = 3;

/// The first pause before a write tries again to compose into a room whose
/// text moved under it; each further try waits twice as long. With
/// [`UNGUARDED_EDIT_ATTEMPTS`] tries that is 120 ms of waiting at most before
/// the write goes to the source instead. An edit's live arm and a move's
/// rewrite of a linker share it.
pub(super) const LIVE_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(40);

/// Whether an error is the store's stale compare-and-swap rather than any
/// other conflict (a permalink another engram owns, say), which applying the
/// edit again could never resolve.
pub(super) fn is_stale_edit(e: &EngineError) -> bool {
    matches!(e, EngineError::Conflict(m) if m.starts_with("stale edit:"))
}

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
        let p = &self.localized_for(p, scope).await?.lf_only();
        let view = DomainView::for_write_joined(self, &p.domain, scope, join).await?;
        let overlay = view.actor();
        // The join as this view actually took it: one naming another domain,
        // or a domain that has stopped reviewing changes, is not a join into
        // this edit at all and must not gate it.
        let join = join.filter(|_| view.joined().is_some());
        let actor = self.actor_for(client, overlay);
        if join.is_none() {
            self.refuse_granted_name(&p.domain, &p.identifier, scope)
                .await?;
        }
        let (desc, source) = match view.resolve(&p.identifier).await {
            Ok(resolved) => resolved,
            // A name this caller's own view cannot resolve, when they hold a
            // link to a draft that answers to it: the miss IS the rule, since
            // the granted draft is deliberately absent from every ordinary
            // read they make. So the refusal teaches instead, in the same
            // words a save at that name gets.
            Err(EngineError::NotFound(missing)) => {
                self.refuse_reflection_identifier(&p.domain, &p.identifier)
                    .await?;
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
        // A hit can be a stale row: a file that became the Reflection's page
        // since the last sync still has its engram row until the next one.
        self.refuse_reflection_identifier(&p.domain, &desc.path)
            .await?;
        self.screen_granted_path(&desc.domain, &desc.path, scope, join)
            .await?;
        // An `evolve_ack` assignment is the one set_frontmatter key whose value
        // the server completes rather than takes: the scope comes from running
        // detection over this engram's domain, which needs the store and so
        // cannot happen inside the pure text edit below. Computed before the
        // write lock is taken, so a sweep never runs while a file is held.
        let ack = self.ack_draft(p, &desc, &actor, scope).await?;
        // `remove V112` takes the pair back wherever its acknowledgment sits:
        // on any engram of the two folders, since the anchor moves as they
        // change. Which pairs, and whether this engram holds an entry itself,
        // is settled here, before the write, like the record's scope above.
        // Not for an edit joined into somebody's draft by its share-link: that
        // edit lands at the one page the link was minted on and nowhere else,
        // so it takes back this engram's entries alone, as it always did.
        let mut folder_pairs = match &ack {
            Some(AckDraft::Remove(rule))
                if join.is_none() && rule.eq_ignore_ascii_case(FOLDER_PAIR_RULE) =>
            {
                let current = view.text_at(&source, &desc).await?.unwrap_or_default();
                let here = has_ack(&current, rule, None);
                // An engram that holds nothing is not written below, so the
                // compare-and-swap the edit would make is made here: a stale
                // token is refused before any other engram changes.
                if !here && let Some(expected) = p.expected_checksum.as_deref() {
                    let found = sha256_hex(current.as_bytes());
                    if found != expected {
                        return Err(EngineError::Conflict(stale_edit_message(expected, &found)));
                    }
                }
                let pairs = self
                    .folder_pairs_to_withdraw(&desc, &current, None, scope)
                    .await?;
                let holders = self
                    .folder_pair_holders(&view, &desc, &pairs, scope)
                    .await?;
                Some((here, holders))
            }
            _ => None,
        };
        if let Some((false, holders)) = folder_pairs.take_if(|(here, h)| !*here && !h.is_empty()) {
            // Nothing here to take back, so nothing here is written; the
            // pair's entries on the other engrams are the whole removal. When
            // there are none either, the edit below refuses as it always has.
            let done = self
                .take_back_folder_pairs(&view, holders, &actor, scope)
                .await;
            if done.removed.is_empty() {
                let named: Vec<String> = done
                    .not_removed
                    .iter()
                    .map(|(permalink, reason)| format!("'{permalink}' ({reason})"))
                    .collect();
                // The same variant the REST withdrawal answers this with:
                // nothing is wrong with the request, the entries just sit
                // where this caller may not write.
                return Err(EngineError::Conflict(format!(
                    "no {FOLDER_PAIR_RULE} acknowledgment on '{}' could be taken back; the pair's acknowledgment stays on {}",
                    desc.permalink,
                    named.join(", ")
                )));
            }
            let mut response = json!({
                "domain": desc.domain,
                "permalink": desc.permalink,
                "path": desc.path,
                "operation": p.operation,
                "unchanged": true,
                "evolve_ack_removed": FOLDER_PAIR_RULE,
            });
            done.note(&mut response);
            if overlay.is_some() {
                response["draft"] = json!(true);
            }
            return Ok(response);
        }

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
        // A MANIFEST edit may switch Reflection on, which meets the screens
        // the domain policies meet. The facts are gathered here, before any
        // lock, because the closure below cannot await, and only while the
        // switch is off: an edit of a MANIFEST that says on cannot switch it
        // on, and gathering would read the pending check from disk for
        // nothing.
        let switch_before = is_manifest.then(|| self.reflection_switch(&desc.domain));
        let switch_facts = if switch_before == Some(crystalline_core::ReflectionSwitch::Off) {
            Some(self.switch_facts(&desc.domain).await?)
        } else {
            None
        };
        let mut heading_stripped: Option<String> = None;
        let mut placed_after_section = false;
        let mut manifest_text: Option<String> = None;
        let mut written_text: Option<String> = None;
        let edited = self
            .apply_source_edit_staged(
                &desc,
                &source,
                &view,
                p.expected_checksum.as_deref(),
                &actor,
                model.as_deref(),
                scope,
                peer,
                // A set_frontmatter that leaves the engram saying what it
                // said writes nothing; every other operation writes as sent.
                p.operation == "set_frontmatter",
                // An edit that switches the Reflection off closes its rooms
                // first, while it is still on; the shared body does it just
                // before it writes, so an edit that lands in an open room
                // over the MANIFEST leaves it to that room's save. A draft
                // moves no switch.
                switch_before == Some(crystalline_core::ReflectionSwitch::On) && overlay.is_none(),
                |current| {
                    // The REPORTED model here, not the one resolved against
                    // this actor: the block this call stamps is held against
                    // the actor it records (above), and a `verified` entry is
                    // held against the actor IT records, which only the arm
                    // that builds it knows.
                    let (text, stripped, placed) = self.apply_edit(
                        current,
                        p,
                        &desc.permalink,
                        &actor,
                        p.model.as_deref(),
                        ack.as_ref(),
                    )?;
                    heading_stripped = stripped;
                    placed_after_section = placed;
                    if let Some(facts) = &switch_facts {
                        super::reflection::screen_switch(current, &text, facts)?;
                    } else if is_manifest {
                        // The switch was on when this edit began; one that
                        // went off meanwhile is not switched on unscreened.
                        super::reflection::refuse_unscreened_switch_on(current, &text)?;
                    }
                    if is_manifest {
                        manifest_text = Some(text.clone());
                    } else {
                        written_text = Some(text.clone());
                    }
                    Ok(text)
                },
            )
            .await
            .map_err(|failure| failure.error)?;
        // A draft moves no switch: the folder's MANIFEST decides.
        if let Some(before) = switch_before
            && overlay.is_none()
        {
            self.announce_switch_if_moved(&desc.domain, before, change_label(scope));
        }

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
        // A room was open and would not take the edit, so it went to the
        // file, the row or the draft behind it. The room takes it in now by
        // a three-way merge when it can, and the receipt says what the
        // people in it meet, in the words a reflection's write uses.
        if edited.room_passed {
            self.note_passed_room(&mut response, &desc, overlay, peer)
                .await;
        }
        // An edit that changed nothing wrote nothing, and says so; the key is
        // absent on every edit that wrote.
        if edited.unchanged {
            response["unchanged"] = json!(true);
        }
        note_unmirrored(&mut response, edited.warning);
        note_domain_names_normalized(&mut response, edited.normalized);
        match &ack {
            Some(AckDraft::Record(entry)) => response["evolve_ack"] = ack_json(entry),
            Some(AckDraft::Remove(rule)) => response["evolve_ack_removed"] = json!(rule),
            None => {}
        }
        if let Some((true, holders)) = folder_pairs {
            // Best effort from here on: this engram's own edit has landed, so
            // a holder that cannot be rewritten is named on the receipt
            // rather than turning a removal that happened into an error.
            self.take_back_folder_pairs(&view, holders, &actor, scope)
                .await
                .note(&mut response);
        }
        note_heading_stripped(&mut response, heading_stripped);
        note_placed_after_section(&mut response, placed_after_section);
        let verify = domain_verify_config(&source);
        if let Some(text) = &manifest_text {
            note_manifest_findings(&mut response, text, verify.as_ref());
        }
        if let Some(text) = written_text.or(manifest_text) {
            note_markdown_findings(
                &mut response,
                crystalline_core::lint_engram_source(&text),
                verify.as_ref(),
            );
        }
        Ok(response)
    }

    /// Tell an open room an edit went past it to the source, and say on the
    /// receipt what the people in it meet. It is always `landed: file`,
    /// never `live`: the edit went to the source, not into the room. With
    /// `room` and `room_note` when the change collides with their unsaved
    /// text or waits behind a conflict or a failed save of the room's own;
    /// a room that merged it in owes the person nothing to be told. A room
    /// that has said goodbye, or closed meanwhile, gets nothing and the
    /// receipt says nothing about it.
    async fn note_passed_room(
        &self,
        receipt: &mut Value,
        desc: &EngramDescriptor,
        overlay: Option<&str>,
        peer: Option<&AgentPeer>,
    ) {
        let Some(rooms) = self.collab_rooms() else {
            return;
        };
        let doc = RoomDoc::Engram(desc.permalink.clone());
        let absorbed = match rooms.absorb_doc_change(&desc.domain, &doc, overlay).await {
            Ok(outcome) => outcome.absorbed,
            Err(e) => {
                tracing::debug!(
                    domain = %desc.domain,
                    permalink = %desc.permalink,
                    "the open room did not take the edit in: {e}"
                );
                return;
            }
        };
        // The agent stands in the strip as it does after a live landing, and
        // `present` leaves it out the same way.
        let mine = match peer {
            Some(peer) => {
                rooms
                    .touch_agent_presence(&desc.domain, &desc.permalink, overlay, peer)
                    .await
            }
            None => None,
        };
        let present = rooms.participants(&desc.domain, &doc, overlay, mine).await;
        match absorbed {
            Absorbed::Merged => {
                receipt["landed"] = json!("file");
                receipt["present"] = json!(present);
            }
            Absorbed::Conflict | Absorbed::Deferred => {
                super::reflection::note_room_outcome(receipt, absorbed, present);
            }
        }
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
        scope: &crate::scope::Scope,
        apply: F,
    ) -> Result<Option<String>>
    where
        F: FnMut(&str) -> Result<String>,
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
            scope,
            None,
            false,
            false,
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
        scope: &crate::scope::Scope,
        peer: Option<&AgentPeer>,
        skip_unchanged: bool,
        close_rooms_on_switch_off: bool,
        mut apply: F,
    ) -> std::result::Result<SourceEdited, SourceEditFailure>
    where
        F: FnMut(&str) -> Result<String>,
    {
        // Every arm below edits the LF form of what it read, whatever line
        // endings the file, the row or the draft held: the whole document is
        // written back as LF, and an edit's own LF text (a find_text, a
        // section heading) matches it. Each arm compares its
        // `expected_checksum` against the bytes it read BEFORE this
        // conversion, which is what a read handed out.
        let mut apply = move |current: &str| apply(&crystalline_core::to_lf(current));
        // Whether an edit's result says what the text it was applied to said,
        // compared as parsed values against the LF form `apply` saw. Asked
        // after `apply` and before the generated stamp, so an edit that
        // changes nothing never reaches the stamp or a write.
        let unchanged = |current: &str, edited: &str| {
            skip_unchanged
                && crystalline_core::frontmatter::same_content(
                    &crystalline_core::to_lf(current),
                    edited,
                )
        };
        let nothing_written = || SourceEdited {
            warning: None,
            live: None,
            normalized: 0,
            unchanged: true,
            room_passed: false,
        };
        // A MANIFEST edit may switch read_only on, which is the owner's
        // (decision D10, on every tier). The verdict is asked here, before any
        // lock, because `domain_is_read_only` may take the store lock and
        // `apply` cannot await. A domain that is read-only already is not a
        // switch-on (only the owner's unlock reaches here for it).
        let may_switch_on = !is_manifest_path(&desc.path)
            || self.domain_is_read_only(&desc.domain).await
            || self
                .require_domain_owner_refusing(
                    &desc.domain,
                    scope,
                    Self::owner_key_refusal(&desc.domain, crystalline_core::READ_ONLY_KEY),
                )
                .await
                .is_ok();
        let manifest_domain = desc.domain.clone();
        let mut apply = move |current: &str| -> Result<String> {
            let edited = apply(current)?;
            if !may_switch_on && crystalline_core::read_only_of_source(&edited).is_on() {
                return Err(Engine::owner_key_refusal(
                    &manifest_domain,
                    crystalline_core::READ_ONLY_KEY,
                ));
            }
            Ok(edited)
        };
        // **The live arm, and it comes before every other one.** While a
        // co-editing room is open over this document, the room's text IS the
        // engram: somebody has it on screen, the file and the row are both
        // behind it, and the room's own saver is what makes anything durable.
        // So an edit composes into the document, and the caller's
        // `expected_checksum` is evaluated against the text the document
        // holds.
        //
        // **Only into a room that will save it, and only onto the text it
        // was computed from.** A room in conflict, with a failed save, or
        // whose final save has run may never write what it is given, so an
        // edit composed into it could be lost while its receipt said it
        // landed. And a person typing between the look below and the compose
        // would have their typing taken back by the diff to a target computed
        // from the older text. `CollabSessions::compose_while_saving`
        // decides both under the room's own lock. A compose refused because
        // the text moved is tried again on the text the room holds then,
        // which may now be unchanged or no longer match; a room that will not
        // save sends the edit on to the arms below, and the room meets it as
        // an external change on its next save. Nothing is composed on a
        // refused try, so an append is never doubled.
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
        //
        // The checksums the live arm accepted, kept for the arms below, each
        // beside the checksum of the stored bytes its room stood over: a
        // caller who read the room's unsaved text holds a checksum the file
        // or the row never had, and an edit that goes on to them is accepted
        // with any checksum the live arm would have accepted - as long as the
        // file or the row is still the one that room last saved. A file
        // another writer changed since is news the caller has not seen.
        let mut live_sums: Vec<(String, String)> = Vec::new();
        // Whether a room was open over the document and did not take the
        // edit, which the receipt reports.
        let mut room_passed = false;
        if let Some(rooms) = self.collab_rooms() {
            let overlay = view.actor();
            for attempt in 0..UNGUARDED_EDIT_ATTEMPTS {
                let Some(live) = rooms
                    .live_text(&desc.domain, &desc.permalink, overlay)
                    .await
                else {
                    break;
                };
                room_passed = true;
                // The document's text is what a caller guarding this edit read
                // a moment ago, so it is what the guard compares against - the
                // file's checksum would refuse every guarded edit made while
                // anybody had the page open.
                //
                // A clean room stands for the stored bytes and accepts their
                // checksum, so a guard taken before it opened still holds;
                // the checksum of its text is accepted too, since that is the
                // same document.
                let text_sum = sha256_hex(live.text.as_bytes());
                let accepted = expected_checksum
                    .is_none_or(|expected| expected == live.checksum || expected == text_sum);
                live_sums.push((live.checksum.clone(), live.base_checksum.clone()));
                live_sums.push((text_sum, live.base_checksum.clone()));
                if !live.saving {
                    break;
                }
                if let Some(expected) = expected_checksum
                    && !accepted
                {
                    return Err(SourceEditFailure::before(EngineError::Conflict(
                        stale_edit_message(expected, &live.checksum),
                    )));
                }
                let edited = apply(&live.text).map_err(SourceEditFailure::before)?;
                // Nothing to compose: the document already says it, and the
                // room will save what it says.
                if unchanged(&live.text, &edited) {
                    return Ok(nothing_written());
                }
                // The same two passes every other arm makes, in the same
                // order. An edit that skipped them would put a document in
                // front of a person that the saver then refuses, minutes
                // later, for a reason nobody watching could connect to this.
                let edited = touch_generated(&edited, actor, model, now_offset());
                let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
                let (edited, normalized) = self
                    .normalize_domain_spellings_for(&edited, scope)
                    .await
                    .map_err(SourceEditFailure::before)?;
                self.take_live_compose_hold().await;
                match rooms
                    .compose_while_saving(
                        &desc.domain,
                        &desc.permalink,
                        overlay,
                        Some(&live.text),
                        edited,
                        actor,
                        peer,
                    )
                    .await
                {
                    // No mirror warning: nothing was mirrored, because nothing
                    // was written. The room's saver owes that warning when it
                    // lands.
                    Ok(applied) => {
                        return Ok(SourceEdited {
                            warning: None,
                            live: Some(applied),
                            normalized,
                            unchanged: false,
                            room_passed: false,
                        });
                    }
                    // Somebody is typing. Wait a moment before the next try,
                    // a little longer each time, so steady typing still leaves
                    // a gap the edit can compose into. A guarded edit is
                    // refused as stale by the next look instead: the text its
                    // checksum named is gone.
                    Err(ComposeRefused::Moved) => {
                        if attempt + 1 < UNGUARDED_EDIT_ATTEMPTS {
                            tokio::time::sleep(LIVE_RETRY_PAUSE * (1 << attempt)).await;
                        }
                    }
                    Err(ComposeRefused::NotSaving) => break,
                }
            }
        }
        // Whether the arms below accept `expected` against the bytes they
        // read, whose checksum is `found`.
        let accepts = |expected: &str, found: &str| {
            expected == found
                || live_sums
                    .iter()
                    .any(|(sum, base)| sum == expected && base == found)
        };

        // A folder MANIFEST edit that switches the Reflection off closes the
        // domain's reflection rooms before it writes, while the switch is
        // still on, so each room's last save lands in its page. Here, past
        // the live arm, so it runs exactly when the edit is about to be
        // written: an edit that composed into a room over the MANIFEST
        // returned above, and that room's own save closes them. The text is
        // worked out unlocked, as the write below will work it out; a later
        // refusal costs the rooms their sockets, never their text. `apply`
        // works on the LF form of what it is handed (its first wrapper
        // above), so a CRLF file is edited here exactly as the write edits it.
        if close_rooms_on_switch_off {
            let raw = view
                .text_at(source, desc)
                .await
                .map_err(SourceEditFailure::before)?
                .unwrap_or_default();
            let fresh = expected_checksum
                .is_none_or(|expected| accepts(expected, &sha256_hex(raw.as_bytes())));
            if fresh && let Ok(after) = apply(&raw) {
                self.close_reflection_rooms_if_switching_off(&desc.domain, Some(&after))
                    .await;
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
                .text_for_draft(source, desc)
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
                if !accepts(expected, &found) {
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
            // No draft is written, so none is created either.
            if unchanged(&current, &edited) {
                return Ok(nothing_written());
            }
            let edited = touch_generated(&edited, actor, model, now_offset());
            let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
            let (edited, normalized) = self
                .normalize_domain_spellings_for(&edited, scope)
                .await
                .map_err(SourceEditFailure::before)?;
            if self.take_armed_failure() {
                return Err(SourceEditFailure::before(EngineError::Internal(
                    "reindex failed (test seam)".to_string(),
                )));
            }
            let warning = view
                .write_from(desc.domain_id, &desc.path, &edited, Some(&current))
                .await
                .map_err(SourceEditFailure::before)?;
            // Neither tail below runs. A draft of the MANIFEST is one actor's
            // proposal about the domain's routing, not the domain's routing,
            // and a draft belongs in no folder's generated index - both of
            // those are properties of what the team reviewed.
            return Ok(SourceEdited {
                warning,
                live: None,
                normalized,
                unchanged: false,
                room_passed,
            });
        }

        // Answered by whichever arm below runs, so the tail's `SourceEdited`
        // reports the count whichever kind of source this write landed on.
        let (normalized, checksum, before, after) = match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                // The row of an engram only the previous host of a host move
                // has is the only shared copy of it: say so and name the
                // remedy, rather than fail on the missing file.
                if !abs.exists() {
                    self.refuse_only_elsewhere(
                        &desc.domain,
                        desc.domain_id,
                        ElsewhereRow::Engram(&desc.path),
                    )
                    .await
                    .map_err(SourceEditFailure::before)?;
                }
                let current = std::fs::read_to_string(&abs)
                    .map_err(|source| SourceEditFailure::before(text_read_error(&abs, source)))?;
                // The CAS token, when the caller presents one: compared inside
                // the lock, against the bytes just read, exactly as save_engram
                // compares (and accepted as the live arm accepted it).
                if let Some(expected) = expected_checksum {
                    let found = sha256_hex(current.as_bytes());
                    if !accepts(expected, &found) {
                        return Err(SourceEditFailure::before(EngineError::Conflict(
                            stale_edit_message(expected, &found),
                        )));
                    }
                }
                let edited = apply(&current).map_err(SourceEditFailure::before)?;
                // Returned rather than handed to the tail below: the tail is
                // the announcement and the index work a write owes.
                if unchanged(&current, &edited) {
                    return Ok(nothing_written());
                }
                let edited = touch_generated(&edited, actor, model, now_offset());
                let edited = Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
                let (edited, count) = self
                    .normalize_domain_spellings_for(&edited, scope)
                    .await
                    .map_err(SourceEditFailure::before)?;
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
                let indexed = {
                    let store = self.store.lock().await;
                    self.reindex_file(&*store, desc.domain_id, root, &desc.path)
                        .await
                };
                if let Err(e) = indexed {
                    // The host lock moved while the file was written: the
                    // edited text is kept and the file put back, so nothing
                    // of this edit landed.
                    if super::fence::is_file_host_refusal(&e) {
                        return Err(SourceEditFailure::before(self.refuse_file_write(
                            e,
                            &desc.domain,
                            &abs,
                            Some(current.as_bytes()),
                            crystalline_core::to_lf(&edited).as_bytes(),
                            Some(super::fence::EngramKeep {
                                rel: &desc.path,
                                text: &edited,
                                account: crate::scope::overlay_actor(scope),
                                actor,
                            }),
                        )));
                    }
                    return Err(SourceEditFailure::after(e));
                }
                (
                    count,
                    sha256_hex(edited.as_bytes()),
                    engram_names(&current, &desc.permalink),
                    engram_names(&edited, &desc.permalink),
                )
            }
            ContentSource::Virtual => {
                // An edit without a checksum is last-write-wins, and on a
                // shared Postgres database that has to survive another
                // instance writing the same row: the store's compare waits for
                // that writer and then refuses, because the text this edit was
                // applied to is gone. So the edit is applied again to what is
                // stored now, up to `UNGUARDED_EDIT_ATTEMPTS` times, and lands
                // on top of the other writer's text. An edit that presents a
                // checksum is never retried: the refusal is what it asked for.
                let mut attempt = 0usize;
                loop {
                    attempt += 1;
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
                    // A token the live arm accepted, over the row as it stands,
                    // is satisfied by that row: the edit then swaps against
                    // the row it just read, as an unguarded one does. Any
                    // other token stays the guard, and a stale one is refused.
                    let found = sha256_hex(current.as_bytes());
                    let guard = expected_checksum
                        .filter(|expected| *expected == found || !accepts(expected, &found));
                    let expected = guard.map(str::to_string).unwrap_or_else(|| found.clone());
                    let edited = apply(&current).map_err(SourceEditFailure::before)?;
                    if unchanged(&current, &edited) {
                        // The store's compare and swap is never reached on
                        // this return, so a checksum the caller presented is
                        // compared here, exactly as the other arms compare it:
                        // a stale one is refused even when nothing would change.
                        if let Some(presented) = guard
                            && found != presented
                        {
                            return Err(SourceEditFailure::before(EngineError::Conflict(
                                stale_edit_message(presented, &found),
                            )));
                        }
                        return Ok(nothing_written());
                    }
                    let edited = touch_generated(&edited, actor, model, now_offset());
                    let edited =
                        Self::enforce_temporal(edited).map_err(SourceEditFailure::before)?;
                    let (edited, count) = self
                        .normalize_domain_spellings_for(&edited, scope)
                        .await
                        .map_err(SourceEditFailure::before)?;
                    let stamp = virtual_stamp(&edited);
                    // The seam, on this arm: a token nothing can match, so the
                    // store raises its own compare-and-swap conflict and rolls
                    // the transaction back. See `Engine::fail_next_source_edit`.
                    // Never retried: it stands for a conflict, not a race.
                    let armed = self.take_armed_failure();
                    let expected = if armed { "0".repeat(64) } else { expected };
                    let store = self.store.lock().await;
                    // Every failure here is a `before`, and that is exact rather
                    // than generous: `index_markdown` runs the compare and swap,
                    // the chunking and the reference resolution inside one store
                    // transaction and rolls it back on any error, so a virtual
                    // source that refuses still holds the bytes it held. A
                    // concurrent edit therefore comes back as the `Conflict` it
                    // is and the caller may undo whatever it wrote first, which
                    // is the failure that actually happens in the field.
                    let indexed = self
                        .index_markdown(
                            &*store,
                            desc.domain_id,
                            &desc.path,
                            &edited,
                            stamp,
                            Some(&expected),
                            true,
                        )
                        .await;
                    drop(store);
                    match indexed {
                        Ok(_) => {
                            break (
                                count,
                                sha256_hex(edited.as_bytes()),
                                engram_names(&current, &desc.permalink),
                                engram_names(&edited, &desc.permalink),
                            );
                        }
                        Err(e)
                            if guard.is_none()
                                && !armed
                                && attempt < UNGUARDED_EDIT_ATTEMPTS
                                && is_stale_edit(&e) =>
                        {
                            tracing::debug!(
                                domain = desc.domain.as_str(),
                                path = desc.path.as_str(),
                                attempt,
                                "an unguarded edit lost a race with another writer; \
                                 applying it again to the stored text"
                            );
                        }
                        Err(e) => return Err(SourceEditFailure::before(e)),
                    }
                }
            }
        };

        // Announced here rather than in `edit_engram_as`, so every edit that
        // reaches a file or a row passes this line: a retirement, a split's
        // tail, a successor's back-link, evolve's rewrites, the name report's
        // fixes, the keyed MANIFEST policy edit, the MANIFEST `domain_name`
        // write-back and a domain rename's MANIFEST and relink steps all
        // funnel through `apply_source_edit`. The live arm and the draft arm
        // returned above; the room's save and `DomainView::put` announce
        // those.
        self.announce(Change::Engram(EngramChanged {
            domain: desc.domain.clone(),
            permalink: desc.permalink.clone(),
            path: desc.path.clone(),
            kind: ChangeKind::Modified,
            from: None,
            checksum: Some(checksum),
            actor: Some(actor.to_string()),
            draft_of: None,
            audience: None,
        }));
        // An edit that moved the permalink or changed the title may be what a
        // link in another domain has been waiting for.
        if after != before {
            self.bind_references_to(&desc.domain, &after.0, &after.1)
                .await;
        }

        // An edit may have rewritten this domain's MANIFEST, its routing and
        // its declared name. The store locks above are all released.
        self.after_source_write(&desc.domain, source, &desc.path)
            .await;
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
            normalized,
            unchanged: false,
            room_passed,
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
    /// operation and for content placed as sent; and whether an
    /// `insert_after_section` put its content after the whole section
    /// ([`crystalline_core::emit::SectionEdit::placed_after_section`]).
    #[allow(clippy::too_many_arguments)]
    fn apply_edit(
        &self,
        source: &str,
        p: &EditParams,
        permalink: &str,
        actor: &str,
        model: Option<&str>,
        ack: Option<&AckDraft>,
    ) -> Result<(String, Option<String>, bool)> {
        // The three section edits share one dispatch with every other kind
        // of document that edits sections. Replace and insert_after put their
        // content under a heading that stays: a repeat of that heading is
        // dropped by the core edit, and reported here so the receipt can say
        // so. insert_before never strips one.
        if let Some(op) = SectionOp::parse(&p.operation, p.include_subsections) {
            let content = self.require_content(p)?;
            let section = self.require_section(p)?;
            let edit = apply_section_op(source, op, section, content).map_err(section_err)?;
            return Ok((edit.text, edit.heading_stripped, edit.placed_after_section));
        }
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
            "set_frontmatter" => {
                Self::apply_set_frontmatter(source, p, permalink, actor, model, ack)?
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "unknown edit operation '{other}'; expected append, prepend, find_replace, replace_section, insert_before_section, insert_after_section or set_frontmatter"
                )));
            }
        };
        Ok((text, None, false))
    }

    /// Refuses a `set_frontmatter` the single-line writers would get wrong.
    ///
    /// Those writers replace or remove the key's own line and nothing under
    /// it, so on a value that spans lines - a block list, a block scalar, a
    /// flow collection, an indented continuation - a set leaves the old lines
    /// joined to the new value and a removal folds them into the key above.
    /// Either writes a false value without a word. A new value holding a line
    /// break is refused for the same reason: it cannot be one line.
    ///
    /// `verified` and `evolve_ack` are not guarded: both are block-valued by
    /// design, written by block-aware emitters, and their values are folded to
    /// one line before they are written. Setting `stale_after` is not guarded
    /// either, since its writer takes an indented bound along; clearing it is,
    /// under both spellings.
    fn guard_one_line_scalar(
        source: &str,
        key: &str,
        value: Option<&str>,
        permalink: &str,
    ) -> Result<()> {
        let checked: &[&str] = match (key, value) {
            ("stale_after", Some(_)) => &[],
            ("stale_after", None) => &["stale_after", "review_after"],
            (
                "status" | "valid_from" | "valid_to" | "source_date" | "resource"
                | "source_version" | "salience",
                _,
            ) => std::slice::from_ref(&key),
            _ => return Ok(()),
        };
        if value.is_some_and(|v| v.contains(['\n', '\r'])) {
            return Err(EngineError::Invalid(format!(
                "{key} takes a one-line value and this one holds a line break. \
                 To write a value over several lines, edit the frontmatter text \
                 with find_replace instead"
            )));
        }
        if let Some(held) = checked
            .iter()
            .find(|k| crystalline_core::emit::frontmatter_value_spans_lines(source, k))
        {
            return Err(EngineError::Invalid(format!(
                "{held} in '{permalink}' holds a list or a value over several \
                 lines, and set_frontmatter only replaces a one-line value. \
                 Edit the frontmatter text with find_replace instead"
            )));
        }
        Ok(())
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
        // A list key takes the whole new list and nothing else, and a scalar
        // key takes one value: each mix-up is refused with the other form
        // named, rather than guessed at. A key outside the settable set falls
        // through to the refusal that names that set.
        if LIST_FRONTMATTER_KEYS.contains(&key) {
            return match (&p.values, value) {
                // Each list key has its own writer: tags fold to the tag
                // spelling, a location is kept exactly as given.
                (Some(values), None) if key == "sources" => Self::set_sources_list(source, values),
                (Some(values), None) => Self::set_tags(source, values),
                _ => Err(EngineError::Invalid(format!(
                    "{key} is a list: pass the whole new list as values"
                ))),
            };
        }
        if p.values.is_some() && SETTABLE_FRONTMATTER_KEYS.contains(&key) {
            return Err(EngineError::Invalid(format!(
                "{key} takes one value: pass value"
            )));
        }
        Self::guard_one_line_scalar(source, key, value, permalink)?;

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
            "resource" | "source_version" => Ok(match value {
                // Plain text, written through the YAML emitter so a URL with
                // `: ` or `#` stays one quoted scalar.
                Some(v) => set_frontmatter_field(source, key, v),
                None => remove_frontmatter_field(source, key),
            }),
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

    /// Replace an engram's whole `tags` list, the list form of
    /// `set_frontmatter`. Each entry is trimmed and folded to the canonical
    /// lowercase-with-hyphens spelling ([`crystalline_core::fold_tag`]), blanks
    /// are dropped and repeats are kept once in first order. One entry that
    /// cannot fold refuses the whole edit by name rather than writing a tag
    /// verify would flag. The list is written as a block list whatever shape
    /// the old value had; an empty list removes the key.
    fn set_tags(source: &str, values: &[String]) -> Result<String> {
        let mut tags: Vec<String> = Vec::with_capacity(values.len());
        for raw in values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
            let tag = crystalline_core::fold_tag(raw).ok_or_else(|| {
                EngineError::Invalid(format!(
                    "'{}' cannot be a tag: a tag is lowercase letters, digits and hyphens, such as confluence-source",
                    raw.escape_debug()
                ))
            })?;
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
        Ok(crystalline_core::emit::set_frontmatter_list(
            source, "tags", &tags,
        ))
    }

    /// Replace an engram's whole `sources` list, the locations it was
    /// distilled from, the list form of `set_frontmatter`. Each entry is
    /// trimmed, blanks are dropped and repeats are kept once in first order.
    /// The spelling is kept exactly: a `$contains` filter matches a location
    /// byte for byte, so folding one would hide it. An entry the metadata
    /// index cannot hold (its compact JSON over
    /// [`crystalline_core::META_VALUE_MAX_BYTES`], measured with
    /// [`crystalline_core::meta_value_text`] as the index measures it)
    /// refuses the whole edit by its 1-based position in `values`, blanks
    /// counted, the way verify's E011 counts elements. An empty list removes
    /// the key.
    fn set_sources_list(source: &str, values: &[String]) -> Result<String> {
        let mut sources: Vec<String> = Vec::with_capacity(values.len());
        for (i, raw) in values.iter().enumerate() {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            if crystalline_core::meta_value_text(&Value::String(entry.to_string())).is_none() {
                return Err(EngineError::Invalid(format!(
                    "sources element {} is too long for the metadata index, so a $contains filter could not find it",
                    i + 1
                )));
            }
            if !sources.iter().any(|s| s == entry) {
                sources.push(entry.to_string());
            }
        }
        Ok(crystalline_core::emit::set_frontmatter_list(
            source, "sources", &sources,
        ))
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

#[cfg(test)]
mod settable_keys_tests {
    use super::*;

    const SOURCE: &str = "---\ntype: engram\ntitle: T\npermalink: t\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\nbody\n";

    /// A value each key's arm accepts.
    fn value_for(key: &str) -> Option<String> {
        Some(match key {
            "status" => "stable".to_string(),
            "valid_from" | "valid_to" | "stale_after" | "source_date" => "2026-02-01".to_string(),
            "salience" => "5".to_string(),
            _ => "x".to_string(),
        })
    }

    fn set(key: &str) -> Result<String> {
        let list = LIST_FRONTMATTER_KEYS.contains(&key);
        let p = EditParams {
            identifier: "t".to_string(),
            domain: "d".to_string(),
            operation: "set_frontmatter".to_string(),
            key: Some(key.to_string()),
            value: if list { None } else { value_for(key) },
            values: list.then(|| vec!["x".to_string()]),
            ..EditParams::default()
        };
        Engine::apply_set_frontmatter(SOURCE, &p, "t", "tester", None, None)
    }

    #[test]
    fn every_listed_key_has_an_arm_and_an_unlisted_key_is_refused() {
        // `evolve_ack` needs a draft the sweep computed, so its arm answers
        // with its own value message here; what matters is that no listed key
        // falls through to the refusal.
        for key in SETTABLE_FRONTMATTER_KEYS {
            if let Err(e) = set(key) {
                assert!(
                    !e.to_string().contains("cannot set"),
                    "{key} is listed but has no arm: {e}"
                );
            }
        }
        assert!(SETTABLE_FRONTMATTER_KEYS.contains(&"resource"));
        assert!(SETTABLE_FRONTMATTER_KEYS.contains(&"source_version"));
        assert!(SETTABLE_FRONTMATTER_KEYS.contains(&"tags"));
        assert!(SETTABLE_FRONTMATTER_KEYS.contains(&"sources"));
        assert!(LIST_FRONTMATTER_KEYS.contains(&"sources"));
        for key in LIST_FRONTMATTER_KEYS {
            assert!(
                SETTABLE_FRONTMATTER_KEYS.contains(key),
                "{key} is a list key but not settable"
            );
        }
        for key in [
            "title",
            "permalink",
            "type",
            "recorded_at",
            "generated",
            "not_a_key",
        ] {
            let e = set(key).expect_err(key).to_string();
            assert!(e.contains(&format!("cannot set '{key}'")), "{key}: {e}");
        }
    }

    /// `set_frontmatter` on `source` with `key` and `value`.
    fn set_on(source: &str, key: &str, value: Option<&str>) -> Result<String> {
        let p = EditParams {
            identifier: "t".to_string(),
            domain: "d".to_string(),
            operation: "set_frontmatter".to_string(),
            key: Some(key.to_string()),
            value: value.map(str::to_string),
            ..EditParams::default()
        };
        Engine::apply_set_frontmatter(source, &p, "t", "tester", None, None)
    }

    /// An engram written by hand with its locations as a block list, which
    /// parses as no `resource` at all - so `V111` sends an agent here.
    const LISTED: &str = "---\ntype: engram\ntitle: T\npermalink: t\ntags:\n  - t\nstatus: stable\nresource:\n  - https://a.example\n  - https://b.example\nrecorded_at: 2026-01-01\n---\n\nbody\n";

    fn assert_refused_toward_find_replace(result: Result<String>, what: &str) {
        let e = result.expect_err(what).to_string();
        assert!(e.contains("find_replace"), "{what}: {e}");
    }

    #[test]
    fn setting_a_list_valued_key_is_refused_toward_find_replace() {
        // The single-line setter would replace only `resource:` and leave the
        // two items under the new value, which then reads as one false
        // citation joining all three.
        assert_refused_toward_find_replace(
            set_on(LISTED, "resource", Some("https://c.example")),
            "set on a list",
        );
    }

    #[test]
    fn removing_a_list_valued_key_is_refused_toward_find_replace() {
        // Removing only the key line would fold the items into the key above.
        assert_refused_toward_find_replace(set_on(LISTED, "resource", None), "remove a list");
    }

    #[test]
    fn a_block_scalar_or_a_flow_list_is_refused_too() {
        for shape in [
            "resource: >-\n  https://a.example\n",
            "resource: [https://a.example, https://b.example]\n",
        ] {
            let source = format!(
                "---\ntype: engram\ntitle: T\nstatus: stable\n{shape}recorded_at: 2026-01-01\n---\n\nbody\n"
            );
            assert_refused_toward_find_replace(
                set_on(&source, "resource", Some("https://c.example")),
                shape,
            );
        }
    }

    #[test]
    fn a_value_holding_a_newline_is_refused() {
        for key in ["resource", "source_version", "status"] {
            assert_refused_toward_find_replace(
                set_on(SOURCE, key, Some("https://a.example\nhttps://b.example")),
                key,
            );
        }
    }

    #[test]
    fn a_hand_written_bound_on_its_own_line_is_set_but_not_removed() {
        // Setting `stale_after` goes through the block-aware writer, which
        // takes the indented date along; clearing it would not, so it is
        // refused - under the legacy `review_after` spelling too.
        for spelling in ["stale_after", "review_after"] {
            let source = SOURCE.replace(
                "status: stable\n",
                &format!("status: stable\n{spelling}:\n  2026-08-01\n"),
            );
            let set = set_on(&source, "stale_after", Some("2026-12-01")).unwrap();
            assert!(!set.contains("2026-08-01"), "{set}");
            assert_refused_toward_find_replace(set_on(&source, "stale_after", None), spelling);
        }
    }

    #[test]
    fn a_one_line_value_is_still_replaced_and_removed() {
        // The control: the guard is about shape, not about the key.
        let one = SOURCE.replace(
            "status: stable\n",
            "status: stable\nresource: https://a.example\n",
        );
        let set = set_on(&one, "resource", Some("https://c.example")).unwrap();
        let parsed = crystalline_core::parse_engram(&set).unwrap();
        assert_eq!(
            parsed.frontmatter.resource.as_deref(),
            Some("https://c.example")
        );
        let removed = set_on(&one, "resource", None).unwrap();
        assert!(!removed.contains("resource"), "{removed}");
        crystalline_core::parse_engram(&removed).unwrap();
    }
}

#[cfg(test)]
mod list_keys_tests {
    use super::*;

    fn engram(tags: &str) -> String {
        format!(
            "---\ntype: engram\ntitle: T\npermalink: t\n{tags}status: stable\nrecorded_at: 2026-01-01\n---\n\nbody\n"
        )
    }

    fn params(key: &str, value: Option<&str>, values: Option<&[&str]>) -> EditParams {
        EditParams {
            identifier: "t".to_string(),
            domain: "d".to_string(),
            operation: "set_frontmatter".to_string(),
            key: Some(key.to_string()),
            value: value.map(str::to_string),
            values: values.map(|v| v.iter().map(|s| s.to_string()).collect()),
            ..EditParams::default()
        }
    }

    fn set_values(source: &str, key: &str, values: &[&str]) -> Result<String> {
        Engine::apply_set_frontmatter(
            source,
            &params(key, None, Some(values)),
            "t",
            "tester",
            None,
            None,
        )
    }

    fn tags_of(source: &str) -> Vec<String> {
        crystalline_core::parse_engram(source)
            .unwrap()
            .frontmatter
            .tags
    }

    fn sources_of(source: &str) -> Vec<String> {
        let parsed = crystalline_core::parse_engram(source).unwrap();
        match parsed.frontmatter.extra.get("sources") {
            None => Vec::new(),
            Some(value) => serde_json::from_value(serde_json::to_value(value).unwrap())
                .unwrap_or_else(|e| panic!("sources is not a list of strings ({e}): {source}")),
        }
    }

    #[test]
    fn a_tag_is_added_whatever_shape_the_old_tags_had() {
        for old in [
            "tags:\n  - old\n",
            "tags: [old]\n",
            "tags:\n- old\n",
            "tags: old\n",
        ] {
            let out = set_values(&engram(old), "tags", &["old", "new"]).unwrap();
            assert_eq!(tags_of(&out), ["old", "new"], "{old:?}: {out}");
            assert!(out.contains("tags:\n- old\n- new\n"), "{old:?}: {out}");
            assert_eq!(out.matches("old").count(), 1, "{old:?}: {out}");
        }
    }

    #[test]
    fn tags_are_written_where_there_were_none() {
        let out = set_values(&engram(""), "tags", &["first"]).unwrap();
        assert_eq!(tags_of(&out), ["first"], "{out}");
    }

    #[test]
    fn a_shorter_list_removes_a_tag_and_an_empty_one_removes_the_key() {
        let source = engram("tags:\n  - a\n  - b\n");
        let out = set_values(&source, "tags", &["b"]).unwrap();
        assert_eq!(tags_of(&out), ["b"], "{out}");
        let out = set_values(&source, "tags", &[]).unwrap();
        assert!(!out.contains("tags"), "{out}");
        assert!(tags_of(&out).is_empty());
        // Entries that are blank after trimming count as nothing.
        let out = set_values(&source, "tags", &["  ", ""]).unwrap();
        assert!(!out.contains("tags"), "{out}");
    }

    #[test]
    fn tags_are_folded_and_deduped_in_first_order() {
        let out = set_values(
            &engram(""),
            "tags",
            &[
                "Confluence Source",
                "api",
                "confluence-source",
                "API",
                "a_b",
            ],
        )
        .unwrap();
        assert_eq!(tags_of(&out), ["confluence-source", "api", "a-b"], "{out}");
    }

    #[test]
    fn a_tag_that_cannot_fold_is_refused_by_name() {
        let source = engram("tags:\n  - a\n");
        let e = set_values(&source, "tags", &["ok", "c++"])
            .expect_err("c++ is not a tag")
            .to_string();
        assert!(e.contains("'c++'"), "{e}");
        assert!(!e.contains('\n'), "one line: {e}");
    }

    #[test]
    fn value_on_a_list_key_points_at_values() {
        let p = params("tags", Some("new"), None);
        let e = Engine::apply_set_frontmatter(&engram(""), &p, "t", "tester", None, None)
            .expect_err("value on tags")
            .to_string();
        assert!(
            e.contains("tags is a list: pass the whole new list as values"),
            "{e}"
        );
        // Neither given is the same mistake, not a removal.
        let p = params("tags", None, None);
        let e = Engine::apply_set_frontmatter(&engram(""), &p, "t", "tester", None, None)
            .expect_err("nothing on tags")
            .to_string();
        assert!(e.contains("pass the whole new list as values"), "{e}");
    }

    #[test]
    fn values_on_a_scalar_key_points_at_value() {
        for key in ["status", "resource", "valid_to", "salience"] {
            let e = set_values(&engram(""), key, &["x"])
                .expect_err(key)
                .to_string();
            assert!(
                e.contains(&format!("{key} takes one value: pass value")),
                "{key}: {e}"
            );
        }
        // An identity key is refused as unsettable, whatever form it came in.
        let e = set_values(&engram(""), "title", &["x"])
            .expect_err("title")
            .to_string();
        assert!(e.contains("cannot set 'title'"), "{e}");
    }

    /// sources is a list key: the whole new list replaces the old one, in
    /// every shape a hand or write_engram may have left it in (serde_yaml_ng
    /// writes a metadata list without indent).
    #[test]
    fn a_location_is_added_and_dropped_whatever_shape_sources_had() {
        for old in [
            "sources:\n  - https://a.example\n",
            "sources:\n- https://a.example\n",
            "sources: [https://a.example]\n",
            "sources: https://a.example\n",
        ] {
            let out = set_values(
                &engram(old),
                "sources",
                &["https://a.example", "https://b.example"],
            )
            .unwrap();
            assert_eq!(
                sources_of(&out),
                ["https://a.example", "https://b.example"],
                "{old:?}: {out}"
            );
            assert!(
                out.contains("sources:\n- https://a.example\n- https://b.example\n"),
                "{old:?}: {out}"
            );
            let out = set_values(&out, "sources", &["https://b.example"]).unwrap();
            assert_eq!(sources_of(&out), ["https://b.example"], "{old:?}: {out}");
        }
    }

    #[test]
    fn an_empty_sources_list_removes_the_key() {
        let source = engram("sources:\n  - https://a.example\n");
        for empty in [&[][..], &["", "   "][..]] {
            let out = set_values(&source, "sources", empty).unwrap();
            assert!(!out.contains("sources"), "{empty:?}: {out}");
            crystalline_core::parse_engram(&out).unwrap();
        }
    }

    /// Entries are trimmed, blanks dropped and repeats kept once in first
    /// order, but never folded: a location is matched byte for byte.
    #[test]
    fn sources_are_trimmed_and_deduped_but_never_folded() {
        let out = set_values(
            &engram(""),
            "sources",
            &[
                "  https://Example.com/A  ",
                "",
                "https://example.com/a",
                "https://Example.com/A",
                "notedown://jordi/n1/p0#b3",
            ],
        )
        .unwrap();
        assert_eq!(
            sources_of(&out),
            [
                "https://Example.com/A",
                "https://example.com/a",
                "notedown://jordi/n1/p0#b3",
            ],
            "{out}"
        );
        // tags still fold: the dispatch is by key, not by "is a list".
        let out = set_values(&engram(""), "tags", &["Retry Policy"]).unwrap();
        assert_eq!(tags_of(&out), ["retry-policy"], "{out}");
    }

    /// A comma, a fragment, a query, a quote, a backslash and non-ASCII text
    /// all survive the write as the exact same string.
    #[test]
    fn sources_keep_their_spelling_through_a_round_trip() {
        let entries = [
            "https://example.com/straße?a=1,2#x",
            "github:acme/api:src/lib.rs",
            "say \"hi\" \\ there",
            "-leading-hyphen",
            "trailing:",
        ];
        let out = set_values(&engram(""), "sources", &entries).unwrap();
        assert_eq!(sources_of(&out), entries, "{out}");
    }

    /// The limit is the index's: the compact JSON text of the entry, quotes
    /// and escapes included. The refusal names the position in the list the
    /// caller sent, blanks counted, and stays on one line.
    #[test]
    fn a_sources_entry_is_measured_as_the_index_measures_it() {
        let fits = "x".repeat(crystalline_core::META_VALUE_MAX_BYTES - 2);
        let out = set_values(&engram(""), "sources", &[fits.as_str()]).unwrap();
        assert_eq!(sources_of(&out), std::slice::from_ref(&fits));

        let over = "x".repeat(crystalline_core::META_VALUE_MAX_BYTES - 1);
        let quotes = "\"".repeat(600);
        for (list, n) in [
            (vec!["https://a.example", over.as_str()], 2),
            (vec!["", "  ", quotes.as_str()], 3),
        ] {
            let e = set_values(&engram(""), "sources", &list)
                .expect_err("over the index limit")
                .to_string();
            assert_eq!(
                e,
                format!(
                    "sources element {n} is too long for the metadata index, so a $contains filter could not find it"
                )
            );
        }
    }

    #[test]
    fn value_on_sources_points_at_values() {
        let p = params("sources", Some("https://a.example"), None);
        let e = Engine::apply_set_frontmatter(&engram(""), &p, "t", "tester", None, None)
            .expect_err("value on sources")
            .to_string();
        assert!(
            e.contains("sources is a list: pass the whole new list as values"),
            "{e}"
        );
    }
}
