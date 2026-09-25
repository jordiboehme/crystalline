use super::*;

impl Engine {
    // --- search --------------------------------------------------------------

    /// Search across domains, embedding the query when the mode needs it.
    ///
    /// `scope` decides which domains are in range at all: a caller that named a
    /// domain it may not see gets what naming an unregistered domain gets - no
    /// hits from it, and no error saying it is there - and a caller that named
    /// none searches every domain minus those.
    pub async fn search_engrams(
        &self,
        p: &SearchParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        self.search_engrams_under(p, None, SearchOrder::default(), scope)
            .await
    }

    /// [`Engine::search_engrams`] narrowed to one domain-relative folder, which
    /// is what a folder view pages from: the same filter-only search, with the
    /// folder pushed into SQL beside the other filters, so `total` stays exact
    /// and paging is unchanged.
    ///
    /// The folder is segment-safe - see [`folder_prefix`] - and `None` or an
    /// empty value searches the whole scope, which is what every caller that
    /// never names a folder keeps getting.
    ///
    /// `order` is the listing's, and a separate argument for the reason the
    /// folder is: [`SearchParams`] is the MCP search tool's argument schema,
    /// and the MCP tool keeps the store's default order. A ranked search
    /// ignores it either way.
    ///
    /// The `total` in the envelope counts the folder recursively: every engram
    /// under it at any depth, since a folder listing promises the folder. The
    /// tree's own `total` counts one level and is deliberately smaller; see
    /// [`Engine::browse_domain`]. It is a separate verb rather than a
    /// field on [`SearchParams`] because that struct is the MCP search tool's
    /// argument schema, and a folder filter is a browsing affordance of this
    /// API rather than a knob worth spending an agent's context on.
    pub async fn search_engrams_under(
        &self,
        p: &SearchParams,
        folder: Option<&str>,
        order: SearchOrder,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let requested = parse_mode(p.search_type.as_deref())?;
        let hidden = self.hidden_for(scope).await?;
        let scoped = self.scoped_domains(&p.domains, &hidden).await?;
        let text = p.query.clone().filter(|s| !s.trim().is_empty());
        let mut query = SearchQuery {
            text: text.clone(),
            domains: Some(p.domains.clone()).filter(|d| !d.is_empty()),
            engram_type: p.engram_type.clone(),
            status: p.status.clone(),
            tags: Some(p.tags.clone()).filter(|t| !t.is_empty()),
            after: p.after.clone(),
            min_similarity: p.min_similarity,
            path_prefix: folder.and_then(folder_prefix),
            order,
            // Whose rows this search is entitled to: the base dimension plus
            // this caller's own drafts, where any domain in range reviews
            // changes at all. One value for the whole query, because a search
            // spans domains and the screen is a column predicate rather than a
            // per-domain decision - see `Engine::reading_actor`, which is where
            // the question and its residue are written down. A reader with no
            // identity names no actor either way and gets the base dimension
            // alone.
            actor: self.reading_actor(scope, &p.domains),
            limit: p.limit.unwrap_or(10).clamp(1, MAX_PAGE_LIMIT),
            page: p.page.unwrap_or(1).max(1),
            ..SearchQuery::default()
        };
        {
            let config = self.config.read().unwrap();
            query.salience_weight = config.salience_weight();
            query.retired_weight = config.retired_weight();
        }
        if let Some(mf) = &p.metadata_filters {
            query.metadata_filters =
                parse_metadata_filters(mf).map_err(|e| EngineError::Invalid(e.to_string()))?;
        }

        // Phase the store lock so it is never held across the provider embed
        // call, the same discipline as `embed_pending`: fetch the provider once,
        // hold the store lock only to resolve the effective mode (which reads
        // embedding coverage), then drop it before embedding the query and
        // relock for the search. The coverage snapshot can go stale between the
        // mode decision and the search, an accepted race of the same class as
        // already exists across two separate search calls.
        let provider = self.provider();
        let effective = {
            let store = self.store.lock().await;
            self.effective_mode(&*store, requested, text.is_some(), provider.is_some())
                .await?
        };
        query.mode = effective;

        // The visibility filter is applied after the mode is settled and before
        // the query is embedded: a search of nothing this caller may read costs
        // no embedding call and no store round trip, and still reports the mode
        // the same search over a visible domain would have reported.
        match scoped {
            ScopedDomains::AsAsked => {}
            ScopedDomains::Only(domains) => query.domains = Some(domains),
            ScopedDomains::Nothing => {
                return Ok(json!({
                    "mode": mode_str(effective),
                    "total": 0,
                    "page": query.page,
                    "limit": query.limit,
                    "count": 0,
                    "hits": Value::Array(Vec::new()),
                }));
            }
        }
        if matches!(effective, SearchMode::Semantic | SearchMode::Hybrid)
            && let Some(provider) = &provider
        {
            let q = text.clone().unwrap_or_default();
            let vecs = provider
                .embed_queries(&[q])
                .await
                .map_err(|e| EngineError::Internal(e.to_string()))?;
            query.query_embedding = vecs.into_iter().next();
            query.active_model = Some(self.model_id.clone());
        }

        let store = self.store.lock().await;
        let page = store.search(&query).await?;
        Ok(json!({
            "mode": mode_str(effective),
            "total": page.total,
            "page": page.page,
            "limit": page.limit,
            "count": page.items.len(),
            "hits": serde_json::to_value(&page.items).unwrap_or(Value::Null),
        }))
    }

