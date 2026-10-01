use super::*;

/// Whether a grant matched only by its TITLE yields to what the reader's own
/// view answers `name` with: an engram in that view whose permalink is the
/// name (the team's page, unless they deleted it, or a draft of their own),
/// or one of their own drafts that answers it the way
/// [`DomainView::resolve_draft`] would. The one title rule for reads, unjoined
/// edits and saves ([`Engine::granted_answer`]) and for the joined and room
/// lookups ([`Engine::granted_draft_named`]).
pub(super) async fn title_grant_yields(
    own: &DomainView<'_>,
    domain_id: DomainId,
    name: &str,
) -> Result<bool> {
    Ok(own.permalink_holder(domain_id, name).await?.is_some()
        || own.holds_own_draft_named(domain_id, name).await?)
}

impl Engine {
    // --- read ----------------------------------------------------------------

    /// One engram's exact file text and identity, addressed by domain name:
    /// the base view of that domain, read the way the collab session layer
    /// reads it at open. Deliberately thin - [`Engine::read_engram`] resolves
    /// references and builds hints this caller never reads.
    ///
    /// **Whose text** is a view's to say, and this name-addressed form answers
    /// with the folder the team reviewed, whoever else is drafting - which is
    /// what it answered before there was anything else it could have answered.
    /// A caller that already holds a reader's own view asks that view instead.
    pub async fn engram_text(&self, domain: &str, identifier: &str) -> Result<EngramText> {
        DomainView::base(self, domain, &HashSet::new())?
            .engram_text(identifier)
            .await
    }

