use super::*;

impl Engine {
    // --- context -------------------------------------------------------------

    /// Traverse the graph around a `crystalline://` anchor.
    ///
    /// `scope` bounds the neighbourhood twice over: an anchor in a domain the
    /// caller may not see is the not-found an anchor that matched nothing gets,
    /// and a neighbour in such a domain is cut out of the slice before anything
    /// is ranked, so it neither appears nor lends its mass to what does.
    pub async fn build_context(
        &self,
        p: &ContextParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let url = CrystallineUrl::parse(&p.anchor).ok_or_else(|| {
            EngineError::Invalid(format!("anchor '{}' is not a crystalline:// URL", p.anchor))
        })?;
        let depth = p.depth.unwrap_or(1).clamp(1, 3);
        let max_related = p.max_related.unwrap_or(10);
        let domain_filter = Some(p.domains.clone()).filter(|d| !d.is_empty());
        let hidden = self.hidden_for(scope).await?;

        // A hidden domain skips the lookup and keeps the branch: a glob over one
        // falls into the same "matched no engrams" an empty glob produces, and a
        // named anchor into the same not-found a missing engram produces. Both
        // are reached by the same lines a visible domain reaches, which is what
        // makes the two indistinguishable.
        let visible_anchor = !hidden.contains(&url.domain);
        // The registered-set screen composes ahead of the actor dimension: the
        // overlay question is asked only about a domain this reader may see, so
        // a draft of their own is no way into one they may not.
        //
        // No view at all for a domain this reader may not see, and none for a
        // name nobody registered either: both keep the base seeds they were
        // handed, and both fall through to the same miss. Building one for
        // either would answer one of them with a refusal the other never gets.
        let view = visible_anchor
            .then(|| DomainView::for_read(self, &url.domain, &hidden, scope).ok())
            .flatten();
        let seeds: Vec<EngramDescriptor> = if url.glob {
            let base = if visible_anchor {
                let store = self.store.lock().await;
                store.list_engrams(&url.domain, None, None).await?
            } else {
                Vec::new()
            };
            match &view {
                Some(view) => view.list_over(base).await?,
                None => base,
            }
            .into_iter()
            .filter(|d| url.matches(&d.domain, &d.permalink))
            .collect()
        } else {
            let found = if visible_anchor {
                let store = self.store.lock().await;
                store.find_engram(&url.domain, &url.permalink).await?
            } else {
                None
            };
            let anchored = match &view {
                Some(view) => view.anchor(found, &url.permalink).await?,
                None => found,
            };
            match anchored {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        url.permalink, url.domain
                    )));
                }
            }
        };
        if seeds.is_empty() {
            return Err(EngineError::NotFound(format!(
                "anchor '{}' matched no engrams",
                p.anchor
            )));
        }
        let seed_ids: HashSet<i64> = seeds.iter().map(|d| d.id.0).collect();
        let ids: Vec<EngramId> = seeds.iter().map(|d| d.id).collect();
        // The traversal is asked as this caller, not as the anchor domain's
        // overlay: a slice crosses domains, so whose rows it may walk is one
        // question about the reader - the same one a search asks, through the
        // same `Engine::reading_actor` - rather than one domain's mode deciding
        // what another domain's edges say. The hop can land anywhere, so the
        // range it asks about is every registered domain.
        let reading = self.reading_actor(scope, &[]);
        let store = self.store.lock().await;
        let mut slice = store.neighbors(&ids, depth, reading.as_deref()).await?;
        // Cut before the ranking, not at output selection like the caller's own
        // `domains` filter below. The two look alike and are not: a presentation
        // filter leaves a node in the graph so it still conducts mass as a
        // bridge, and a node this caller may not see must not be in the graph at
        // all - a path that only exists through a private engram is a fact about
        // that engram.
        retain_visible(&mut slice, &hidden);

        // Rank the full slice before any filtering so a domain-filtered node
        // still conducts mass as a bridge; the domain filter applies only at
        // output selection, preserving the current presentation.
        let mass = context_rank(&slice, &seed_ids);
        let (weight, retired_weight) = {
            let config = self.config.read().unwrap();
            (
                config.salience_weight().unwrap_or(DEFAULT_SALIENCE_WEIGHT),
                config.retired_weight().unwrap_or(DEFAULT_RETIRED_WEIGHT),
            )
        };

        // Output pass: keep seeds in slice (ascending-id) order, then rank the
        // related nodes by spread mass lifted by the salience prior, highest
        // first with an ascending-id tiebreak, capped at max_related.
        let mut seed_nodes: Vec<&GraphNode> = Vec::new();
        let mut related: Vec<(f64, &GraphNode)> = Vec::new();
        for node in &slice.nodes {
            if let Some(filter) = &domain_filter
                && !filter.contains(&node.domain)
            {
                continue;
            }
            if seed_ids.contains(&node.id.0) {
                seed_nodes.push(node);
            } else {
                let score = mass.get(&node.id.0).copied().unwrap_or(0.0)
                    * (1.0 + salience_prior(node.salience, weight))
                    * retired_factor(&node.status, retired_weight);
                related.push((score, node));
            }
        }
        related.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.1.id.0.cmp(&b.1.id.0))
        });
        related.truncate(max_related);

        let mut kept: HashSet<i64> = HashSet::new();
        let mut nodes = Vec::new();
        for node in seed_nodes
            .into_iter()
            .chain(related.into_iter().map(|(_, node)| node))
        {
            let is_seed = seed_ids.contains(&node.id.0);
            kept.insert(node.id.0);
            let mut out = json!({
                "id": node.id.0,
                "domain": node.domain,
                "permalink": node.permalink,
                "title": node.title,
                "type": node.engram_type,
                "seed": is_seed,
            });
            mark_draft(&mut out, node);
            nodes.push(out);
        }
        let edges: Vec<Value> = slice
            .edges
            .iter()
            .filter(|e| kept.contains(&e.from.0) && kept.contains(&e.to.0))
            .map(|e| {
                json!({
                    "from": e.from.0,
                    "to": e.to.0,
                    "rel_type": e.rel_type,
                    "kind": match e.kind {
                        crystalline_index::EdgeKind::Relation => "relation",
                        crystalline_index::EdgeKind::Link => "link",
                    },
                })
            })
            .collect();

        Ok(json!({
            "anchor": url.to_url(),
            "depth": depth,
            "timeframe": p.timeframe,
            "nodes": nodes,
            "edges": edges,
        }))
    }

    /// The nodes and typed edges around an anchor, for a graph view.
    ///
    /// The same traversal [`Engine::build_context`] runs, answered in the flat
    /// shape a graph renderer wants: every node carries what a client labels and
    /// styles it with (`id`, `domain`, `permalink`, `title`, `status`, `type`),
    /// every edge keeps its direction and `rel_type`, and `truncated` says
    /// whether the cap cut anything. `id` is the index's own engram id, opaque
    /// to a client and stable only within one response: it is what the edges
    /// join on, never an address. `crystalline://domain/permalink` is the
    /// address.
    ///
    /// `depth` is clamped to one or two hops and `max_nodes` to at least one and
    /// at most [`MAX_GRAPH_NODES`], so a hand-written URL can ask neither for
    /// nothing nor for the whole index in one payload.
    ///
    /// Retired engrams come back like any other, with their status, because the
    /// graph is the shape of what is written rather than of what still holds:
    /// hiding a superseded node would break the chain that explains what replaced
    /// it. A client fades them; this does not drop them from an uncapped answer.
    ///
    /// When the cap bites, the anchors are kept first, then the rest are kept by
    /// the same spread mass and salience lift `build_context` ranks with - except
    /// that retired non-anchor nodes yield to live ones first, since a fading
    /// node is the one the budget can least afford to keep over live knowledge.
    /// `hidden` counts every node the cap cut, retired or not.
    pub async fn graph_neighborhood(
        &self,
        anchor: &str,
        depth: u8,
        max_nodes: usize,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let url = CrystallineUrl::parse(anchor).ok_or_else(|| {
            EngineError::Invalid(format!("anchor '{anchor}' is not a crystalline:// URL"))
        })?;
        let depth = depth.clamp(1, 2);
        let max_nodes = max_nodes.clamp(1, MAX_GRAPH_NODES);
        let hidden = self.hidden_for(scope).await?;

        let store = self.store.lock().await;
        // A hidden domain skips the lookup and keeps the branch, so it answers
        // with the same miss a visible domain with nothing in it answers with.
        // See [`Engine::build_context`], which seeds the same way.
        let visible_anchor = !hidden.contains(&url.domain);
        let base: Vec<EngramDescriptor> = if url.glob {
            if visible_anchor {
                store.list_engrams(&url.domain, None, None).await?
            } else {
                Vec::new()
            }
        } else {
            match visible_anchor {
                true => store
                    .find_engram(&url.domain, &url.permalink)
                    .await?
                    .into_iter()
                    .collect(),
                false => Vec::new(),
            }
        };
        drop(store);
        // See [`Engine::build_context`], which seeds through a view the same
        // way: a domain this reader may not see builds none.
        let view = visible_anchor
            .then(|| DomainView::for_read(self, &url.domain, &hidden, scope).ok())
            .flatten();
        let seeds: Vec<EngramDescriptor> = if url.glob {
            match &view {
                Some(view) => view.list_over(base).await?,
                None => base,
            }
            .into_iter()
            .filter(|d| url.matches(&d.domain, &d.permalink))
            .collect()
        } else {
            let found = base.into_iter().next();
            let anchored = match &view {
                Some(view) => view.anchor(found, &url.permalink).await?,
                None => found,
            };
            match anchored {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        url.permalink, url.domain
                    )));
                }
            }
        };
        if seeds.is_empty() {
            return Err(EngineError::NotFound(format!(
                "anchor '{anchor}' matched no engrams"
            )));
        }

        let seed_ids: HashSet<i64> = seeds.iter().map(|d| d.id.0).collect();
        let ids: Vec<EngramId> = seeds.iter().map(|d| d.id).collect();
        let mut slice = self
            .sweep_neighbors(&ids, depth, crate::scope::overlay_actor(scope).as_deref())
            .await?;
        // Before the ranking and before the cap, so a hidden neighbour is
        // neither drawn nor counted in `hidden` - that number reports what the
        // cap cut, and a node this caller may not see was never in the picture.
        retain_visible(&mut slice, &hidden);

        let mass = context_rank(&slice, &seed_ids);
        let weight = {
            let config = self.config.read().unwrap();
            config.salience_weight().unwrap_or(DEFAULT_SALIENCE_WEIGHT)
        };
        let mut anchors: Vec<&GraphNode> = Vec::new();
        let mut related: Vec<(f64, &GraphNode)> = Vec::new();
        for node in &slice.nodes {
            if seed_ids.contains(&node.id.0) {
                anchors.push(node);
            } else {
                let score = mass.get(&node.id.0).copied().unwrap_or(0.0)
                    * (1.0 + salience_prior(node.salience, weight));
                related.push((score, node));
            }
        }
        related.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.1.id.0.cmp(&b.1.id.0))
        });
        // Retired knowledge yields first when the cap bites. A stable partition
        // after the score sort: under the cap the kept SET is identical (order
        // within the payload is not part of the contract), over it the live
        // neighborhood survives and the hidden count below reports the cut.
        related.sort_by_key(|(_, node)| crystalline_index::is_retired_status(&node.status));

        let total = anchors.len() + related.len();
        let mut kept: HashSet<i64> = HashSet::new();
        let mut nodes = Vec::new();
        for node in anchors
            .into_iter()
            .chain(related.into_iter().map(|(_, node)| node))
            .take(max_nodes)
        {
            kept.insert(node.id.0);
            let mut out = json!({
                "id": node.id.0,
                "domain": node.domain,
                "permalink": node.permalink,
                "title": node.title,
                "status": node.status,
                "type": node.engram_type,
            });
            mark_draft(&mut out, node);
            nodes.push(out);
        }
        // An edge is only meaningful when both of its ends survived the cap; one
        // that lost an end would render as an arrow into nothing. The relation
        // and the prose link between one pair are one edge here, because the
        // payload states the type rather than the origin: an engram that both
        // declares `- links_to [[X]]` and writes the wikilink in its prose is
        // one line on the picture, not two drawn over each other.
        let mut drawn: HashSet<(i64, i64, &str)> = HashSet::new();
        let edges: Vec<Value> = slice
            .edges
            .iter()
            .filter(|e| kept.contains(&e.from.0) && kept.contains(&e.to.0))
            .filter(|e| drawn.insert((e.from.0, e.to.0, e.rel_type.as_str())))
            .map(|e| {
                json!({
                    "from": e.from.0,
                    "to": e.to.0,
                    "rel_type": e.rel_type,
                })
            })
            .collect();

        Ok(json!({
            "nodes": nodes,
            "edges": edges,
            "truncated": total > nodes.len(),
            "hidden": total - nodes.len(),
        }))
    }
}
