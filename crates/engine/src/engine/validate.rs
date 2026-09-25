use super::*;

impl Engine {
    // --- validate ------------------------------------------------------------

    /// Validate a domain's engrams against its schema engrams. Engram content is
    /// loaded from disk for a file domain and from the database for a virtual
    /// domain, so validation covers both kinds.
    ///
    /// A domain `scope` may not see is refused as an unregistered one before
    /// anything is listed: the report names permalinks, paths and per-engram
    /// messages, which is a reading of the domain's contents by another route.
    pub async fn validate_engrams(
        &self,
        p: &ValidateParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let hidden = self.hidden_for(scope).await?;
        let source = self.content_source_scoped(&p.domain, &hidden)?;
        let store = self.store.lock().await;
        let schema_descs = store.list_engrams(&p.domain, None, Some("schema")).await?;
        let targets = if let Some(id) = &p.identifier {
            match store.find_engram(&p.domain, id).await? {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{id}' in domain '{}'",
                        p.domain
                    )));
                }
            }
        } else {
            store
                .list_engrams(&p.domain, None, p.engram_type.as_deref())
                .await?
        };
        drop(store);

        let mut schemas: Vec<Schema> = Vec::new();
        for d in &schema_descs {
            if let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await
                && let Some(schema) = Schema::from_engram(&engram)
            {
                schemas.push(schema);
            }
        }

        let mut issues = Vec::new();
        let mut checked = 0usize;
        // When drift is requested, target engrams are grouped by their selected
        // schema so `schema::diff` runs once per schema over its own group.
        let mut drift_groups: Vec<(Schema, Vec<Engram>)> = Vec::new();
        for d in &targets {
            let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await else {
                continue;
            };
            checked += 1;
            let selected = schema::select_schema(&engram, &schemas);
            if let Some(schema) = &selected {
                for issue in schema::validate(&engram, schema) {
                    issues.push(json!({
                        "permalink": d.permalink,
                        "path": d.path,
                        "severity": issue.severity,
                        "kind": issue.kind,
                        "field": issue.field,
                        "message": issue.message,
                        "line": issue.line,
                    }));
                }
            }
            for issue in crystalline_core::verify::check_temporal(Path::new(&d.path), &engram) {
                let message = match issue.fix {
                    Some(fix) => format!("{} (fix: {fix})", issue.message),
                    None => issue.message,
                };
                issues.push(json!({
                    "permalink": d.permalink,
                    "path": d.path,
                    "severity": issue.severity,
                    "kind": issue.rule,
                    "field": Value::Null,
                    "message": message,
                    "line": issue.line,
                }));
            }
            if p.drift
                && let Some(schema) = selected
            {
                match drift_groups.iter_mut().find(|(s, _)| *s == schema) {
                    Some((_, group)) => group.push(engram),
                    None => drift_groups.push((schema, vec![engram])),
                }
            }
        }

        // The domain's own verify settings, checked the way `crystalline
        // verify` checks them: a severity word it does not know, or a file
        // that does not parse, is one M108 warning each. Only on a
        // whole-domain run, since the file is about the domain and not about
        // any one engram.
        if p.identifier.is_none()
            && let ContentSource::File { root } = &source
        {
            for problem in crystalline_core::verify::load_domain_config(root).problems {
                let message = match &problem.fix {
                    Some(fix) => format!("{} (fix: {fix})", problem.message),
                    None => problem.message,
                };
                issues.push(json!({
                    "permalink": Value::Null,
                    "path": crystalline_core::verify::DOMAIN_CONFIG_FILE,
                    "severity": crystalline_core::Severity::Warning,
                    "kind": "M108",
                    "field": Value::Null,
                    "message": message,
                    "line": Value::Null,
                }));
            }
        }

        let mut response = json!({
            "domain": p.domain,
            "checked": checked,
            "schemas": schemas.len(),
            "issue_count": issues.len(),
            "issues": issues,
        });
        if p.drift {
            let drift: Vec<Value> = drift_groups
                .iter()
                .map(|(schema, engrams)| {
                    let d = schema::diff(schema, engrams);
                    json!({
                        "schema": schema.entity,
                        "undeclared_observations": d.undeclared_observations,
                        "undeclared_relations": d.undeclared_relations,
                        "unused_observations": d.unused_observations,
                        "unused_relations": d.unused_relations,
                    })
                })
                .collect();
            response["drift"] = Value::Array(drift);
        }
        Ok(response)
    }
}
