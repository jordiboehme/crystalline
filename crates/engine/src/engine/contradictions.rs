//! The contradiction check on the engine side: which model the configured
//! profile runs, the daemon pass that scores related pairs, the lazy scorer
//! and its idle drop, the pending cache and the worker.
//!
//! A pass has two halves. The first needs no model: per domain it reads the
//! stamps, and a domain whose digest is unchanged since a walk that left it at
//! zero is skipped without parsing. Otherwise it assembles the base facts (the
//! listing, each engram's parsed frontmatter and observations, the stamps and
//! the lead vectors), finds the related pairs and diffs them against the
//! stored pair rows. The second half loads the model only when something is
//! pending and scores at most [`MAX_INFERENCES_PER_PASS`] line pairs, each
//! engram pair whole or not at all.
//!
//! Nothing here holds the store lock across a model load or a scoring batch,
//! and every batch runs on a blocking thread of its own: the daemon's
//! shutdown takes the store lock as its last step, and a pass that held it
//! through a download would keep the process alive until the watchdog.

use super::*;
use crystalline_index::ScoredPair;
use crystalline_index::nli::{
    CandidateFacts, CandidatePair, ContradictionScorer, MAX_INFERENCES_PER_PASS, NLI_BATCH_SIZE,
    NliModel, NliProfile, contradiction_candidates, eligible, line_pairs, max_related_pairs,
    nli_model, pending_pairs, related_threshold, score_rows, scorer_inputs,
};

/// The key whose change lifts a failed load and makes pending unknown again.
const CONTRADICTIONS_KEY: &str = "evolve.contradictions";

/// What the contradiction check reads about one base engram, owned. Built by
/// [`Engine::contradiction_facts`] for both the daemon pass and the sweep.
pub(crate) struct ContradictionFact {
    /// The engram's id.
    pub(crate) id: EngramId,
    /// The effective status, trimmed and lowercased, so the candidate rule
    /// reads a hand-written `Stable` as current and `Draft` as speculative.
    pub(crate) status: String,
    /// Start of the validity window; absent is unbounded.
    pub(crate) valid_from: Option<NaiveDate>,
    /// End of the validity window; absent is unbounded.
    pub(crate) valid_to: Option<NaiveDate>,
    /// The content checksum `file_stamps` reports for the engram's path.
    pub(crate) checksum: String,
    /// The lead vector for the active embedding model, when asked for and
    /// stored.
    pub(crate) lead_vector: Option<Vec<f32>>,
    /// The observation bullets, category and tags stripped.
    pub(crate) observations: Vec<FactObservation>,
}

impl ContradictionFact {
    /// The borrowed view the candidate rules take.
    pub(crate) fn view(&self) -> CandidateFacts<'_> {
        CandidateFacts {
            id: self.id,
            status: &self.status,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
            checksum: &self.checksum,
            lead_vector: self.lead_vector.as_deref(),
            observations: &self.observations,
        }
    }
}

/// One domain's share of a walk.
struct DomainWork {
    name: String,
    id: DomainId,
    facts: Vec<ContradictionFact>,
    pending: Vec<CandidatePair>,
    settle: SettledDomain,
}

