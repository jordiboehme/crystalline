use super::*;

impl Engine {
    // --- evolve --------------------------------------------------------------

    /// Run the consolidation sweep over a scope and return one page of its
    /// ranked queue, recording that the sweep ran.
    ///
    /// The thin half of the seam: [`Engine::evolve_detect`] does the work and
    /// this adds the one side effect, stamping the sweep into the maintenance
    /// state so the Stop hook stops nudging about domains this sweep just
    /// looked at - the swept scope for a scoped call, the whole backlog for an
    /// unscoped one. Detection is shared and pure, so a surface that
    /// only wants to show the queue (the REST queue view) calls `evolve_detect`
    /// and never counts as a run; an agent that actually works the queue comes
    /// through here.
    ///
    /// The recording is best effort by design - see [`crate::maintenance`] -
    /// and the response is returned exactly as detection built it.
    pub async fn evolve_engrams(
        &self,
        p: &EvolveParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let value = self.evolve_detect(p, scope).await?;
        // The swept scope is read back out of the response rather than
        // re-derived from the parameters: an unscoped call defaults to every
        // registered domain, and only the response knows which those were.
        let swept: Vec<String> = value["scope"]["domains"]
            .as_array()
            .map(|domains| {
                domains
                    .iter()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // A sweep with no scope of its own looked at every registered domain,
        // so it settles the whole backlog rather than subtracting the names it
        // saw. That is what heals the state file: a domain a human wrote to and
        // then unregistered can never appear in a swept scope again, and
        // subtracting would leave it pending for ever with the Stop hook naming
        // a ghost nothing can act on. A scoped call keeps the exact opposite
        // property, settling its own domains and leaving the rest of the
        // backlog standing at its original age.
        if p.domains.is_empty() {
            crate::maintenance::record_run_unscoped();
        } else {
            crate::maintenance::record_run(&swept);
        }
        Ok(value)
    }

    /// The detection half of the consolidation sweep: one page of the ranked
    /// queue over a scope, with no side effect of any kind.
    ///
    /// Read-only end to end: it resolves the scope, assembles the facts every
    /// detector reads, runs [`crystalline_index::detect`] once per domain and
    /// shapes the merged result. Nothing is written and nothing is remembered,
    /// so "what is left" is re-derived by calling again with the same scope.
    ///
    /// Seven details of the assembly are load-bearing, each guarding a class of
    /// silently wrong finding:
    ///
    /// - the resolved degrees are counted over the **merged** graph slices, so
    ///   chunking the `neighbors` seed list never turns a linked engram into a
    ///   `V104` orphan. The merge dedupes edges on the same key the backends
    ///   use, because an edge whose ends land in two different chunks comes
    ///   back from both calls;
    /// - `stale_on` and `verified_on` come from the [`Frontmatter`] accessors,
    ///   never the raw keys, so the legacy `review_after` and `last_verified`
    ///   spellings fold in exactly as they do for search and verify;
    /// - the token budget is resolved the way verify's `Q002` resolves it - a
    ///   per-file override, then the domain default, then 2500 - so `V105` and
    ///   `Q002` never disagree about what oversized means;
    /// - `status` and `engram_type` arrive lowercased, because the status sets
    ///   the rules test against are exact matches;
    /// - `known_domains` is every registered domain, so `V102` can tell an
    ///   unregistered target domain apart from a target that does not exist,
    ///   and the graph is taken at depth 1 so cross-domain targets carry a
    ///   status for `V101` to read;
    /// - the attachment facts (`analyzes`, `analyzed_hash`, `asset_refs`) are
    ///   read off the **parsed engram** [`Engine::load_engram`] returns - the
    ///   file for a file domain, the stored source for a virtual one - and
    ///   never off the index's `content` column, which for a file domain holds
    ///   the body alone. A claim lives in the frontmatter, so counting it off
    ///   the index would make every file domain look as if it claimed nothing
    ///   and would report claimed attachments as orphans. This is the same
    ///   split [`Engine::peer_engram_text`] makes for the move's referent
    ///   count, and the two agree on what a reference is: an `assets/` link in
    ///   the body or the `analyzes` key, compared as exact paths;
    /// - the lead vectors reach `V301` only when a provider is installed, so
    ///   meaning is compared on exactly the machines that compute it. A machine
    ///   with an index full of embeddings and no provider says nothing about
    ///   meaning rather than scoring against whatever an older model left
    ///   behind.
    pub async fn evolve_detect(
        &self,
        p: &EvolveParams,
        scope: &crate::scope::Scope,
    ) -> Result<Value> {
        let hidden = self.hidden_for(scope).await?;
        let today = match p.today.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                EngineError::Invalid(format!("today '{s}' is not an ISO date (YYYY-MM-DD)"))
            })?,
            None => Utc::now().date_naive(),
        };
        let families = parse_families(&p.families)?;
        let rules = parse_rules(&p.rules)?;

        // Every registered domain this caller may see, both as the default
        // scope and as `V102`'s idea of which `[[domain:Target]]` prefixes name
        // a real domain. Filtered on both counts deliberately: a finding names
        // the domain, permalink and file path it fired on, and `V102`'s verdict
        // on a cross-domain target is itself an answer about whether that
        // domain exists.
        let mut known_domains = self.known_domain_names();
        known_domains.retain(|name| !hidden.contains(name));
        known_domains.sort();
        known_domains.dedup();