    /// The exact text a domain holds at a domain-relative PATH right now, or
    /// `None` when nothing is there: the base view of that domain.
    ///
    /// Path-addressed on purpose, and the counterpart of
    /// [`Engine::restore_engram`]: a collab room whose engram vanished from
    /// the index has no identifier left to resolve, and before it puts its own
    /// text back it has to know whether somebody else's bytes are sitting at
    /// that path (an external rename, or a delete followed by a recreate).
    /// The `permalink` reported back is what the index answers for this path,
    /// which an external rename may just have moved; it falls back to the path
    /// slug when no row holds the path any more.
    pub async fn engram_text_at_path(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<Option<EngramText>> {
        DomainView::base(self, domain, &HashSet::new())?
            .engram_text_at_path(path)
            .await
    }

    /// Read an engram's full markdown and resolved frontmatter. The content
    /// comes from the local file when a file domain holds it, else from the
    /// database (virtual domains, and non-host reads over a shared database). The
    /// returned `checksum` is the CAS token an `edit_engram` can pass back as
    /// `expected_checksum` to detect a change since this read.
    ///
    /// `scope` decides what may be read at all: an engram in a domain the
    /// caller may not see is [`EngineError::NotFound`], the same miss an engram
    /// nobody wrote produces, and the inbound sample below never names a domain
    /// the caller cannot see.
    /// The draft this caller holds a share-link to, when the identifier they
    /// read names it - the ONE read in this engine that answers with another
    /// actor's work.
    ///
    /// **Why a read widens at all.** A grant is visibility, and a person's
    /// agent is that person: it signs in as the same account, and it has no
    /// screen to open a link on. If only the browser could see a granted
    /// draft, somebody could be handed a colleague's page and their own agent
    /// could not be shown what they were looking at. So the account's agent
    /// sees it where its person does - the draft the link was for.
    ///
    /// **By the names every engram answers to.** The granted draft answers to
    /// its permalink and to its title, the title compared case-insensitively
    /// like the resolver's title step, and never to its file path. A
    /// permalink match beats a title match; among equals the path that sorts
    /// first byte-wise wins. A title match yields to whatever in the reader's
    /// own view has that name as its permalink, and to their own drafts, the
    /// resolver's order ([`Engine::granted_answer`]).
    ///
    /// **And nowhere else.** The widening is this function and this function
    /// only: search, listing, the reference candidate set and every other read
    /// are untouched, so a grantee's view of the domain is exactly what it was
    /// except at one path. That is the whole of ruling 1's "reads outside the
    /// granted path never show it", and
    /// `a_grantees_search_still_excludes_the_owners_draft` asserts it from the
    /// other side.
    ///
    /// Three properties are deliberate and each is visible in the answer:
    ///
    /// * **it says whose it is.** `draft: true` and `draft_owner` ride on the
    ///   payload, because a granted draft standing where the team's own page
    ///   stands must never be mistaken for that page;
    /// * **no transitivity.** The references are reported as they parse and
    ///   nothing resolves: a link in the granted draft onto another of the
    ///   owner's drafts names a page that was not shared, and resolving it
    ///   would make one grant into a tour of an overlay. A link onto a page
    ///   the team holds is unresolved here too, which is the cost of the rule
    ///   and is stated rather than hidden;
    /// * **nothing points at it.** A draft has no inbound references, because
    ///   nobody can write a reference to a page only its author can read.
    ///
    /// **It is asked before the resolution, so the granted draft stands OVER
    /// the base row at that path**, and that is a decision rather than an
    /// artefact of where the call sits: a draft always stands over the base for
    /// whoever may see it, which is the rule this whole mode runs on, and the
    /// link is what says the grantee may. The alternative - the team's page
    /// wins and a grant only ever adds a page no file holds - would mean a
    /// redraft of a shared page stayed invisible to the grantee's agent while
    /// its person was reading it on screen. Moving the call in
    /// [`Engine::read_engram`] into the `NotFound` arm is the whole of the
    /// flip, if that is ever the wanted answer.
    ///
    /// `None` - the ordinary answer, for every caller and every identifier -
    /// short-circuits before any store read when the caller has no account, so
    /// the common path costs nothing.
    pub(super) async fn granted_read(
        &self,
        p: &ReadParams,
        scope: &crate::scope::Scope,
        hidden: &HashSet<String>,
    ) -> Result<Option<Value>> {
        if crate::scope::overlay_actor(scope).is_none() {
            return Ok(None);
        }
        // Which domain the identifier is asking about. An absolute address
        // names its own; otherwise the caller's `domain` does, and a read with
        // neither is not a read this can answer - a grant names one domain,
        // and guessing which is not something a widening may do.
        let domain = match CrystallineUrl::parse(&p.identifier) {
            Some(url) => url.domain,
            None => match p.domain.as_deref() {
                Some(named) => named.to_string(),
                None => return Ok(None),
            },
        };
        match self
            .granted_answer(&domain, &p.identifier, scope, hidden)
            .await?
        {
            Some((owner, draft)) => Ok(Some(granted_draft_json(&domain, &owner, &draft)?)),
            None => Ok(None),
        }
    }

    /// The granted draft, and whose, that a read of `identifier` in `domain`
    /// answers with, or `None` for the ordinary answer.
    ///
    /// **One rule for reads and edits**: [`Engine::granted_read`] answers
    /// with it, and an unjoined edit or save of the same name is refused with
    /// the join sentence when it answers, so an agent is never shown a draft
    /// under a name its next write would put somewhere else.
    ///
    /// Matched the way the resolver matches, in the resolver's order: a grant
    /// whose permalink is the name wins; a grant whose title is the name
    /// applies only when nothing in the reader's own view has that name as
    /// its permalink (the team's page, unless they deleted it, or a draft of
    /// their own) and none of their own drafts answers it the way
    /// [`DomainView::resolve_draft`] would. A base page that shares only the
    /// TITLE never beats the grant. An absolute address naming another domain
    /// matches nothing here.
    pub(super) async fn granted_answer(
        &self,
        domain: &str,
        identifier: &str,
        scope: &crate::scope::Scope,
        hidden: &HashSet<String>,
    ) -> Result<Option<(String, GrantedDraft)>> {
        let Some(account) = crate::scope::overlay_actor(scope) else {
            return Ok(None);
        };
        let Some(access) = self.domain_access.get() else {
            return Ok(None);
        };
        let bare = match CrystallineUrl::parse(identifier) {
            Some(url) if url.domain != domain => return Ok(None),
            Some(url) => url.permalink,
            None => identifier.to_string(),
        };
        // **The registered-set screen, the same one every other read makes.**
        // A grant is the author's word about one draft and never about a
        // domain, so it must not outlive the grantee's access to the domain
        // that draft is in: an account whose membership was taken away, or a
        // domain that has since been made private, is answered here exactly as
        // it is answered everywhere else - there is no grant to widen with.
        // `domain_entry_scoped` rather than the bare `hidden` set, so this is
        // the identical check `Engine::require_domain` makes (it covers a
        // domain nobody registered too), and its refusal is turned into `None`
        // rather than raised: the ordinary read path below is what answers a
        // caller who named a domain they may not see, and it already answers
        // it without saying the domain exists.
        if self.domain_entry_scoped(domain, hidden).is_err() {
            return Ok(None);
        }
        let held = access
            .overlay_grants_held(&account, domain)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        if held.is_empty() {
            return Ok(None);
        }
        // Every live grant that answers the name, and how.
        let mut answers: Vec<(GrantMatch, String, String, GrantedDraft)> = Vec::new();
        for (path, owner) in held {
            if owner == account {
                continue;
            }
            // The freshness check every other grant surface makes: a link
            // whose draft has gone opens nothing, so it widens nothing.
            let Some(draft) = self.overlay_draft_at(domain, &owner, &path).await? else {
                continue;
            };
            if let Some(how) = grant_match(&draft, &bare) {
                answers.push((how, path, owner, draft));
            }
        }
        if answers.is_empty() {
            return Ok(None);
        }
        // A permalink match first; among equals the path that sorts first
        // byte-wise, the tie-break `find_engram` uses.
        answers.sort_by(|a, b| (a.0, a.1.as_bytes()).cmp(&(b.0, b.1.as_bytes())));
        // This reader's own view of the domain, for the questions below that
        // are about THEM rather than about the grant. The read-only id
        // lookup, never an upserting one: asking about a domain this index has
        // never seen must not register it.
        let own = DomainView::for_read(self, domain, hidden, scope)?;
        let domain_id = {
            let store = self.store.lock().await;
            store.domain_id(domain).await?
        };
        // Whether the reader's own view answers the name ahead of a title
        // match, asked once and only when a title match needs it.
        let mut view_answers: Option<bool> = None;
        for (how, path, owner, draft) in answers {
            // **The reader's own row at that path wins.** A link handed to
            // somebody is not a reason to hide their own unfolded work from
            // them: the precedence is the mode's own - your own draft, then
            // what a grant widens, then the page the team holds - and the
            // granted draft is still exactly where the link put it, on the
            // surface that opened it. A deletion of their own counts here too:
            // it is their decision about that path, and standing somebody
            // else's draft on top of it would answer around it.
            if let Some(domain_id) = domain_id
                && own.holds_own_entry(domain_id, &path).await?
            {
                continue;
            }
            if how == GrantMatch::Title
                && let Some(domain_id) = domain_id
            {
                let answered = match view_answers {
                    Some(answered) => answered,
                    None => {
                        let answered = title_grant_yields(&own, domain_id, &bare).await?;
                        view_answers = Some(answered);
                        answered
                    }
                };
                if answered {
                    continue;
                }
            }
            return Ok(Some((owner, draft)));
        }
        Ok(None)
    }

    /// Who is in the room over one document, for the read payload's `present`.
    ///
    /// Asked only when [`CollabSessions::live_text`] already answered, so the
    /// second lookup is over a room that was open a moment ago; a room that
    /// closed in between answers an empty list rather than an error, which is
    /// the true thing to say about who is in a room nobody is in.
    async fn live_participants(
        &self,
        desc: &EngramDescriptor,
        view: &DomainView<'_>,
        except: Option<yrs::ClientID>,
    ) -> Vec<String> {
        match self.collab_rooms() {
            Some(rooms) => {
                rooms
                    .participants(&desc.domain, &desc.permalink, view.actor(), except)
                    .await
            }
            None => Vec::new(),
        }
    }

    pub async fn read_engram(&self, p: &ReadParams, scope: &crate::scope::Scope) -> Result<Value> {
        self.read_engram_in(p, scope, true, None).await
    }

    /// [`Engine::read_engram`], with the reading agent as a named peer in the
    /// room it may be answered from.
    ///
    /// A read that comes back live is a read over somebody's shoulder: the
    /// bytes are their unsaved work, and the person who typed them is owed the
    /// same name in the strip an edit puts there. So the claim is made here
    /// too, and it expires the same way. A read answered from the file or the
    /// row puts nothing anywhere, which is nearly every read.
    pub async fn read_engram_present(
        &self,
        p: &ReadParams,
        scope: &crate::scope::Scope,
        peer: Option<&AgentPeer>,
    ) -> Result<Value> {
        self.read_engram_in(p, scope, true, peer).await
    }

    /// [`Engine::read_engram`] with the live document deliberately ignored:
    /// the stored version, and its checksum.
    ///
    /// **For the surface where a checksum is a VERSION TOKEN rather than a
    /// description.** The JSON API hands its checksum over as an `ETag`, the
    /// browser sends it back as `If-Match`, and the durable write it guards
    /// compares against the row or the file. A checksum of somebody's unsaved
    /// document would be a token no save could ever match, so a reader who
    /// opened a page while a colleague had it open would be told their edit was
    /// stale, handed the same token again, and told so again - until the
    /// colleague's session happened to save. That surface has its own live view
    /// of a document, and it is the co-editing socket.
    ///
    /// An agent's read is the other case and takes the live text: it has no
    /// socket, its `expected_checksum` is compared by the verb that composes
    /// into the document, and being answered the bytes the engram actually says
    /// right now is the whole of Task 14.
    #[doc(hidden)]
    pub async fn read_engram_stored(
        &self,
        p: &ReadParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.read_engram_in(p, scope, false, None).await
    }

    async fn read_engram_in(
        &self,
        p: &ReadParams,
        scope: &crate::scope::Scope,
        live_wins: bool,
        peer: Option<&AgentPeer>,
    ) -> Result<Value> {
        let hidden = self.hidden_for(scope).await?;
        let p = &self.localized(p, &hidden).await;
        // The one path a read crosses between two overlays on: a draft this
        // caller was handed a link to. Asked first, so the grant stands over
        // whatever the team's own folder holds at that path - a draft always
        // stands over the base for whoever may see it, and the link is what
        // says they may. See `Engine::granted_read`.
        if let Some(granted) = self.granted_read(p, scope, &hidden).await? {
            return Ok(granted);
        }
        let (desc, source, overlay) = self
            .resolve_shadowed(&p.identifier, p.domain.as_deref(), &hidden, scope)
            .await?;
        // The reader's own view of the domain the identifier landed in, which
        // is the one `resolve_shadowed` just answered through.
        let view = DomainView::for_read(self, &desc.domain, &hidden, scope)?;
        let content = match &overlay {
            Some(_) => view.text_at(&source, &desc).await?.ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no engram '{}' in domain '{}'",
                    p.identifier, desc.domain
                ))
            })?,
            None => self.load_content(&source, &desc).await?,
        };
        // **A co-editing room over this document outranks both.** While one is
        // open the room's text is what the engram says: somebody is typing
        // into it, the file and the row are both a save behind, and a reader
        // answered from either would be reading a version the room has already
        // moved past. The checksum below is the live text's too, so an edit
        // guarded with it is guarded against the document rather than against
        // the file - which is what makes read-then-edit work at all while
        // somebody is in there.
        let live = match live_wins {
            true => self.live_text_at(&desc, &view).await,
            false => None,
        };
        let present = match &live {
            Some(_) => {
                // The agent stands in the room first and is then told who is
                // in it: `present` is who this read is reading over, and the
                // reader is not one of them - not on the call that mints its
                // slot and not on the ones that find it standing. Another
                // agent working in the same document is.
                let mine = match (self.collab_rooms(), peer) {
                    (Some(rooms), Some(peer)) => {
                        rooms
                            .touch_agent_presence(&desc.domain, &desc.permalink, view.actor(), peer)
                            .await
                    }
                    _ => None,
                };
                self.live_participants(&desc, &view, mine).await
            }
            None => Vec::new(),
        };
        let is_live = live.is_some();
        let content = live.unwrap_or(content);
        // The checksum is of the bytes the file or the row holds, taken before
        // the text is handed back as LF: it is the token an edit or a save
        // compares against what it reads there, and an origin's base stamp is
        // of those bytes too. A CRLF file therefore reads back as LF with the
        // checksum of its CRLF bytes, and the next write lands it as LF.
        let checksum = sha256_hex(content.as_bytes());
        let content = crystalline_core::to_lf(&content).into_owned();
        let engram = parse_engram(&content).map_err(|e| EngineError::Invalid(e.to_string()))?;

        // Enrich the response with reference resolution: which outbound links
        // land, and who points back in. The descriptor carries the ids, so this
        // works for file, virtual and non-host reads alike.
        //
        // Outbound is asked of the row this reader is actually looking at, and
        // judged in their view - see [`DomainView::outbound`], which is both
        // halves of that sentence. Inbound stays the base row's, deliberately:
        // who points here is a fact about the address the team shares, and
        // nobody can write a reference to a draft only its author can read.
        let outbound = view.outbound(&desc).await?;
        let inbound = {
            let store = self.store.lock().await;
            let mut inbound = store
                .inbound_refs(desc.id, desc.domain_id, &desc.permalink, &desc.title)
                .await?;
            // Who points here is answered for the caller asking: a reference
            // out of a domain this caller may not see names that domain and one
            // of its file paths, so it is dropped before the count as well as
            // before the sample. The count states what the sample is drawn
            // from, and a count of references that cannot be shown would be a
            // second, quieter way of saying the domain is there.
            inbound.retain(|r| !hidden.contains(&r.src_domain));
            inbound
        };

        // A parsed reference resolves when a matching indexed row (same source
        // line, kind and target) is resolved. An unmatched parsed entry, which a
        // just-edited or non-host read can produce, is reported as unresolved.
        // The domain is matched by meaning, not by bytes: a row recorded under
        // one spelling of a domain and text that now spells it another way
        // (its canonical name, an alias, a former local name) are one link.
        let names = self.name_table_now().await;
        let resolves = |kind: EdgeKind, line: usize, target: &LinkTarget| -> bool {
            outbound.iter().any(|o| {
                o.kind == kind
                    && o.line == line
                    && o.to_target == target.target
                    && super::names::same_domain(
                        &names,
                        o.to_domain.as_deref(),
                        target.domain.as_deref(),
                    )
                    && o.resolved
            })
        };

        #[derive(serde::Serialize)]
        struct RelationOut<'a> {
            line: usize,
            rel_type: &'a str,
            target: &'a LinkTarget,
            resolved: bool,
        }
        #[derive(serde::Serialize)]
        struct LinkOut<'a> {
            line: usize,
            target: &'a LinkTarget,
            resolved: bool,
        }

        let relations: Vec<RelationOut> = engram
            .relations
            .iter()
            .map(|r| RelationOut {
                line: r.line,
                rel_type: &r.rel_type,
                target: &r.target,
                resolved: resolves(EdgeKind::Relation, r.line, &r.target),
            })
            .collect();
        let links: Vec<LinkOut> = engram
            .links
            .iter()
            .map(|l| LinkOut {
                line: l.line,
                target: &l.target,
                resolved: resolves(EdgeKind::Link, l.line, &l.target),
            })
            .collect();
        let resolved_outbound = relations.iter().filter(|r| r.resolved).count()
            + links.iter().filter(|l| l.resolved).count();

        let url = format!("crystalline://{}/{}", desc.domain, desc.permalink);
        let mut value = json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "title": desc.title,
            "type": desc.engram_type,
            "status": desc.status,
            "path": desc.path,
            "url": url,
            "content": content,
            "checksum": checksum,
            "frontmatter": engram.frontmatter,
            "observations": engram.observations,
            "relations": relations,
            "links": links,
        });
        let obj = value
            .as_object_mut()
            .expect("read_engram response is a JSON object");

        // One line saying whose page this is. Emitted only when the reader is
        // looking at a draft of their own - over a base row or at a path no
        // file holds - so a direct domain's payload never grows a key, and a
        // reader never sees the word about anybody else's work.
        if view.draft_at(desc.domain_id, &desc.path).await?.is_some() {
            obj.insert("draft".to_string(), json!(true));
        }

        // One word saying how this page differs from what the team has, for
        // a team domain that takes changes directly. A reviewing domain says
        // `draft` instead; a domain with no origin says nothing; a listing or
        // the log is never compared. Costs one parse of `state.json` and one
        // map lookup, never a file read: the checksum above is of the bytes
        // the page is about to show.
        if overlay.is_none()
            && origin::takes_part_in_local_change(&desc.path)
            && !self.reviews_changes(&desc.domain)
            && self.domain_has_origin(&desc.domain).unwrap_or(false)
            && let Ok((_, _, state_dir)) = self.origin_spec_for_domain(&desc.domain)
            && let Ok(Some(state)) = crystalline_remote::state::OriginState::load(&state_dir)
        {
            let base = ops::unshared_base(&state);
            match base.get(&desc.path) {
                None => {
                    obj.insert("local_change".to_string(), json!("added"));
                }
                Some(stamp) if stamp.sha256 != checksum => {
                    obj.insert("local_change".to_string(), json!("modified"));
                }
                Some(_) => {}
            }
        }

        // Two lines saying the bytes above are somebody's unsaved work and who
        // is holding them. Emitted only when a room is actually open, so every
        // other read on this instance answers exactly what it always did.
        if is_live {
            obj.insert("live".to_string(), json!(true));
            obj.insert("present".to_string(), json!(present));
        }

        // Inbound summary: how many references point here, with a small capped
        // sample so a heavily linked engram never bloats the response. Omitted
        // entirely when nothing points here.
        if !inbound.is_empty() {
            let refs: Vec<Value> = inbound
                .iter()
                .take(5)
                .map(|r| {
                    json!({
                        "domain": r.src_domain,
                        "path": r.src_path,
                        "kind": match r.kind {
                            EdgeKind::Relation => "relation",
                            EdgeKind::Link => "link",
                        },
                    })
                })
                .collect();
            obj.insert(
                "inbound".to_string(),
                json!({ "count": inbound.len(), "refs": refs }),
            );
        }

        // A build_context hint, emitted only when there is a neighbourhood to
        // explore: something points here, or something here resolves outward.
        if !inbound.is_empty() || resolved_outbound > 0 {
            obj.insert(
                "related".to_string(),
                json!(format!(
                    "build_context anchor {url} to explore linked knowledge"
                )),
            );
        }

        Ok(value)
    }

    /// One page of what points at an engram, with the per-relation summary of
    /// all of it: the browsing view of the inbound block [`Engine::read_engram`]
    /// samples.
    ///
    /// The read payload's `inbound` stays what it is - an exact count and five
    /// references, cheap enough to ride every read. This is for the case that
    /// count implies but cannot serve: hundreds or thousands of engrams pointing
    /// at one, where the answer is a map to browse rather than a list to print.
    /// Both are the same rows, so the counts agree.
    ///
    /// `q` matches the referencing engram's title or path, case-insensitively,
    /// and `rel` narrows to one relation type (`links_to` for prose wikilinks).
    /// `total` is exact under both; `types` ignores both, because a summary that
    /// shrank as it was used would be a map redrawing itself while it is read.
    ///
    /// `limit` is clamped to [`MAX_INBOUND_LIMIT`] and a page past the end is an
    /// empty page carrying the true total, never the first page's rows.
    ///
    /// An engram nobody wrote is [`EngineError::NotFound`], the same resolution
    /// every other read of one identifier opens with.
    ///
    /// Scoped: the anchor resolves through [`Engine::resolve_scoped`], so an
    /// engram in a domain the caller may not see is the not-found a missing
    /// one produces, and the domains it may not see are subtracted inside the
    /// query rather than from its answer. This list names other domains by
    /// name and path - it is the one read on this surface whose *rows* are
    /// mostly about somewhere else - so a referrer inside a hidden domain is
    /// absent from the page, from the total and from the per-relation summary
    /// alike. [`crate::scope::Scope::Unrestricted`] subtracts nothing.
    pub async fn inbound_references(
        &self,
        p: &ReadParams,
        q: Option<&str>,
        rel: Option<&str>,
        page: Option<usize>,
        limit: Option<usize>,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let hidden = self.hidden_for(scope).await?;
        let p = &self.localized(p, &hidden).await;
        let (desc, _) = self
            .resolve_scoped(&p.identifier, p.domain.as_deref(), &hidden)
            .await?;
        // Clamped rather than refused, the way the listing clamps its own: a
        // hand-written page number below one is answered with the first page,
        // and a page size past [`MAX_INBOUND_LIMIT`] is answered with that
        // many. The envelope reports the clamped values, so a caller is told
        // what it was actually given rather than having its own number read
        // back at it.
        let page = page.unwrap_or(1).max(1);
        let limit = limit.unwrap_or(10).clamp(1, MAX_INBOUND_LIMIT);
        // Sorted so the query text is stable for one caller across calls,
        // which keeps a prepared-statement cache and a log line honest; the
        // set itself is unordered.
        let mut exclude: Vec<String> = hidden.iter().cloned().collect();
        exclude.sort();
        let found = {
            let store = self.store.lock().await;
            store
                .inbound_page(&InboundQuery {
                    engram_id: desc.id,
                    domain_id: desc.domain_id,
                    permalink: &desc.permalink,
                    title: &desc.title,
                    q,
                    rel,
                    exclude_domains: &exclude,
                    page,
                    limit,
                })
                .await?
        };
        let types: Vec<Value> = found
            .types
            .iter()
            .map(|t| json!({ "rel": t.name, "count": t.count }))
            .collect();
        let hits: Vec<Value> = found
            .hits
            .iter()
            .map(|h| {
                json!({
                    "domain": h.domain,
                    "permalink": h.permalink,
                    "title": h.title,
                    "path": h.path,
                    "status": h.status,
                    "rel": h.rel,
                })
            })
            .collect();
        Ok(json!({
            "total": found.total,
            "page": page,
            "limit": limit,
            "count": hits.len(),
            "types": types,
            "hits": hits,
        }))
    }
}