impl Engine {
    /// The NLI model the configured `evolve.contradictions` profile runs.
    /// `None` for `off`, and for a hand-edited value this build does not know,
    /// which reads as off rather than as a guess at a model.
    pub fn contradiction_model(&self) -> Option<&'static NliModel> {
        let cfg = self.config.read().unwrap();
        NliProfile::from_setting(cfg.evolve_contradictions()).map(nli_model)
    }

    /// Called by `configure` after every set or unset: a change of
    /// `evolve.contradictions` lifts a failed load (the one way, besides a
    /// daemon start, to ask for that model again) and makes pending unknown,
    /// so the next tick asks for a pass under the new profile.
    pub(crate) fn contradiction_setting_touched(&self, key: &str) {
        if key != CONTRADICTIONS_KEY {
            return;
        }
        let mut state = self.contradiction_state.lock().unwrap();
        state.pending = None;
        state.load_failed = None;
        state.last_error = None;
        state.error_logged = false;
    }

    /// Ask the contradiction worker for a pass. `false` when no worker is
    /// wired, which is everywhere but the daemon: standalone, nothing scores.
    pub fn request_contradictions(&self) -> bool {
        match &self.contradiction_tx {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// Whether a contradiction pass is running right now.
    pub fn contradictions_in_flight(&self) -> bool {
        self.contradiction_gate.lock().unwrap().running
    }

    /// Whether the tick should ask for a pass: the check is on, nothing is
    /// scoring, the model's last load did not fail, and pending is unknown (a
    /// fresh start, a changed setting) or non-zero (a pass the budget cut
    /// short, a batch that failed).
    pub fn contradictions_wanted(&self) -> bool {
        let Some(model) = self.contradiction_model() else {
            return false;
        };
        if self.contradictions_in_flight() {
            return false;
        }
        let state = self.contradiction_state.lock().unwrap();
        if state.load_failed == Some(model.repo) {
            return false;
        }
        match &state.pending {
            None => true,
            Some(pending) => pending.values().sum::<usize>() > 0,
        }
    }

    /// Whether an NLI model is in memory.
    pub fn contradiction_scorer_loaded(&self) -> bool {
        self.scorer.lock().unwrap().is_some()
    }

    /// Why the model could not be loaded or a batch could not be scored, as
    /// the pass last saw it. Test-only; status reads the state directly.
    #[cfg(any(test, feature = "testing"))]
    pub fn contradiction_last_error(&self) -> Option<String> {
        self.contradiction_state.lock().unwrap().last_error.clone()
    }

    /// How many times [`Engine::contradiction_facts`] has parsed a domain
    /// since this engine was built. The seam exists because a settled domain
    /// is skipped and a skip is invisible in every outcome.
    #[cfg(any(test, feature = "testing"))]
    pub fn contradiction_fact_walks(&self) -> u64 {
        self.contradiction_fact_walks
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Drop the loaded model once it has sat unused for [`NLI_IDLE_DROP`], or
    /// at once when the check was turned off or moved to another profile.
    pub fn drop_idle_scorer(&self) {
        self.drop_idle_scorer_at(tokio::time::Instant::now());
    }

    /// [`Engine::drop_idle_scorer`] as of `now`, so a test can step the clock.
    pub fn drop_idle_scorer_at(&self, now: tokio::time::Instant) {
        let wanted = self.contradiction_model().map(|m| m.repo);
        let in_flight = self.contradictions_in_flight();
        let mut held = self.scorer.lock().unwrap();
        let idle = match held.as_ref() {
            None => false,
            Some(h) => {
                Some(h.repo) != wanted
                    || (!in_flight && now.saturating_duration_since(h.last_used) >= NLI_IDLE_DROP)
            }
        };
        if idle {
            *held = None;
            tracing::debug!("dropped the idle contradiction model");
        }
    }

    /// Score related pairs for possible contradictions, one walk over every
    /// domain in scope and another for each request that arrived meanwhile.
    pub async fn score_contradictions(&self) -> Result<ContradictionOutcome> {
        let Some(model) = self.contradiction_model() else {
            *self.contradiction_state.lock().unwrap() = ContradictionState::default();
            *self.scorer.lock().unwrap() = None;
            return Ok(ContradictionOutcome::Off);
        };
        if self.load_blocked(model) {
            return Ok(ContradictionOutcome::ModelUnavailable);
        }
        let Some(mut pass) = ContradictionPass::claim(&self.contradiction_gate) else {
            tracing::debug!("a contradiction pass is already running; it walks again");
            return Ok(ContradictionOutcome::AlreadyRunning);
        };
        let mut outcome = ContradictionOutcome::Scored {
            pairs: 0,
            line_pairs: 0,
            remaining: 0,
        };
        loop {
            let walk = self.contradiction_walk(model).await?;
            outcome = match (outcome, walk) {
                (
                    ContradictionOutcome::Scored {
                        pairs, line_pairs, ..
                    },
                    ContradictionOutcome::Scored {
                        pairs: p,
                        line_pairs: l,
                        remaining,
                    },
                ) => ContradictionOutcome::Scored {
                    pairs: pairs + p,
                    line_pairs: line_pairs + l,
                    remaining,
                },
                (_, other) => other,
            };
            if outcome == ContradictionOutcome::ModelUnavailable {
                // Another walk would ask the loader again, which is exactly
                // what a failed load must not do; release the claim.
                while pass.walk_again() {}
                break;
            }
            if !pass.walk_again() {
                break;
            }
        }
        Ok(outcome)
    }

    /// Whether `model`'s last load failed and nothing has lifted it since.
    /// A failure recorded for another model is forgotten here: the profile
    /// moved, which is a setting change.
    fn load_blocked(&self, model: &'static NliModel) -> bool {
        let mut state = self.contradiction_state.lock().unwrap();
        match state.load_failed {
            Some(repo) if repo == model.repo => true,
            Some(_) => {
                state.load_failed = None;
                state.last_error = None;
                state.error_logged = false;
                false
            }
            None => false,
        }
    }

    async fn contradiction_walk(&self, model: &'static NliModel) -> Result<ContradictionOutcome> {
        let work = self.contradiction_work(model).await?;
        let mut pending: BTreeMap<String, usize> = work
            .iter()
            .map(|w| (w.name.clone(), w.pending.len()))
            .collect();
        if pending.values().sum::<usize>() == 0 {
            let mut state = self.contradiction_state.lock().unwrap();
            for w in &work {
                state.settled.insert(w.name.clone(), w.settle.clone());
            }
            state.pending = Some(pending);
            state.last_error = None;
            state.error_logged = false;
            return Ok(ContradictionOutcome::Scored {
                pairs: 0,
                line_pairs: 0,
                remaining: 0,
            });
        }
        let scorer = match self.scorer_for(model).await {
            Ok(scorer) => scorer,
            Err(e) => {
                let mut state = self.contradiction_state.lock().unwrap();
                if !state.error_logged {
                    tracing::warn!(
                        model = model.repo,
                        "the contradiction model could not be loaded; it is tried again once evolve.contradictions is set again or the daemon starts again: {e}"
                    );
                    state.error_logged = true;
                }
                state.last_error = Some(e.to_string());
                state.load_failed = Some(model.repo);
                state.pending = Some(pending);
                return Ok(ContradictionOutcome::ModelUnavailable);
            }
        };
        let _activity = ActivityState::begin(&self.activity, "contradictions", None);
        let started = std::time::Instant::now();
        let mut budget = MAX_INFERENCES_PER_PASS;
        let (mut pairs_done, mut lines_done, mut batches) = (0usize, 0usize, 0usize);
        let mut batch_error: Option<String> = None;
        'domains: for w in &work {
            for pair in &w.pending {
                let (a, b) = (&w.facts[pair.a], &w.facts[pair.b]);
                let lines = line_pairs(&a.observations, &b.observations);
                if lines.len() > budget {
                    break 'domains;
                }
                let inputs = scorer_inputs(&lines);
                let expected = inputs.len();
                // One failing batch skips its pair, which stays pending and
                // unstored, and the pass goes on with the others: a pair is
                // stored whole or not at all.
                let probabilities = match run_scorer(Arc::clone(&scorer), inputs).await {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            a = a.id.0,
                            b = b.id.0,
                            "skipping a pair the contradiction scorer failed on: {e}"
                        );
                        batch_error = Some(e.to_string());
                        continue;
                    }
                };
                batches += expected.div_ceil(NLI_BATCH_SIZE);
                let rows = score_rows(a.id, b.id, &lines, &probabilities);
                let scored = ScoredPair {
                    a: a.id,
                    b: b.id,
                    checksum_a: a.checksum.clone(),
                    checksum_b: b.checksum.clone(),
                };
                {
                    let store = self.store.lock().await;
                    store
                        .replace_contradictions(
                            w.id,
                            &scored,
                            pair.cosine,
                            model.repo,
                            &Utc::now().to_rfc3339(),
                            &rows,
                        )
                        .await?;
                }
                budget -= lines.len();
                pairs_done += 1;
                lines_done += lines.len();
                if let Some(left) = pending.get_mut(&w.name) {
                    *left -= 1;
                }
            }
        }
        if let Some(held) = self.scorer.lock().unwrap().as_mut() {
            held.last_used = tokio::time::Instant::now();
        }
        if pairs_done > 0 {
            tracing::info!(
                model = model.id,
                pairs = pairs_done,
                line_pairs = lines_done,
                batches,
                ms = started.elapsed().as_millis() as u64,
                "scored related pairs for possible contradictions"
            );
        }
        let remaining = pending.values().sum();
        let mut state = self.contradiction_state.lock().unwrap();
        for w in &work {
            if pending.get(&w.name) == Some(&0) {
                state.settled.insert(w.name.clone(), w.settle.clone());
            } else {
                state.settled.remove(&w.name);
            }
        }
        state.pending = Some(pending);
        state.last_error = batch_error;
        state.error_logged = false;
        Ok(ContradictionOutcome::Scored {
            pairs: pairs_done,
            line_pairs: lines_done,
            remaining,
        })
    }

    /// The first half of a walk: every domain in scope with its pending pairs.
    async fn contradiction_work(&self, model: &'static NliModel) -> Result<Vec<DomainWork>> {
        let (scope, coverage) = {
            let store = self.store.lock().await;
            let scope = self.embed_scope(&*store).await?;
            // The cached coverage snapshot, one cheap read per walk.
            let coverage = store
                .embedding_coverage()
                .await?
                .embedded_for(&self.model_id);
            (scope, coverage)
        };
        let mut names = self.known_domain_names();
        names.sort();
        names.dedup();
        let (threshold, max_pairs) = (related_threshold(), max_related_pairs());
        let mut out = Vec::new();
        for name in names {
            let Ok(source) = self.content_source(&name) else {
                continue;
            };
            let (domain_id, stamps) = {
                let store = self.store.lock().await;
                let Some(domain_id) = store.domain_id(&name).await? else {
                    continue;
                };
                (domain_id, store.file_stamps(domain_id).await?)
            };
            if scope.as_ref().is_some_and(|ids| !ids.contains(&domain_id)) {
                continue;
            }
            let digest = walk_digest(model, threshold, &self.model_id, &stamps);
            let settled = self
                .contradiction_state
                .lock()
                .unwrap()
                .settled
                .get(&name)
                .filter(|s| s.digest == digest && s.coverage.is_none_or(|c| c == coverage))
                .cloned();
            if let Some(settle) = settled {
                out.push(DomainWork {
                    name,
                    id: domain_id,
                    facts: Vec::new(),
                    pending: Vec::new(),
                    settle,
                });
                continue;
            }
            let facts = self
                .contradiction_facts(&source, &name, domain_id, &stamps, true)
                .await?;
            // Coverage below 100 percent never holds a pair back: an engram
            // with a lead vector is a candidate whatever else is unembedded.
            // It only decides whether a later embedding can add a pair
            // without moving a stamp, which is when the count joins the
            // settled record.
            let waiting = facts.iter().any(|f| {
                f.lead_vector.is_none() && eligible(&f.status) && !f.observations.is_empty()
            });
            let settle = SettledDomain {
                digest,
                coverage: waiting.then_some(coverage),
            };
            let scored = {
                let store = self.store.lock().await;
                store
                    .contradiction_pairs_scored(domain_id, model.repo)
                    .await?
            };
            let pending = {
                let views: Vec<CandidateFacts<'_>> =
                    facts.iter().map(ContradictionFact::view).collect();
                let found = contradiction_candidates(&views, threshold, max_pairs);
                if found.capped {
                    tracing::info!(
                        domain = %name,
                        compared = found.compared,
                        "contradiction candidates skipped: over the lead-vector cap"
                    );
                }
                pending_pairs(&views, &found.pairs, &scored)
            };
            out.push(DomainWork {
                name,
                id: domain_id,
                facts,
                pending,
                settle,
            });
        }
        Ok(out)
    }

    /// The contradiction check's facts for one domain's base engrams: the
    /// listing, each engram parsed through `source` (frontmatter status and
    /// window, observations), its checksum from `stamps`, and, when
    /// `with_lead_vectors`, its lead vector for the active embedding model.
    /// The daemon pass and the sweep's read share it, so both see one
    /// candidate set. An engram with no stamp or that no longer parses is
    /// left out, as the sweep leaves it out.
    ///
    /// The lead-vector fetch is the whole domain's, unbounded (see
    /// [`Store::lead_vectors`]); a caller that only needs statuses, windows,
    /// checksums and lines passes `false` and pays nothing for it.
    pub(crate) async fn contradiction_facts(
        &self,
        source: &ContentSource,
        domain: &str,
        domain_id: DomainId,
        stamps: &HashMap<String, FileStamp>,
        with_lead_vectors: bool,
    ) -> Result<Vec<ContradictionFact>> {
        #[cfg(any(test, feature = "testing"))]
        self.contradiction_fact_walks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (descs, mut vectors) = {
            let store = self.store.lock().await;
            let descs = store.list_engrams(domain, None, None).await?;
            let vectors: HashMap<i64, Vec<f32>> = if with_lead_vectors {
                store
                    .lead_vectors(domain_id, &self.model_id, None)
                    .await?
                    .into_iter()
                    .map(|lv| (lv.engram_id.0, lv.vector))
                    .collect()
            } else {
                HashMap::new()
            };
            (descs, vectors)
        };
        let mut facts = Vec::with_capacity(descs.len());
        for d in &descs {
            let Some(stamp) = stamps.get(&d.path) else {
                continue;
            };
            let Some(engram) = self.load_engram(source, domain_id, &d.path).await else {
                continue;
            };
            let fm = &engram.frontmatter;
            let status = fm
                .status
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| d.status.trim())
                .to_lowercase();
            facts.push(ContradictionFact {
                id: d.id,
                status,
                valid_from: fm.valid_from,
                valid_to: fm.valid_to,
                checksum: stamp.sha256.clone(),
                lead_vector: vectors.remove(&d.id.0),
                observations: engram
                    .observations
                    .iter()
                    .map(|o| FactObservation {
                        line: o.line,
                        text: o.content.clone(),
                    })
                    .collect(),
            });
        }
        Ok(facts)
    }

    /// The scorer for `model`, loaded on first use. Another profile's model is
    /// dropped before the new one loads, so two never sit in memory together.
    async fn scorer_for(&self, model: &'static NliModel) -> Result<Arc<dyn ContradictionScorer>> {
        {
            let mut held = self.scorer.lock().unwrap();
            if let Some(h) = held.as_mut()
                && h.repo == model.repo
            {
                h.last_used = tokio::time::Instant::now();
                return Ok(Arc::clone(&h.scorer));
            }
            *held = None;
        }
        let loaded = (self.scorer_loader)(model).await?;
        *self.scorer.lock().unwrap() = Some(LoadedScorer {
            repo: model.repo,
            scorer: Arc::clone(&loaded),
            last_used: tokio::time::Instant::now(),
        });
        Ok(loaded)
    }
}