        let mut swept_scope: Vec<String> = Vec::new();
        if p.domains.is_empty() {
            swept_scope = known_domains.clone();
        } else {
            for name in &p.domains {
                // The same resolution every other tool uses, so an unknown name
                // errors identically and a domain registered after startup is
                // still found - and a domain this caller may not see is one of
                // the names that errors.
                self.domain_entry_scoped(name, &hidden)?;
                if !swept_scope.contains(name) {
                    swept_scope.push(name.clone());
                }
            }
        }

        let mut findings: Vec<Finding> = Vec::new();
        let mut truncations: Vec<String> = Vec::new();
        let mut engrams_scanned = 0usize;
        let mut unparsed = 0usize;
        let mut acknowledged = AckCounts::default();

        // One domain at a time: `SweepInput` is domain-scoped (two rules are
        // domain-relative) and processing them in turn bounds the memory an
        // unscoped sweep needs to whatever the largest domain costs.
        for name in &swept_scope {
            let Some(swept) = self
                .sweep_domain(name, today, &known_domains, p.include_acknowledged, scope)
                .await?
            else {
                continue;
            };
            engrams_scanned += swept.report.engrams_scanned;
            unparsed += swept.unparsed;
            // A cap that fired is domain-local, so the merged list names the
            // domain it fired in.
            truncations.extend(
                swept
                    .report
                    .truncations
                    .iter()
                    .map(|t| format!("{name} - {t}")),
            );
            // Counted before the family and rule filters below, because what an
            // acknowledgment suppressed is a fact about the domain rather than
            // about the slice of it this call asked for.
            acknowledged.total += swept.report.acknowledged.total;
            acknowledged.temporal += swept.report.acknowledged.temporal;
            acknowledged.structure += swept.report.acknowledged.structure;
            acknowledged.redundancy += swept.report.acknowledged.redundancy;
            findings.extend(swept.report.findings);
        }

        findings.retain(|f| {
            (families.is_empty() || families.contains(&f.family))
                && (rules.is_empty() || rules.contains(&f.rule))
                && p.min_priority.is_none_or(|min| f.priority >= min)
        });
        // Re-ranked after the merge: each domain's report is ranked on its own,
        // and the sort is total and deterministic, so consecutive pages of an
        // unscoped sweep stay coherent.
        rank(&mut findings);

        let total = findings.len();
        let limit = p
            .limit
            .unwrap_or(EVOLVE_DEFAULT_LIMIT)
            .clamp(1, EVOLVE_MAX_LIMIT);
        let page = p.page.unwrap_or(1).max(1);
        let offset = (page - 1).saturating_mul(limit);
        let shown: &[Finding] = match findings.get(offset..) {
            Some(rest) => &rest[..rest.len().min(limit)],
            None => &[],
        };

        // Family counts are over the whole filtered result, not the page, so a
        // reader on page 1 sees the shape of everything waiting.
        let family_counts: Vec<Value> = Family::ALL
            .iter()
            .filter_map(|family| {
                let count = findings.iter().filter(|f| f.family == *family).count();
                (count > 0).then(|| json!({ "family": family.as_str(), "findings": count }))
            })
            .collect();

