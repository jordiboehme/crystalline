use super::*;

impl Engine {
    // --- infer schema --------------------------------------------------------

    /// Infer a Picoschema from a domain's engrams of a type. Engram content is
    /// loaded from disk for a file domain and from the database for a virtual
    /// domain.
    ///
    /// Scoped like [`Engine::validate_engrams`]: the inferred field names are
    /// generalized out of the domain's own engrams, so a domain the caller may
    /// not see is refused as an unregistered one.
    pub async fn infer_schema(
        &self,
        p: &InferParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let hidden = self.hidden_for(scope).await?;
        let source = self.content_source_scoped(&p.domain, &hidden)?;
        let store = self.store.lock().await;
        let descs = store
            .list_engrams(&p.domain, None, Some(&p.engram_type))
            .await?;
        drop(store);

        let mut engrams = Vec::new();
        for d in &descs {
            if let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await {
                engrams.push(engram);
            }
        }
        let threshold = p.threshold.unwrap_or(0.25);
        let schema = schema::infer(&engrams, threshold);
        Ok(json!({
            "domain": p.domain,
            "type": p.engram_type,
            "count": engrams.len(),
            "threshold": threshold,
            "schema": schema,
        }))
    }

    // --- vocabulary ----------------------------------------------------------

    /// List the tags, observation categories, relation types and engram `type`
    /// and `status` values already in use, each with a usage count, for one
    /// domain or across every domain. An unknown domain reports empty lists
    /// rather than erroring, matching the store contract, so an agent can probe a
    /// fresh domain safely. `domain` echoes the request, `null` for an all-domain
    /// sweep.
    ///
    /// Scoped: a named domain the caller may not see reports the same empty
    /// lists an unknown one does, and an all-domain sweep covers the domains the
    /// caller may read. Tag names and their counts are content, so a sweep that
    /// summed a private domain into its totals would publish that domain's
    /// vocabulary to everyone who asked for the whole picture.
    ///
    /// The sweep is one query per visible domain, merged by
    /// [`crystalline_index::merge_vocabularies`], and only when something is
    /// hidden - the store's own sweep is all-domains or one domain, with no
    /// domain list to hand it. Every unscoped caller keeps the single query.
    pub async fn vocabulary(
        &self,
        p: &VocabularyParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let hidden = self.hidden_for(scope).await?;
        let vocab = self.scoped_vocabulary(p.domain.as_deref(), &hidden).await?;
        // Every count list is present unconditionally, empty when nothing is in
        // use, so a client reads a list rather than testing for a missing key.
        // Only the two advisory keys below (clusters, aliases) are omitted when
        // they have nothing to say.
        let mut out = json!({
            "domain": p.domain,
            "tags": vocab.tags,
            "categories": vocab.categories,
            "relation_types": vocab.relation_types,
            "types": vocab.types,
            "statuses": vocab.statuses,
        });
        // Near-duplicate tag clusters, omitted entirely when there are none so a
        // clean vocabulary stays quiet. They point at tags to consolidate with
        // `crystalline tags merge`. Declared aliases are folded out first, so a
        // cluster an alias already explains is never reported.
        let clusters = crystalline_index::tag_clusters_with_aliases(&vocab.tags, &vocab.aliases);
        if !clusters.is_empty()
            && let Value::Object(map) = &mut out
        {
            map.insert(
                "clusters".to_string(),
                serde_json::to_value(&clusters).unwrap_or(Value::Null),
            );
        }
        // The tag aliases in effect, omitted when there are none. They tell an
        // agent which spellings fold onto which canonical tag.
        if !vocab.aliases.is_empty()
            && let Value::Object(map) = &mut out
        {
            map.insert(
                "aliases".to_string(),
                serde_json::to_value(&vocab.aliases).unwrap_or(Value::Null),
            );
        }
        Ok(out)
    }
}