/// Score `inputs` in batches of [`NLI_BATCH_SIZE`], each on a blocking thread
/// of its own, so no blocking unit outlasts one batch. A batch that fails, or
/// answers with the wrong number of scores, fails the call.
async fn run_scorer(
    scorer: Arc<dyn ContradictionScorer>,
    inputs: Vec<(String, String)>,
) -> Result<Vec<f32>> {
    let mut out = Vec::with_capacity(inputs.len());
    for batch in inputs.chunks(NLI_BATCH_SIZE) {
        let (scorer, batch) = (Arc::clone(&scorer), batch.to_vec());
        let expected = batch.len();
        let scores = tokio::task::spawn_blocking(move || scorer.score(&batch))
            .await
            .map_err(|e| EngineError::Internal(format!("contradiction scoring task failed: {e}")))?
            .map_err(EngineError::from)?;
        if scores.len() != expected {
            return Err(EngineError::Internal(format!(
                "the contradiction scorer returned {} scores for {expected} inputs",
                scores.len()
            )));
        }
        out.extend(scores);
    }
    Ok(out)
}

/// What a domain looked like to a walk: the NLI model, the related line, the
/// embedding model and every path with its checksum. Built from the stamps
/// alone, never from the vectors, so checking it costs one narrow read.
fn walk_digest(
    model: &NliModel,
    threshold: f64,
    embedding_model: &str,
    stamps: &HashMap<String, FileStamp>,
) -> String {
    let mut h = Sha256::new();
    h.update(model.repo.as_bytes());
    h.update([0]);
    h.update(threshold.to_bits().to_le_bytes());
    h.update(embedding_model.as_bytes());
    h.update([0]);
    let mut paths: Vec<(&String, &FileStamp)> = stamps.iter().collect();
    paths.sort_by(|a, b| a.0.cmp(b.0));
    for (path, stamp) in paths {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(stamp.sha256.as_bytes());
        h.update([0]);
    }
    crystalline_index::hex_lower(&h.finalize())
}