        // Every row flat with scalar-only cells, so the queue renders as one
        // tabular block. `n` is the rank across the whole result, not within the
        // page, so an item keeps its number as the reader pages.
        let queue: Vec<Value> = shown
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut row = json!({
                    "n": offset + i + 1,
                    "priority": f.priority,
                    "rule": f.rule,
                    "class": f.class.as_str(),
                    "domain": f.domain,
                    "permalink": f.permalink,
                    "title": f.title,
                    "line": f.line,
                    "finding": f.finding,
                    "evidence": f.evidence,
                    "fix": f.fix,
                });
                // The pair a twin row is about, which is the value an
                // acknowledgment for it is given for and the value the ack
                // route takes back. Only a pair-scoped rule carries it: it is
                // the one rule that fires more than once on an engram, so it
                // is the one whose rows a caller has to be able to tell apart.
                // Every other rule's acknowledgment is named by the engram and
                // the rule alone, and a column repeating what those two fields
                // already say would cost every queue tokens for nothing.
                if crystalline_index::is_pair_scoped(f.rule) && !f.scope.is_empty() {
                    row["scope"] = Value::String(f.scope.clone());
                }
                // The acknowledgment columns ride along only when they say
                // something, so an ordinary queue row stays the flat shape every
                // renderer already knows.
                if f.acknowledged {
                    row["acknowledged"] = Value::Bool(true);
                }
                if f.ack_stale {
                    row["ack_stale"] = Value::Bool(true);
                }
                // The scope the acknowledgment was **given for**, which on a
                // stale row is deliberately not what the finding fires on now:
                // the row's own evidence and fix columns say that, and the pair
                // is what shows a reader why the acknowledgment stopped
                // matching. Named beside `ack_note` rather than sharing the
                // `scope` above it, because the two disagree on exactly the
                // rows that matter - and it is this one a withdrawal names,
                // since it is the entry the file holds.
                if let Some(scope) = f.ack_scope.as_deref().filter(|s| !s.is_empty()) {
                    row["ack_scope"] = Value::String(scope.to_string());
                }
                if let Some(note) = &f.ack_note {
                    row["ack_note"] = Value::String(note.clone());
                }
                row
            })
            .collect();

        // The prose instruction rides a per-rule legend rather than a column, so
        // a page of ten findings from one rule carries it once. Only the rules
        // on this page appear. The catalog's short `summary` rides beside it:
        // a renderer wants a few words for a heading and the instruction for
        // the body, and deriving one from the other is not a client's job.
        let actions: Vec<Value> = RULES
            .iter()
            .filter(|info| shown.iter().any(|f| f.rule == info.id))
            .map(|info| {
                json!({
                    "rule": info.id,
                    "summary": info.summary,
                    "instruction": info.instruction,
                })
            })
            .collect();

        Ok(json!({
            "scope": {
                "domains": swept_scope,
                "families": families.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
                "rules": rules,
                "min_priority": p.min_priority,
                "today": today.to_string(),
            },
            "engrams_scanned": engrams_scanned,
            "unparsed": unparsed,
            "total": total,
            "page": page,
            "limit": limit,
            "count": queue.len(),
            "families": family_counts,
            "acknowledged": {
                "total": acknowledged.total,
                "by_family": {
                    "temporal": acknowledged.temporal,
                    "structure": acknowledged.structure,
                    "redundancy": acknowledged.redundancy,
                },
            },
            "queue": queue,
            "actions": actions,
            "guidance": EVOLVE_GUIDANCE,
            "truncations": truncations,
        }))
    }

    // --- acknowledgments -----------------------------------------------------

    /// What an `evolve_ack` assignment asks for, or `None` when this edit is
    /// not one. Pure and public, so a surface can put the act to a user before
    /// it happens without the engine having to guess at the wording.
    ///
    /// The value is the rule id optionally followed by a note, split at the
    /// first whitespace: `V101` or `V101 lineage citation, keep`. The rule has
    /// to be one the catalog knows, because an acknowledgment of a rule that
    /// does not exist can never suppress anything and silently storing it would
    /// read as work done. `remove V101` takes an entry back instead of
    /// recording one (see [`parse_ack_value`]).
    pub fn ack_intent(p: &EditParams) -> Result<Option<AckIntent>> {
        if p.operation != "set_frontmatter"
            || p.key.as_deref().map(str::trim) != Some(EVOLVE_ACK_KEY)
        {
            return Ok(None);
        }
        let raw = p.value.as_deref().map(str::trim).unwrap_or_default();
        parse_ack_value(raw).map(Some)
    }

    /// What an `evolve_ack` confirmation round names: `{domain, permalink}` for
    /// the engram the assignment would land on.
    ///
    /// Shaped after [`Engine::delete_preview`] and there for the same reason: a
    /// question is only worth putting to a user about a call that can run.
    /// Read-only is checked first and the identifier is resolved next, so a
    /// server that never writes, a domain nobody registered and an identifier
    /// nobody has each fail in round one - rather than collecting a yes and
    /// reporting the miss in round two, against a name the user already
    /// approved.
    ///
    /// It resolves and nothing else. The `expected_checksum` comparison and the
    /// "is that acknowledgment even there" test both read what the file holds,
    /// the file can change between the rounds, and both already run in the
    /// round that writes; repeating them here would buy a guarantee that does
    /// not survive the gap.
    pub async fn ack_preview(&self, p: &EditParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (desc, _) = self.resolve_in(&p.identifier, &p.domain).await?;
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
        }))
    }

    /// The acknowledgment an `evolve_ack` assignment is asking for, completed
    /// with the scope only the sweep can supply, or `None` when this edit is
    /// not one.
    ///
    /// The scope is the firing finding's, and its absence is meaningful: a rule
    /// that is not currently firing for this engram is acknowledged scope-less,
    /// which matches whatever it finds later. That is the honest reading of
    /// "acknowledge this before it appears".
    ///
    /// **A caller that names the pair gets that pair**, which is what
    /// [`EditParams::ack_scope`] carries. It is honoured for a
    /// [pair-scoped](crystalline_index::is_pair_scoped) rule and ignored for
    /// every other, whose acknowledgment answers for the engram and so has
    /// nothing to choose between; and it is checked rather than trusted, by
    /// [`Engine::named_scope`]. So detection still runs on this path - it just
    /// validates a request instead of resolving one.
    ///
    /// A removal needs none of that: it names an entry that is already on the
    /// engram, so it travels to the text edit as the rule id alone and the
    /// filtering happens there, under the lock, against what the file holds.
    pub(super) async fn ack_draft(
        &self,
        p: &EditParams,
        desc: &EngramDescriptor,
        actor: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Option<AckDraft>> {
        Ok(match Self::ack_intent(p)? {
            None => None,
            Some(AckIntent::Remove { rule }) => Some(AckDraft::Remove(rule)),
            Some(AckIntent::Record { rule, note }) => {
                let named = p
                    .ack_scope
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && crystalline_index::is_pair_scoped(&rule));
                let scope = match named {
                    Some(named) => Some(
                        self.named_scope(&desc.domain, &desc.permalink, &rule, named, scope)
                            .await?,
                    ),
                    None => {
                        self.firing_scope(&desc.domain, &desc.permalink, &rule, scope)
                            .await?
                    }
                };
                Some(AckDraft::Record(EvolveAck {
                    scope,
                    rule,
                    note,
                    by: actor.to_string(),
                    at: Some(now_offset()),
                }))
            }
        })
    }

    /// `named` back, once detection confirms `rule` is really firing on
    /// `permalink` for it. The refusal is the point: an acknowledgment is a
    /// record that somebody read a finding and ruled it intentional, so one
    /// given for evidence no sweep can see is a claim about nothing.
    ///
    /// What it protects against is a queue read too long ago. Two twin
    /// findings on one engram differ only by their pair, so a page still
    /// showing yesterday's rows would otherwise silence a pair its reader
    /// never saw, with their note on it - the exact confusion the pair is
    /// carried to prevent.
    ///
    /// Compared as the whole string, because a scope **is** one value: the
    /// sweep sorts and joins its parts before it renders one, so two callers
    /// naming the same pair send the same bytes, and a pair that gained a
    /// member is a different scope rather than a near miss.
    async fn named_scope(
        &self,
        domain: &str,
        permalink: &str,
        rule: &str,
        named: &str,
        scope: &crate::scope::Scope,
    ) -> Result<String> {
        let firing = self.firing_findings(domain, permalink, rule, scope).await?;
        if firing.iter().any(|f| f.scope == named) {
            return Ok(named.to_string());
        }
        Err(EngineError::Invalid(format!(
            "no {rule} finding on '{permalink}' for '{named}'; re-read the queue and acknowledge a row it still shows"
        )))
    }

    /// What `rule` is currently firing on `permalink` for, as the scope an
    /// acknowledgment is matched against later. `None` when the rule is not
    /// firing, or when it fires with an empty scope because its identity is
    /// just the engram and the rule.
    ///
    /// Detection runs with the suppressed findings included, so
    /// re-acknowledging a finding an older entry already silences still sees
    /// the evidence it fires on and records the current scope rather than
    /// dropping to a scope-less entry.
    ///
    /// **The unacknowledged finding wins when the rule fires more than once
    /// here**, which only the pair-scoped rule does
    /// ([`crystalline_index::is_pair_scoped`]): an engram that twins two others
    /// carries two `V301` findings and neither one is "the" finding. Taking
    /// the first row every time made the second acknowledgment re-record the
    /// pair the first already covered, so the other pair could never be
    /// acknowledged at all. With every pair acknowledged the first row wins
    /// again, which is what makes a re-acknowledgment update a note in place.
    async fn firing_scope(
        &self,
        domain: &str,
        permalink: &str,
        rule: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Option<String>> {
        let firing = self.firing_findings(domain, permalink, rule, scope).await?;
        Ok(firing
            .iter()
            .find(|f| !f.acknowledged)
            .or(firing.first())
            .map(|f| f.scope.clone())
            .filter(|scope| !scope.is_empty()))
    }

    /// Every finding `rule` is raising on `permalink` right now, in queue
    /// order and with the suppressed ones included, which is what both readers
    /// need: [`Engine::firing_scope`] to resolve a scope and
    /// [`Engine::named_scope`] to check one. A domain that sweeps to nothing
    /// answers with no findings rather than an error - there is no evidence
    /// there to name.
    async fn firing_findings(
        &self,
        domain: &str,
        permalink: &str,
        rule: &str,
        scope: &crate::scope::Scope,
    ) -> Result<Vec<Finding>> {
        let mut known_domains = self.known_domain_names();
        known_domains.sort();
        known_domains.dedup();
        let today = Utc::now().date_naive();
        let Some(swept) = self
            .sweep_domain(domain, today, &known_domains, true, scope)
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(swept
            .report
            .findings
            .into_iter()
            .filter(|f| f.rule == rule && f.permalink == permalink)
            .collect())
    }

    /// Acknowledge a finding: record on the engram that this rule's finding was
    /// read and ruled intentional, so future sweeps count it rather than
    /// raising it. The Fluid half of the same act
    /// [`Engine::edit_engram_as`]'s `set_frontmatter` performs for an agent,
    /// through the one edit path, with the scope computed the one way.
    ///
    /// The rule is screened here as well as inside the edit, because this
    /// surface carries the rule and the note as separate fields and joins them
    /// into one value: the screen is what keeps a rule field that happens to
    /// read `remove` an unknown-rule refusal rather than a value the parser
    /// would take for a removal.
    ///
    /// **The screen runs before resolution by design, so an unknown rule
    /// answers before a missing engram does**: a request that gets both halves
    /// wrong is refused for the rule (a 422 on the REST surface) rather than
    /// for the permalink (a 404). The rule is the half the caller can fix
    /// without another lookup - the catalog is right there in the message - and
    /// an acknowledgment of a rule nobody has could not be recorded even on an
    /// engram that does exist.
    ///
    /// `scope` names the pair the acknowledgment is for, which only a
    /// [pair-scoped](crystalline_index::is_pair_scoped) rule has more than one
    /// of; it is ignored for every other rule and checked rather than trusted
    /// for that one (see [`Engine::named_scope`]). `None` leaves the choice to
    /// the server, which is what an agent's `set_frontmatter` does.
    ///
    /// `acting` is who is asking, and it is a different thing entirely from
    /// the `scope` above: this recording is an [`Engine::edit_engram_as`] in
    /// the end, and every write verb takes the acting scope (see
    /// [`Engine::write_engram_as`]), so the surface's answer is passed through
    /// rather than a stand-in invented here.
    // Eight, and the eighth is the acting scope every write verb now takes.
    // Bundling the six the caller supplies into a struct would put a type
    // between the REST handler and its one call for no reader's benefit.
    #[allow(clippy::too_many_arguments)]
    pub async fn acknowledge_finding_as(
        &self,
        domain: &str,
        identifier: &str,
        rule: &str,
        note: Option<&str>,
        scope: Option<&str>,
        client: Option<&str>,
        acting: &crate::scope::Scope,
    ) -> Result<Value> {
        let screened = rule.trim().to_ascii_uppercase();
        if rule_info(&screened).is_none() {
            return Err(EngineError::Invalid(unknown_rule_message(&screened)));
        }
        let value = match note.map(str::trim).filter(|n| !n.is_empty()) {
            Some(note) => format!("{} {note}", rule.trim()),
            None => rule.trim().to_string(),
        };
        let params = EditParams {
            identifier: identifier.to_string(),
            domain: domain.to_string(),
            operation: "set_frontmatter".to_string(),
            key: Some(EVOLVE_ACK_KEY.to_string()),
            value: Some(value),
            ack_scope: scope.map(str::to_string),
            ..EditParams::default()
        };
        let result = self.edit_engram_as(&params, client, acting).await?;
        Ok(result.get("evolve_ack").cloned().unwrap_or(Value::Null))
    }

    /// Withdraw an acknowledgment, leaving the engram's other entries alone.
    /// `false` when the engram carries none this names, which the surface
    /// answers as a 404 rather than pretending a removal happened.
    ///
    /// `scope` narrows the withdrawal to the one entry given for that pair,
    /// and like the recording half it speaks only for a
    /// [pair-scoped](crystalline_index::is_pair_scoped) rule: every other rule
    /// keeps one entry, so there is nothing to narrow. `None` takes every
    /// entry the rule has, which is the whole of it for the ten and all pairs
    /// at once for the one.
    ///
    /// Fluid's half of the take-back an agent asks for with the `remove
    /// <rule-id>` value form; both filter through [`without_ack`]. They differ
    /// in how an entry that is not there is reported, and in that the agent's
    /// form names a rule and never a pair.
    pub async fn unacknowledge_finding_as(
        &self,
        domain: &str,
        identifier: &str,
        rule: &str,
        scope: Option<&str>,
        client: Option<&str>,
        acting: &crate::scope::Scope,
    ) -> Result<bool> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let rule = rule.trim().to_ascii_uppercase();
        if rule_info(&rule).is_none() {
            return Err(EngineError::Invalid(unknown_rule_message(&rule)));
        }
        let view = DomainView::for_write(self, domain, acting).await?;
        let overlay = view.actor();
        let actor = self.actor_for(client, overlay);
        let scope = scope
            .map(str::trim)
            .filter(|s| !s.is_empty() && crystalline_index::is_pair_scoped(&rule));
        let (desc, source) = view.resolve(identifier).await?;
        // Checked before the write so an engram carrying no such entry answers
        // "nothing to withdraw" without a rewrite, a reindex or a touched
        // generated block. Read through the overlay, so an acknowledgment a
        // draft carries is what a withdrawal in review mode looks at.
        let current = match overlay {
            Some(_) => view.text_at(&source, &desc).await?.ok_or_else(|| {
                EngineError::NotFound(format!("no engram '{identifier}' in domain '{domain}'"))
            })?,
            None => self.load_source(&source, &desc).await?,
        };
        if !has_ack(&current, &rule, scope) {
            return Ok(false);
        }
        // The answer here is a bool, so a mirror warning has nowhere to ride
        // out; `write_overlay_entry` has already logged it.
        self.apply_source_edit(&desc, &source, &view, None, &actor, None, |current| {
            Ok(without_ack(current, &rule, scope))
        })
        .await?;
        Ok(true)
    }

    /// An engram's markdown as its domain holds it, whichever kind that is.
    async fn load_source(&self, source: &ContentSource, desc: &EngramDescriptor) -> Result<String> {
        match source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                store
                    .engram_content(desc.domain_id, &desc.path)
                    .await?
                    .ok_or_else(|| {
                        EngineError::NotFound(format!(
                            "no content stored for '{}' in domain '{}'",
                            desc.permalink, desc.domain
                        ))
                    })
            }
        }
    }

    /// One domain's assembled facts, detected: the sweep's whole per-domain
    /// half, shared by [`Engine::evolve_detect`] and the acknowledgment write
    /// path, which needs the same verdict about one engram before it can record
    /// what a finding was acknowledged for.
    ///
    /// **Run in the caller's own dimension**, which is what makes a draft's
    /// findings its author's: on a domain that reviews changes, the listing,
    /// the text behind each fact, the graph the degrees are counted over, the
    /// lead vectors and the dangling references are all taken as that actor
    /// reads them, and the acknowledgments a fact carries are the ones written
    /// into the document they are reading. Base findings stay everybody's,
    /// because a base row nobody has redrafted is in everybody's listing.
    /// Outside review mode - and for a caller with no identity - every one of
    /// those is the base answer, unchanged.
    ///
    /// What is deliberately NOT per actor is the run recorder in
    /// [`Engine::evolve_engrams`]: a sweep having run is a fact about the
    /// machine's maintenance backlog, whoever asked for it.
    ///
    /// `Ok(None)` for a domain with no engrams: no domain row to query against
    /// and nothing to detect. An empty domain is quiet, not an error.
    async fn sweep_domain(
        &self,
        name: &str,
        today: NaiveDate,
        known_domains: &[String],
        include_acknowledged: bool,
        scope: &crate::scope::Scope,
    ) -> Result<Option<DomainSweep>> {
        let mut unparsed = 0usize;
        let source = self.content_source(name)?;
        // One view, built once and asked five times below - the listing, the
        // entries, the base text behind them, the draft references and the
        // actor key the lead vectors are read in - where each of those used to
        // derive the same actor for itself. The caller has already screened the
        // domain (`evolve_detect` resolves every name through
        // `domain_entry_scoped` before it gets here), so this pass screens
        // nothing further.
        let view = DomainView::for_read(self, name, &HashSet::new(), scope)?;
        let overlay = view.actor();
        let store = self.store.lock().await;
        let base = store.list_engrams(name, None, None).await?;
        drop(store);
        // The sweep looks at exactly what its caller reads, through the same
        // helper every read verb shadows a listing with: a path this caller is
        // drafting is their own row, a path they have deleted is absent, and a
        // draft at a path no file holds is a row like any other. `None` - a
        // domain that takes changes directly, or a caller with no identity -
        // hands the base listing straight back, so a sweep outside review mode
        // is byte for byte the one that was there before.
        //
        // Shadowed rather than additive, and that is the whole ruling: a
        // finding about a base row its author has already redrafted is a
        // finding about text they no longer see. The lock is dropped first
        // because the helper takes it itself.
        let descs = view.list_over(base).await?;
        // No engrams means no domain row to query against and nothing to
        // detect. An empty domain is quiet, not an error. Read off the listing
        // rather than the registration, so a domain whose only rows are one
        // actor's drafts still sweeps for that actor.
        let Some(domain_id) = descs.first().map(|d| d.domain_id) else {
            return Ok(None);
        };

        let held = view.entries(domain_id).await?;
        let drafts = DomainView::held_text(&held);
        let drafts = &drafts;
        // What the domain's own rows reference at the paths this caller's view
        // replaced or removed, which is the other half of the union `V108` asks
        // its question of. Read from the base text, never from the draft.
        let shadowed_asset_refs = view.shadowed_asset_refs(domain_id, &held).await;

        // Traversed in the caller's dimension too, or every draft would come
        // back with no edges at all and `V104` would report the engrams
        // somebody is working on hardest as orphans.
        let graph = self.sweep_graph(&descs, overlay).await?;
        let mut inbound: HashMap<i64, usize> = HashMap::new();
        let mut outbound: HashMap<i64, usize> = HashMap::new();
        for edge in &graph.edges {
            *outbound.entry(edge.from.0).or_default() += 1;
            *inbound.entry(edge.to.0).or_default() += 1;
        }

        // Read before the store lock is taken: the provider lives behind its
        // own guard and the sweep has no reason to hold both.
        let embedded = self.provider().is_some();

        // One query in this caller's own dimension: the base rows their drafts
        // do not shadow and their own drafts together, each judged against what
        // they hold at the target's address. It replaced a pass that assembled
        // the draft half in Rust beside this one - which could not see the
        // colon-prefixed title form and could not be asked about a base row at
        // all, so a base link a reader's own draft had already answered was
        // still raised at them.
        let unresolved = view.unresolved(domain_id).await?;
        // The `vocabulary` TOOL stays shared and the V203 FINDING moved, which
        // is the half of the Task 9 ruling that held and the half that did not.
        // What a person is shown is still the domain's agreement - a word one
        // author is trying out in a draft is not the team's vocabulary - but a
        // drift finding is about what THAT author wrote, and reading it off the
        // team's list told them their own new word was already established, or
        // said nothing at all about the one beside it.
        let vocab = view.vocabulary().await?;
        // The attachment set in the caller's own dimension, like every other
        // input on this path. A file this caller drafted is one they can see, so
        // `V107` must not call their own reference to it dangling; a file they
        // have deleted reads absent for them. On a domain that takes changes
        // directly the view has no actor and this is the base listing, byte for
        // byte the query that was here before.
        //
        // **This is also `V108`'s input**, so the orphan rule reads the actor
        // view from here on: a file only this actor holds is theirs to be
        // orphaned or referenced. The union rule that keeps a reference dropped
        // in a draft from orphaning a shared file is `shadowed_asset_refs`'
        // above and is untouched by this.
        let attachments = view.attachments().await?;
        // Asked before the store lock, like the two above it: the view takes
        // the lock itself.
        let store = self.store.lock().await;
        // Lead vectors for V301, only with a provider installed: without one
        // the rule stays silent whatever a previous run left embedded, so a
        // sweep on a machine that never embeds never speaks about meaning.
        //
        // Asked in the caller's own dimension. On a domain that does not review
        // changes this is `None` and the answer is the base rows, byte for byte
        // what it always was. On one that does, a path this caller is drafting
        // contributes THEIR row's vector rather than the reviewed file's - so
        // `V301` never tells an author their own rewrite is a twin of the
        // version they are rewriting, and never speaks about a version they are
        // not reading. The listing above is shadowed with the same rows, so the
        // vector and the fact it attaches to are one engram's.
        let mut lead_vectors: HashMap<i64, Vec<f32>> = if embedded {
            store
                .lead_vectors(domain_id, &self.model_id, overlay)
                .await?
                .into_iter()
                .map(|lv| (lv.engram_id.0, lv.vector))
                .collect()
        } else {
            HashMap::new()
        };
        drop(store);

        let verify_config = domain_verify_config(&source);
        let mut facts: Vec<EngramFacts> = Vec::with_capacity(descs.len());
        for d in &descs {
            // This caller's own draft of the path when they hold one, and
            // otherwise files-are-truth for a file domain, the stored content
            // for a virtual one. Assembling the reviewed file's text under a
            // draft's row would be the quietest way to get this wrong: every
            // rule would then speak about text its author is not reading.
            //
            // An engram that no longer parses is counted and skipped rather
            // than failing the whole sweep, since one broken file must not hide
            // every finding behind it.
            let held = drafts.get(&d.path);
            let parsed = match held {
                Some(text) => parse_engram(text).ok(),
                None => self.load_engram(&source, d.domain_id, &d.path).await,
            };
            let Some(engram) = parsed else {
                unparsed += 1;
                continue;
            };
            let fm = &engram.frontmatter;
            let status = match fm
                .status
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(s) => s.to_ascii_lowercase(),
                None => d.status.trim().to_ascii_lowercase(),
            };
            let title = if fm.title.trim().is_empty() {
                d.title.clone()
            } else {
                fm.title.clone()
            };
            let tokens = engram.body.chars().count() / 4;
            facts.push(EngramFacts {
                id: d.id,
                domain: d.domain.clone(),
                permalink: d.permalink.clone(),
                title,
                path: d.path.clone(),
                // Which dimension this fact came out of: the empty string for a
                // row the domain's files or its database hold, and the caller's
                // own actor key for their draft standing at that path. What
                // `V301`'s path skip reads the dimension out of.
                actor: match held {
                    Some(_) => overlay.unwrap_or_default().to_string(),
                    None => String::new(),
                },
                status,
                engram_type: fm.engram_type.trim().to_ascii_lowercase(),
                tags: fm.tags.clone(),
                salience: yaml_number(fm.extra.get("salience")),
                recorded_at: fm.recorded_at,
                valid_from: fm.valid_from,
                valid_to: fm.valid_to,
                stale_on: fm.stale_on(),
                verified_on: fm.latest_verified().map(|v| v.at.date_naive()),
                tokens,
                token_budget: resolve_token_budget(verify_config.as_ref(), &d.path),
                inbound: inbound.get(&d.id.0).copied().unwrap_or(0),
                outbound: outbound.get(&d.id.0).copied().unwrap_or(0),
                generated_by: fm.generated.as_ref().map(|g| g.by.clone()),
                analyzes: asset_claim(fm),
                analyzed_hash: fm
                    .extra
                    .get("analyzed_hash")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .map(str::to_string),
                asset_refs: crystalline_core::find_asset_refs(&engram.body),
                acks: ack_entries(fm),
                // Filled in by the caller that has the store: the sweep is
                // pure, so the lead embeddings are handed to it, never fetched
                // from inside it. Removed rather than cloned - one engram is
                // assembled once, and the vector is the largest field here.
                lead_vector: lead_vectors.remove(&d.id.0),
                // The parser's own bullets, so `V010` compares what an
                // observation asserts rather than re-deriving it from the body.
                observations: engram
                    .observations
                    .iter()
                    .map(|o| FactObservation {
                        line: o.line,
                        text: o.content.clone(),
                    })
                    .collect(),
                body: engram.body,
            });
        }

        let input = SweepInput {
            domain: name.to_string(),
            today,
            engrams: facts,
            graph,
            unresolved,
            tags: vocab.tags,
            tag_aliases: vocab.aliases,
            known_domains: known_domains.to_vec(),
            attachments,
            shadowed_asset_refs,
            share: self.share_facts(name).await,
            include_acknowledged,
            // The sweep module's own constants, never literals repeated here:
            // the thresholds and the twin caps are one place, and nothing
            // configures them yet. The twin caps hold an invariant the defaults
            // satisfy and a future settings surface has to keep - the pairs
            // retained bound the findings emitted, so `max_twin_pairs` stays
            // above `max_twin_findings` or the cap on findings is unreachable.
            options: SweepOptions::default(),
        };
        let report = detect(&input);
        Ok(Some(DomainSweep { report, unparsed }))
    }

    /// What a domain owes its team origin, for `V009`, or `None` for a domain
    /// with no origin, no recorded origin state or no readable working tree.
    ///
    /// Offline by construction: [`crate::origin::unshared_work`] walks the tree
    /// against the base snapshot and never probes the forge, so a sweep costs
    /// the same whether the machine is connected or on a train. The rule reads
    /// substantive changes only, which is the filter that helper applies.
    ///
    /// Under the domain's [`Engine::origin_lock`], like every other origin
    /// read: the tree and `state.json` are compared against each other, and a
    /// share or a pull rewrites both. Unlocked, a sweep landing mid-share reads
    /// one of them from before the write and the other from after, and answers
    /// a count for a delta that never existed.
    pub(super) async fn share_facts(&self, name: &str) -> Option<ShareFacts> {
        let lock = self.origin_lock(name);
        let _guard = lock.lock().await;
        let (_spec, root, state_dir) = self.origin_spec_for_domain(name).ok()?;
        let work = origin::unshared_work(&root, &state_dir)?;
        Some(ShareFacts {
            unshared: work.count(),
            oldest_change: work.oldest_change_date(),
        })
    }

    /// How many substantive changes one team domain holds that the team has
    /// not seen, for the share ask a write receipt carries
    /// ([`crate::nudge::write_verb_trailer`]).
    ///
    /// [`Engine::share_facts`] narrowed to its count, which is what keeps the
    /// receipt's answer and the sweep's `V009` one reading: the same offline
    /// walk ([`crate::origin::unshared_work`]) and the same
    /// substantive-changes-only filter.
    ///
    /// Two things separate it from the sweep's version, and both are about
    /// where it runs: on the path of a write that has already succeeded.
    ///
    /// **It never waits for the origin lock.** That lock is held across the
    /// network by a pull, a share and a connect, and the poller takes it on a
    /// timer, so waiting for it here would hold a receipt until somebody else's
    /// forge call came back. A domain whose origin is mid-operation therefore
    /// contributes nothing, on the same terms as a domain whose tree cannot be
    /// walked: nothing is KNOWN to be unshared, and a delta that cannot be read
    /// is no reason to speak. Taking the lock at all is what keeps the walk off
    /// a half-written pair (see [`Engine::share_facts`]).
    ///
    /// **And it never runs on a runtime thread.** The walk reads and hashes
    /// every file in the domain root, which is blocking I/O measured in
    /// hundreds of milliseconds on a large or networked tree; run inline it
    /// would occupy a tokio worker and, with the guard held across it, queue a
    /// real share or the poller's pull behind a question about one sentence.
    /// The owned guard travels into the blocking task and is released with it,
    /// so the lock is held for exactly the walk and not a moment of scheduling
    /// either side of it.
    ///
    /// The caller memoizes this per domain
    /// ([`crate::nudge`]), so a machine whose team domains are fully
    /// shared pays for the walk once a minute rather than once a write.
    ///
    /// `None` for a domain with no origin, no recorded origin state, no
    /// readable working tree, an origin operation in flight, or a blocking task
    /// that panicked.
    pub(crate) async fn unshared_change_count(&self, name: &str) -> Option<u64> {
        let guard = self.origin_lock(name).try_lock_owned().ok()?;
        let (_spec, root, state_dir) = self.origin_spec_for_domain(name).ok()?;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            origin::unshared_work(&root, &state_dir).map(|work| work.count() as u64)
        })
        .await
        .ok()?
    }

    /// The resolved graph around a whole domain, at depth 1 so every
    /// cross-domain target carries a status.
    ///
    /// In one caller's dimension, the same one their engram list was shadowed
    /// in: `None` for a domain that takes changes directly, and that actor's
    /// key for one that reviews them. The two have to agree or the degrees are
    /// counted against nodes that are not in the fact list - a draft seeded
    /// into a base-only traversal comes back with no edges at all, which reads
    /// as an orphan on exactly the engram somebody is working on.
    async fn sweep_graph(
        &self,
        descs: &[EngramDescriptor],
        actor: Option<&str>,
    ) -> Result<GraphSlice> {
        let ids: Vec<EngramId> = descs.iter().map(|d| d.id).collect();
        self.sweep_neighbors(&ids, 1, actor).await
    }

    /// [`Store::neighbors`] over a seed list of any size, merged into one slice.
    ///
    /// The seed list is chunked because both backends inline it into an SQL
    /// `IN (...)`, so a whole domain in one call would build a statement
    /// proportional to its size. Merging is exact rather than approximate at
    /// every depth the callers use: the traversal collects an edge whenever one
    /// of its ends lies within `depth - 1` hops of a seed, and a node whenever it
    /// lies within `depth` hops, and hop distance from the whole seed set is the
    /// smallest hop distance from any one chunk. The union over the chunks is
    /// therefore exactly what one unchunked call would have returned.
    ///
    /// Nodes and edges are deduped on the merge: a node reached from two chunks,
    /// and an edge whose two ends sit in different chunks, both come back more
    /// than once, and a double-counted edge would inflate the degrees the
    /// consolidation ranking and the orphan rule read. The merged nodes are
    /// sorted by id, so a chunked sweep answers in the same ascending order a
    /// single-chunk one does and every caller's ordering holds either way.
    pub(super) async fn sweep_neighbors(
        &self,
        ids: &[EngramId],
        depth: u8,
        actor: Option<&str>,
    ) -> Result<GraphSlice> {
        let mut graph = GraphSlice::default();
        let mut seen_nodes: HashSet<i64> = HashSet::new();
        let mut seen_edges: HashSet<(i64, i64, String, u8)> = HashSet::new();
        for chunk in ids.chunks(NEIGHBOR_CHUNK) {
            let store = self.store.lock().await;
            let slice = store.neighbors(chunk, depth, actor).await?;
            drop(store);
            for node in slice.nodes {
                if seen_nodes.insert(node.id.0) {
                    graph.nodes.push(node);
                }
            }
            for edge in slice.edges {
                let kind = match edge.kind {
                    EdgeKind::Relation => 0u8,
                    EdgeKind::Link => 1u8,
                };
                if seen_edges.insert((edge.from.0, edge.to.0, edge.rel_type.clone(), kind)) {
                    graph.edges.push(edge);
                }
            }
        }
        graph.nodes.sort_by_key(|n| n.id.0);
        Ok(graph)
    }
}