    /// The nearest current engrams to `probe_text` that `scope` may see: the
    /// retrieval behind the write receipt's `similar` list.
    ///
    /// Vector-only on purpose - prose never goes through the lexical parser,
    /// where parentheses and AND/OR/NOT in an ordinary sentence read as
    /// operators. The mode is settled the way search settles it, so with no
    /// provider or no active embeddings for this model the answer is empty
    /// rather than a text search in disguise. `exclude` is the engram that was
    /// just written, matched on domain and permalink; the retirement set is
    /// dropped after the page comes back, which is why the page is one wider
    /// than the list. Ranking only: no score leaves this function.
    ///
    /// **The candidate set is the writer's own view of the index.** The
    /// advisory is a search, so it asks the same question about whose rows are
    /// in range: a writer in review mode whose neighbours are all still drafts
    /// would otherwise be told there is nothing near what they just wrote. Two
    /// consequences, and both are the shape rather than an accident.
    ///
    /// The base row at a path the writer is drafting is not a candidate at all,
    /// so a draft never lists the engram it is a draft of - it would be told to
    /// merge its own work into the team's wording of it. And `exclude` is
    /// matched by address across every actor's rows, because a hit says which
    /// engram it is and never whose row carried it - so the caller has to hand
    /// in the address the ROW answers to, which is not always the one its
    /// receipt names; [`Engine::attach_similar`] resolves that and says why.
    ///
    /// **A writer's own drafts can fill the list, and the cut stands.** The
    /// drafts are not additional, they are rows on one ladder, so an author
    /// holding several drafts on a topic is told about those and not about the
    /// reviewed engram further away. That is the ranking answering the question
    /// it was asked. Reserving a slot for a base row, or marking which
    /// neighbours are the caller's own drafts, both need a fact no hit carries -
    /// whether the row behind it was a draft - and putting it on
    /// [`crystalline_index::SearchHit`] would widen every search answer on
    /// every surface for this one advisory. So the policy is the cut, stated
    /// here and pinned by
    /// `an_authors_own_drafts_can_fill_the_advisory_and_the_cut_stands`.
    ///
    /// The scoping reconciliation, the mode decision and the phasing that never
    /// holds the store lock across the embed call are all
    /// [`Engine::search_engrams_under`]'s, repeated here rather than shared: the
    /// two bodies differ enough (no text, vector only, no envelope, typed rows,
    /// two post-filters) that a common helper would cost more than it saves, so
    /// a change to either belongs in both. One thing differs on purpose. That
    /// function settles the mode before it applies the scoping, so its envelope
    /// reports a truthful mode even to a caller who may see nothing; this one
    /// scopes first, because it has no envelope to be truthful in and would
    /// rather skip the coverage read for a caller with nothing to search.
    pub async fn similar_engrams(
        &self,
        probe_text: &str,
        exclude: Option<(&str, &str)>,
        scope: &crate::scope::Scope,
    ) -> Result<Vec<SimilarEngram>> {
        let Some(provider) = self.provider() else {
            return Ok(Vec::new());
        };
        let hidden = self.hidden_for(scope).await?;
        let domains = match self.scoped_domains(&[], &hidden).await? {
            ScopedDomains::AsAsked => None,
            ScopedDomains::Only(domains) => Some(domains),
            ScopedDomains::Nothing => return Ok(Vec::new()),
        };
        let effective = {
            let store = self.store.lock().await;
            self.effective_mode(&*store, SearchMode::Semantic, true, true)
                .await?
        };
        if !matches!(effective, SearchMode::Semantic) {
            return Ok(Vec::new());
        }
        let vecs = provider
            .embed_queries(&[probe_text.to_string()])
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        let Some(embedding) = vecs.into_iter().next() else {
            return Ok(Vec::new());
        };
        let query = SearchQuery {
            domains,
            mode: SearchMode::Semantic,
            query_embedding: Some(embedding),
            active_model: Some(self.model_id.clone()),
            // The advisory is a search, so it is the same question about whose
            // rows are in range, asked the same way (`Engine::reading_actor`).
            // A writer in review mode whose neighbours are all still drafts
            // would otherwise be told there is nothing near what they just
            // wrote; an instance where nothing reviews anything names nobody.
            actor: self.reading_actor(scope, &[]),
            // Pure cosine order: the fade would only reorder hits this drops.
            retired_weight: Some(1.0),
            limit: SIMILAR_PAGE,
            page: 1,
            ..SearchQuery::default()
        };
        let page = {
            let store = self.store.lock().await;
            store.search(&query).await?
        };
        Ok(page
            .items
            .into_iter()
            .filter(|hit| !is_retired_status(&hit.status))
            .filter(|hit| exclude.is_none_or(|(d, p)| !(hit.domain == d && hit.permalink == p)))
            .take(SIMILAR_LIMIT)
            .map(|hit| SimilarEngram {
                domain: hit.domain,
                permalink: hit.permalink,
                title: hit.title,
                status: hit.status,
                engram_type: hit.engram_type,
            })
            .collect())
    }