/// Runs contradiction passes on demand, one per burst of requests. A pass
/// that scored something and left pairs pending asks for the next one at
/// once; a failed load asks for nothing.
pub async fn run_contradiction_worker(
    engine: Arc<Engine>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    while rx.recv().await.is_some() {
        while rx.try_recv().is_ok() {}
        match engine.score_contradictions().await {
            Ok(ContradictionOutcome::Scored {
                pairs, remaining, ..
            }) if pairs > 0 && remaining > 0 => {
                engine.request_contradictions();
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("background contradiction scoring failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(sha: &str) -> FileStamp {
        FileStamp {
            mtime: 1,
            size: 1,
            sha256: sha.to_string(),
        }
    }

    #[test]
    fn the_walk_digest_moves_with_a_stamp_the_model_or_the_embedding_model_only() {
        let full = nli_model(NliProfile::Full);
        let light = nli_model(NliProfile::Light);
        let stamps: HashMap<String, FileStamp> = [
            ("a.md".to_string(), stamp("1")),
            ("b.md".to_string(), stamp("2")),
        ]
        .into();
        let base = walk_digest(full, 0.8, "granite", &stamps);
        assert_eq!(base, walk_digest(full, 0.8, "granite", &stamps.clone()));
        let mut edited = stamps.clone();
        edited.insert("b.md".to_string(), stamp("3"));
        assert_ne!(base, walk_digest(full, 0.8, "granite", &edited));
        let mut touched = stamps.clone();
        touched.get_mut("a.md").unwrap().mtime = 99;
        assert_eq!(
            base,
            walk_digest(full, 0.8, "granite", &touched),
            "a touch that keeps the content keeps the digest"
        );
        assert_ne!(base, walk_digest(light, 0.8, "granite", &stamps));
        assert_ne!(base, walk_digest(full, 0.7, "granite", &stamps));
        assert_ne!(base, walk_digest(full, 0.8, "bge", &stamps));
    }
}