    /// Put the neighbours advisory on a write receipt, or leave it alone.
    ///
    /// Runs after the write has landed and can never fail it: every failure -
    /// no provider, no embeddings, a store error, the clock - is a debug line
    /// and an unchanged receipt. [`SIMILAR_TIMEOUT`] bounds the wait, but only
    /// where the probe yields; a store statement that steps synchronously is
    /// not cut by it and can run well past the budget, which is logged at
    /// `warn` rather than left silent. Off when `capture.similar` is off. The
    /// probe first waits, briefly, for the embed worker to drain what was
    /// just written, so a capture made a moment ago can be a neighbour of
    /// this one.
    ///
    /// The receipt must already name the engram that landed, as top-level
    /// string `domain` and `permalink` keys: they are what the advisory
    /// excludes itself by, and what an edit looks its title up from. A receipt
    /// shaped any other way is left untouched.
    pub async fn attach_similar(
        &self,
        receipt: &mut Value,
        probe: SimilarProbe<'_>,
        scope: &crate::scope::Scope,
    ) {
        if !self.config.read().unwrap().capture_similar() {
            return;
        }
        let (Some(domain), Some(permalink)) = (
            receipt
                .get("domain")
                .and_then(Value::as_str)
                .map(str::to_string),
            receipt
                .get("permalink")
                .and_then(Value::as_str)
                .map(str::to_string),
        ) else {
            return;
        };
        // The path the receipt names, for the overlay verbs that carry one. A
        // receipt without it is a direct domain's, which has no draft to
        // resolve an address against.
        let receipt_path = receipt
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Read off the variant before `probe` is moved into `work` below, so
        // the overrun warning can still name what kind of probe this was.
        let kind = match &probe {
            SimilarProbe::Write { .. } => "write",
            SimilarProbe::Markdown { .. } => "markdown",
            SimilarProbe::Edit { .. } => "edit",
        };
        let started = std::time::Instant::now();
        let work = async {
            // This writer's own view of the domain they just wrote in. The
            // write itself already screened the domain (`refuse_hidden_domain`
            // on the way in), so there is nothing left for this pass to screen.
            let view = DomainView::for_read(self, &domain, &HashSet::new(), scope)?;
            let text = match probe {
                SimilarProbe::Write {
                    title,
                    description,
                    body,
                } => similar::write_probe_text(title, description, body),
                SimilarProbe::Markdown { text } => similar::markdown_probe_text(text),
                SimilarProbe::Edit { new_text } => {
                    // Through this caller's own view of the domain, not the
                    // base rows alone. An engram that exists only as their
                    // draft has no base row to read a title off, and a
                    // base-only lookup answered `None` there - which skipped
                    // the advisory silently, on exactly the writes a domain in
                    // review mode is made of.
                    let title = view
                        .resolve(&permalink)
                        .await
                        .ok()
                        .map(|(desc, _)| desc.title);
                    title.and_then(|t| similar::edit_probe_text(&t, new_text))
                }
            };
            let Some(text) = text else {
                return Ok(Vec::new());
            };
            // The address to exclude is the one the row a search would answer
            // with carries, which is not always the one the receipt names. A
            // draft over a base row resolves to the BASE descriptor, so an
            // edit's receipt names the address the team knows the engram by,
            // while the draft's own row carries whatever address its
            // frontmatter gave it. Excluding the receipt's alone hands the
            // writer their own draft as a neighbour, under guidance that tells
            // them to merge into it. Excluding the draft's loses nothing:
            // wherever a draft stands at a path, the base row at that path is
            // shadowed out of the candidate set anyway.
            let exclude = view
                .draft_permalink_at(receipt_path.as_deref())
                .await?
                .unwrap_or_else(|| permalink.clone());
            self.await_embed_backlog(SIMILAR_BACKLOG_WAIT).await;
            self.similar_engrams(&text, Some((&domain, &exclude)), scope)
                .await
        };
        match tokio::time::timeout(SIMILAR_TIMEOUT, work).await {
            Ok(Ok(found)) => {
                tracing::debug!("similar probe completed in {:?}", started.elapsed());
                similar::attach(receipt, &found);
            }
            Ok(Err(e)) => tracing::debug!("similar probe skipped: {e}"),
            Err(_) => tracing::debug!("similar probe cut at {SIMILAR_TIMEOUT:?}"),
        }
        // The timeout above only cuts a poll that yields; a store statement
        // that steps synchronously runs past it undetected unless the wall
        // clock is checked here, after the fact.
        let elapsed = started.elapsed();
        if similar::probe_overran(elapsed) {
            tracing::warn!("{}", similar::overrun_warning(elapsed, &domain, kind));
        }
    }

    /// Wait, at most `budget`, for the embed worker to clear the backlog.
    /// Without a worker there is nothing to wait for and no wait happens.
    async fn await_embed_backlog(&self, budget: std::time::Duration) {
        if self.embed_tx.is_none() {
            return;
        }
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            match self.embedding_backlog().await {
                Ok(0) | Err(_) => return,
                Ok(_) if tokio::time::Instant::now() >= deadline => return,
                Ok(_) => tokio::time::sleep(SIMILAR_BACKLOG_POLL).await,
            }
        }
    }

    async fn effective_mode(
        &self,
        store: &dyn Store,
        requested: SearchMode,
        has_text: bool,
        has_provider: bool,
    ) -> Result<SearchMode> {
        if !matches!(requested, SearchMode::Semantic | SearchMode::Hybrid) {
            return Ok(requested);
        }
        if !has_text || !has_provider {
            return Ok(SearchMode::Text);
        }
        let coverage = store.embedding_coverage().await?;
        if coverage.has_active_embeddings(&self.model_id) {
            Ok(requested)
        } else {
            Ok(SearchMode::Text)
        }
    }
}
