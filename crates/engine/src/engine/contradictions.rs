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
use crystalline_index::nli::{
    CandidateFacts, CandidatePair, ContradictionScorer, MAX_INFERENCES_PER_PASS, NLI_BATCH_SIZE,
    NliModel, NliProfile, contradiction_candidates, eligible, line_pairs, max_related_pairs,
    nli_model, observations_digest, pending_pairs, related_threshold, score_rows, scorer_inputs,
};
use crystalline_index::{ContradictionRow, ScoredPair};

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
    /// The observation digest ([`observations_digest`]): what the model
    /// reads of the engram, which is what a scored pair and a parked failure
    /// are keyed by. The file stamps only decide whether a domain is parsed
    /// again ([`walk_digest`]).
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

/// What [`Engine::sweep_contradictions`] hands the sweep for one domain.
/// Everything empty, zero and `false` when the check is off.
#[derive(Default)]
pub(crate) struct SweepContradictions {
    /// The configured model, `None` when the check is off.
    pub(crate) model: Option<&'static NliModel>,
    /// The stored rows for that model at or above the store floor.
    pub(crate) rows: Vec<ContradictionRow>,
    /// No walk has counted the domain as it stands now, so the counts below
    /// are unknown rather than zero.
    pub(crate) uncounted: bool,
    /// The configured model's last load failed and nothing has lifted it,
    /// so the daemon neither counts nor scores until the setting is set
    /// again or it restarts: the truncation lines must not promise a pass.
    pub(crate) model_unavailable: bool,
    /// This daemon serves read-only, so the setting cannot be set here and a
    /// restart is the one way to ask for an unavailable model again.
    pub(crate) read_only: bool,
    /// Related pairs with no scored row at their current checksums.
    pub(crate) pending: usize,
    /// The related pairs reached the per-domain cap.
    pub(crate) capped: bool,
    /// Current engrams with observations and no lead vector yet.
    pub(crate) unembedded: usize,
    /// The lead vectors met when the scope is over the vector cap.
    pub(crate) vectors_capped: Option<usize>,
}

/// One domain's share of a walk.
struct DomainWork {
    name: String,
    id: DomainId,
    facts: Vec<ContradictionFact>,
    /// The pairs this walk scores.
    pending: Vec<CandidatePair>,
    /// Known failures this walk leaves alone (every walk but a retry walk).
    known_failing: usize,
    /// The known failures still pending at their checksums, as the walk
    /// started.
    failures: HashSet<FailedPair>,
    settle: SettledDomain,
    /// What this walk counted for the sweep, its `pending` filled in when the
    /// walk publishes. `None` for a domain skipped as settled, whose earlier
    /// record still holds.
    count: Option<DomainCount>,
}

impl Engine {
    /// The NLI model the configured `evolve.contradictions` profile runs.
    /// `None` for `off`, and for a hand-edited value this build does not know,
    /// which reads as off rather than as a guess at a model.
    pub fn contradiction_model(&self) -> Option<&'static NliModel> {
        let cfg = self.config.read().unwrap();
        NliProfile::from_setting(cfg.evolve_contradictions()).map(nli_model)
    }

    /// The `contradictions` block of `ctl status`: the configured profile,
    /// its model's short id (`null` when off), the pairs left unscored after
    /// the last pass (`null` until one ran - never `0`, which would read as
    /// "nothing left" rather than "not counted yet", lesson 37/62), the
    /// subset of those that are known-failing and parked, the pairs scored
    /// for the model across the index, why the model could not be loaded, and
    /// the flags status and doctor use to pick which reason and remedy to
    /// name (L1): `load_failed` (the model itself never loaded, as against a
    /// batch that failed while scoring), `load_retry` (that failure was a
    /// download, which the daemon tries again on its own), `read_only` (the
    /// setting cannot be set here, so only a restart retries a blocked load)
    /// and `embedding_pending` (a settled domain still has a possible
    /// candidate with no lead vector yet, so its pending count of zero is not
    /// the whole story). `device` is where the loaded model runs, in the
    /// embeddings line's words (`metal`, `cpu (fallback: ...)`), and `null`
    /// while no model is in memory (never loaded, dropped after its idle
    /// time, or a direct read with no worker): no guess at a device nothing
    /// runs on.
    pub async fn contradictions_status(&self) -> Result<Value> {
        let profile = self
            .config
            .read()
            .unwrap()
            .evolve_contradictions()
            .to_string();
        let model = self.contradiction_model();
        let scored = match model {
            Some(m) => {
                let store = self.store.lock().await;
                store.scored_pair_count(m.repo).await?
            }
            None => 0,
        };
        let state = self.contradiction_state.lock().unwrap();
        let pending_pairs =
            model.and_then(|_| state.pending.as_ref().map(|p| p.values().sum::<usize>()));
        let failing_pairs =
            model.and_then(|m| state.pending.as_ref().map(|_| failed_count(&state, m.repo)));
        let load_failed = model.is_some_and(|m| state.load_failed == Some(m.repo));
        let load_retry = load_failed && state.load_retry_at.is_some();
        let embedding_pending = state.settled.values().any(|s| s.coverage.is_some());
        let last_error = state.last_error.clone();
        drop(state);
        let device = model.and_then(|m| {
            self.scorer
                .lock()
                .unwrap()
                .as_ref()
                .filter(|h| h.repo == m.repo)
                .and_then(|h| h.scorer.device())
                .map(|d| d.to_string())
        });
        Ok(json!({
            "profile": profile,
            "model": model.map(|m| m.id),
            "pending_pairs": pending_pairs,
            "failing_pairs": failing_pairs,
            "scored_pairs": scored,
            "last_error": last_error,
            "load_failed": load_failed,
            "load_retry": load_retry,
            "read_only": self.read_only,
            "embedding_pending": embedding_pending,
            "device": device,
        }))
    }

    /// Called by `configure` after every set or unset: a change of
    /// `evolve.contradictions` lifts a failed load (the one way, besides a
    /// daemon start, to ask for that model again), forgets the failed pairs
    /// and makes pending unknown, so the next tick asks for a pass under the
    /// new profile. The generation moves too, so a walk still running under
    /// the old profile publishes nothing.
    pub(crate) fn contradiction_setting_touched(&self, key: &str) {
        if key != CONTRADICTIONS_KEY {
            return;
        }
        self.reset_contradiction_state();
    }

    /// Forget what the pass remembers about the domain `name`: called when
    /// it is unregistered or collected, which clears its rows. Without this a
    /// domain added back under the same name from the same files would match
    /// its settled digest and its sweep count, and read as clean over an
    /// empty table. Pending becomes unknown, so the tick asks for a pass, and
    /// the generation moves, so a walk that started before the removal
    /// publishes nothing about it.
    pub(crate) fn forget_contradiction_domain(&self, name: &str) {
        let mut state = self.contradiction_state.lock().unwrap();
        state.settled.remove(name);
        state.counted.remove(name);
        state.failed.remove(name);
        state.pending = None;
        state.generation = state.generation.wrapping_add(1);
    }

    /// Forget everything the pass remembers and move the generation on.
    fn reset_contradiction_state(&self) {
        let mut state = self.contradiction_state.lock().unwrap();
        let generation = state.generation.wrapping_add(1);
        *state = ContradictionState {
            generation,
            ..ContradictionState::default()
        };
    }

    /// Ask the contradiction worker for a pass. `false` when no worker is
    /// wired, which is everywhere but the daemon: standalone, nothing scores.
    pub fn request_contradictions(&self) -> bool {
        match &self.contradiction_tx {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    /// The tick's once-per-tick mark: the next walk retries the pairs a failed
    /// batch or store write left. Marked only while there are such pairs and
    /// the model is loaded anyway, so a failing pair never loads a model by
    /// itself. Returns whether it marked.
    pub fn mark_contradiction_retry(&self) -> bool {
        let Some(model) = self.contradiction_model() else {
            return false;
        };
        if !self.contradiction_scorer_loaded() {
            return false;
        }
        let mut state = self.contradiction_state.lock().unwrap();
        let any = failed_count(&state, model.repo) > 0;
        if any {
            state.retry_due = true;
        }
        any
    }

    /// Whether a contradiction pass is running right now.
    pub fn contradictions_in_flight(&self) -> bool {
        self.contradiction_gate.lock().unwrap().running
    }

    /// Whether the tick should ask for a pass: the check is on, nothing is
    /// scoring, the model's last load did not fail (or failed to download and
    /// its wait is over), and pending is unknown (a
    /// fresh start, a changed setting), holds a pair no batch has failed on
    /// (a pass the budget cut short), or holds a failed pair while the model
    /// is loaded (the tick's retry).
    pub fn contradictions_wanted(&self) -> bool {
        let Some(model) = self.contradiction_model() else {
            return false;
        };
        if self.contradictions_in_flight() {
            return false;
        }
        let loaded = self.contradiction_scorer_loaded();
        let state = self.contradiction_state.lock().unwrap();
        if state.load_failed == Some(model.repo) {
            // A failed download asks again once its wait is over; a build
            // error never does.
            return load_retry_due(&state, tokio::time::Instant::now());
        }
        match &state.pending {
            None => true,
            Some(pending) => {
                let total = pending.values().sum::<usize>();
                let failing = failed_count(&state, model.repo);
                total > failing || (failing > 0 && loaded)
            }
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

    /// How many walks the contradiction pass has made since this engine was
    /// built, parsed or not: a worker that keeps asking shows here.
    #[cfg(any(test, feature = "testing"))]
    pub fn contradiction_walks(&self) -> u64 {
        self.contradiction_walks
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
    /// Each walk reads the profile afresh, so a changed setting takes effect
    /// on the next walk and off ends the pass.
    pub async fn score_contradictions(&self) -> Result<ContradictionOutcome> {
        let Some(first) = self.contradiction_model() else {
            self.reset_contradiction_state();
            *self.scorer.lock().unwrap() = None;
            return Ok(ContradictionOutcome::Off);
        };
        if self.load_blocked(first) {
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
            let Some(model) = self.contradiction_model() else {
                // Turned off while the pass ran: silent at once.
                *self.scorer.lock().unwrap() = None;
                while pass.walk_again() {}
                break;
            };
            let walk = if self.load_blocked(model) {
                ContradictionOutcome::ModelUnavailable
            } else {
                self.contradiction_walk(model).await?
            };
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
    /// moved, which is a setting change. A failed download whose wait is over
    /// is lifted here too, so this walk tries the load once more; the reason
    /// and the backoff stay until a load succeeds.
    fn load_blocked(&self, model: &'static NliModel) -> bool {
        let mut state = self.contradiction_state.lock().unwrap();
        match state.load_failed {
            Some(repo) if repo == model.repo => {
                if load_retry_due(&state, tokio::time::Instant::now()) {
                    state.load_failed = None;
                    state.load_retry_at = None;
                    false
                } else {
                    true
                }
            }
            Some(_) => {
                state.load_failed = None;
                state.load_retry_at = None;
                state.load_backoff = None;
                state.last_error = None;
                state.error_logged = false;
                false
            }
            None => false,
        }
    }

    async fn contradiction_walk(&self, model: &'static NliModel) -> Result<ContradictionOutcome> {
        #[cfg(any(test, feature = "testing"))]
        self.contradiction_walks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (generation, retry) = {
            let mut state = self.contradiction_state.lock().unwrap();
            // A mark is taken either way, and honoured only while the model
            // is still loaded: a mark that outlived an idle drop must not
            // load the model for a pair that is known to fail.
            let due = std::mem::take(&mut state.retry_due);
            (state.generation, due)
        };
        let retry = retry && self.contradiction_scorer_loaded();
        let work = self.contradiction_work(model, retry).await?;
        let mut pending: BTreeMap<String, usize> = work
            .iter()
            .map(|w| (w.name.clone(), w.pending.len() + w.known_failing))
            .collect();
        let mut failures: HashMap<String, HashSet<FailedPair>> = work
            .iter()
            .map(|w| (w.name.clone(), w.failures.clone()))
            .collect();
        if work.iter().all(|w| w.pending.is_empty()) {
            let remaining = pending.values().sum();
            self.publish_walk(model, generation, &work, pending, failures, None);
            return Ok(ContradictionOutcome::Scored {
                pairs: 0,
                line_pairs: 0,
                remaining,
            });
        }
        let scorer = match self.scorer_for(model).await {
            Ok(scorer) => {
                let mut state = self.contradiction_state.lock().unwrap();
                state.load_backoff = None;
                state.load_retry_at = None;
                scorer
            }
            Err(e) => {
                let current = self.contradiction_model().map(|m| m.repo);
                let mut state = self.contradiction_state.lock().unwrap();
                if state.generation == generation && current == Some(model.repo) {
                    if matches!(e, IndexError::NliFetch(_)) {
                        // A download that failed is tried again later, each
                        // failure waiting twice as long, up to an hour.
                        let wait = state
                            .load_backoff
                            .map_or(NLI_FETCH_RETRY_FIRST, |w| (w * 2).min(NLI_FETCH_RETRY_MAX));
                        state.load_backoff = Some(wait);
                        state.load_retry_at = Some(tokio::time::Instant::now() + wait);
                        if !state.error_logged {
                            tracing::warn!(
                                model = model.repo,
                                "the contradiction model could not be downloaded; it is tried again in {} minutes: {e}",
                                wait.as_secs() / 60
                            );
                            state.error_logged = true;
                        } else {
                            tracing::info!(
                                model = model.repo,
                                "the contradiction model could still not be downloaded; it is tried again in {} minutes: {e}",
                                wait.as_secs() / 60
                            );
                        }
                    } else {
                        state.load_retry_at = None;
                        if !state.error_logged {
                            tracing::warn!(
                                model = model.repo,
                                "the contradiction model could not be loaded; it is tried again once evolve.contradictions is set again or the daemon starts again: {e}"
                            );
                            state.error_logged = true;
                        }
                    }
                    state.last_error = Some(e.to_string());
                    state.load_failed = Some(model.repo);
                    record_counts(&mut state, &work, &pending);
                    state.pending = Some(pending);
                }
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
                // The profile is read between pairs, so off, or another
                // profile, ends this walk at once and lets go of the model.
                if self.contradiction_model().map(|m| m.repo) != Some(model.repo) {
                    break 'domains;
                }
                let (a, b) = (&w.facts[pair.a], &w.facts[pair.b]);
                let lines = line_pairs(&a.observations, &b.observations);
                if lines.len() > budget {
                    break 'domains;
                }
                let key = failed_key(model, a, b);
                // One failing batch or store write skips its pair, which stays
                // pending and unstored, and the pass goes on with the others:
                // a pair is stored whole or not at all.
                let done = self
                    .score_pair(model, w.id, pair.cosine, a, b, &lines, &scorer)
                    .await;
                let set = failures.entry(w.name.clone()).or_default();
                match done {
                    Ok(n) => {
                        batches += n;
                        set.remove(&key);
                    }
                    Err(e) => {
                        if w.failures.contains(&key) {
                            tracing::debug!(
                                a = a.id.0,
                                b = b.id.0,
                                "a pair the contradiction check failed on failed again: {e}"
                            );
                        } else {
                            tracing::warn!(
                                a = a.id.0,
                                b = b.id.0,
                                "skipping a pair the contradiction check failed on; the tick retries it: {e}"
                            );
                        }
                        batch_error = Some(e.to_string());
                        set.insert(key);
                        continue;
                    }
                }
                budget -= lines.len();
                pairs_done += 1;
                lines_done += lines.len();
                if let Some(left) = pending.get_mut(&w.name) {
                    *left -= 1;
                }
            }
        }
        drop(scorer);
        if self.contradiction_model().is_none() {
            *self.scorer.lock().unwrap() = None;
        }
        if pairs_done > 0 {
            if let Some(held) = self.scorer.lock().unwrap().as_mut() {
                held.last_used = tokio::time::Instant::now();
            }
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
        self.publish_walk(model, generation, &work, pending, failures, batch_error);
        Ok(ContradictionOutcome::Scored {
            pairs: pairs_done,
            line_pairs: lines_done,
            remaining,
        })
    }

    /// Score one pair and store it, returning the batches it took. The store
    /// lock is taken only for the write, never across a batch.
    #[allow(clippy::too_many_arguments)]
    async fn score_pair(
        &self,
        model: &'static NliModel,
        domain: DomainId,
        cosine: f64,
        a: &ContradictionFact,
        b: &ContradictionFact,
        lines: &[crystalline_index::nli::LinePair<'_>],
        scorer: &Arc<dyn ContradictionScorer>,
    ) -> Result<usize> {
        let inputs = scorer_inputs(lines);
        let batches = inputs.len().div_ceil(NLI_BATCH_SIZE);
        let probabilities = run_scorer(Arc::clone(scorer), inputs).await?;
        let rows = score_rows(a.id, b.id, lines, &probabilities);
        let scored = ScoredPair {
            a: a.id,
            b: b.id,
            checksum_a: a.checksum.clone(),
            checksum_b: b.checksum.clone(),
        };
        let store = self.store.lock().await;
        store
            .replace_contradictions(
                domain,
                &scored,
                cosine,
                model.repo,
                &Utc::now().to_rfc3339(),
                &rows,
            )
            .await?;
        Ok(batches)
    }

    /// Record what a walk found, but only when the generation it started under
    /// is still current and the setting still names its model: a walk under
    /// a profile the setting has since left must not overwrite the unknown
    /// pending the change asked for.
    fn publish_walk(
        &self,
        model: &'static NliModel,
        generation: u64,
        work: &[DomainWork],
        pending: BTreeMap<String, usize>,
        mut failures: HashMap<String, HashSet<FailedPair>>,
        batch_error: Option<String>,
    ) -> bool {
        let current = self.contradiction_model().map(|m| m.repo);
        let mut state = self.contradiction_state.lock().unwrap();
        if state.generation != generation || current != Some(model.repo) {
            return false;
        }
        for w in work {
            let failing = failures.remove(&w.name).unwrap_or_default();
            let left = pending.get(&w.name).copied().unwrap_or(0);
            if left == failing.len() {
                state.settled.insert(
                    w.name.clone(),
                    SettledDomain {
                        failing: !failing.is_empty(),
                        ..w.settle.clone()
                    },
                );
            } else {
                state.settled.remove(&w.name);
            }
            if failing.is_empty() {
                state.failed.remove(&w.name);
            } else {
                state.failed.insert(w.name.clone(), failing);
            }
        }
        record_counts(&mut state, work, &pending);
        state.pending = Some(pending);
        match batch_error {
            Some(e) => state.last_error = Some(e),
            None if failed_count(&state, model.repo) == 0 => state.last_error = None,
            None => {}
        }
        state.error_logged = false;
        true
    }

    /// The first half of a walk: every domain in scope with its pending pairs.
    /// Known failures are left alone unless `retry`.
    async fn contradiction_work(
        &self,
        model: &'static NliModel,
        retry: bool,
    ) -> Result<Vec<DomainWork>> {
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
            let digest = walk_digest(model, threshold, &self.model_id, domain_id, &stamps);
            let (settled, known) = {
                let state = self.contradiction_state.lock().unwrap();
                let settled = state
                    .settled
                    .get(&name)
                    .filter(|s| {
                        s.digest == digest
                            && s.coverage.is_none_or(|c| c == coverage)
                            && !(retry && s.failing)
                    })
                    .cloned();
                let known: HashSet<FailedPair> = state
                    .failed
                    .get(&name)
                    .map(|set| {
                        set.iter()
                            .filter(|f| f.repo == model.repo)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                (settled, known)
            };
            if let Some(settle) = settled {
                out.push(DomainWork {
                    name,
                    id: domain_id,
                    facts: Vec::new(),
                    pending: Vec::new(),
                    known_failing: known.len(),
                    failures: known,
                    settle,
                    count: None,
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
            let unembedded = facts
                .iter()
                .filter(|f| {
                    f.lead_vector.is_none() && eligible(&f.status) && !f.observations.is_empty()
                })
                .count();
            let waiting = unembedded > 0;
            let settle = SettledDomain {
                digest,
                coverage: waiting.then_some(coverage),
                failing: false,
            };
            let mut count = DomainCount {
                digest: settle.digest.clone(),
                coverage: settle.coverage,
                pending: 0,
                capped: false,
                vectors_capped: None,
                unembedded,
            };
            let scored = {
                let store = self.store.lock().await;
                store
                    .contradiction_pairs_scored(domain_id, model.repo)
                    .await?
            };
            // The all-pairs cosine walk and the diff against the stored rows
            // run on a blocking thread: a write to a large domain asks for
            // this walk every time, and on a runtime worker it would hold up
            // every MCP and HTTP request queued behind it. The facts move in
            // and come back.
            let (facts, full, vectors_capped, all) = tokio::task::spawn_blocking(move || {
                let (full, vectors_capped, all) = {
                    let views: Vec<CandidateFacts<'_>> =
                        facts.iter().map(ContradictionFact::view).collect();
                    let found = contradiction_candidates(&views, threshold, max_pairs);
                    let all = pending_pairs(&views, &found.pairs, &scored);
                    (found.full, found.capped.then_some(found.compared), all)
                };
                (facts, full, vectors_capped, all)
            })
            .await
            .map_err(|e| {
                EngineError::Internal(format!("contradiction candidate walk failed: {e}"))
            })?;
            if let Some(compared) = vectors_capped {
                tracing::info!(
                    domain = %name,
                    compared,
                    "contradiction candidates skipped: over the lead-vector cap"
                );
            }
            count.capped = full;
            count.vectors_capped = vectors_capped;
            // A known failure counts only while its pair is still pending at
            // the checksums it failed at; an edit makes it a new pair.
            let key = |p: &CandidatePair| failed_key(model, &facts[p.a], &facts[p.b]);
            let failures: HashSet<FailedPair> =
                all.iter().map(key).filter(|k| known.contains(k)).collect();
            let (pending, known_failing) = if retry {
                (all, 0)
            } else {
                let fresh: Vec<CandidatePair> = all
                    .into_iter()
                    .filter(|p| !failures.contains(&key(p)))
                    .collect();
                (fresh, failures.len())
            };
            out.push(DomainWork {
                name,
                id: domain_id,
                facts,
                pending,
                known_failing,
                failures,
                settle,
                count: Some(count),
            });
        }
        Ok(out)
    }

    /// The contradiction check's facts for one domain's base engrams: the
    /// listing, each engram parsed through `source` (frontmatter status and
    /// window, observations), its observation digest, and, when
    /// `with_lead_vectors`, its lead vector for the active embedding model.
    /// The one assembly of the check: the sweep never builds its own, it
    /// reads the counts the walk kept from this one
    /// ([`Engine::sweep_contradictions`]). An engram with no stamp or that no
    /// longer parses is left out, as the sweep leaves it out.
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
            if !stamps.contains_key(&d.path) {
                continue;
            }
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
            let observations: Vec<FactObservation> = engram
                .observations
                .iter()
                .map(|o| FactObservation {
                    line: o.line,
                    text: o.content.clone(),
                })
                .collect();
            facts.push(ContradictionFact {
                id: d.id,
                status,
                valid_from: fm.valid_from,
                valid_to: fm.valid_to,
                checksum: observations_digest(&observations),
                lead_vector: vectors.remove(&d.id.0),
                observations,
            });
        }
        Ok(facts)
    }

    /// `V302`'s input for one domain's sweep: the stored rows for the
    /// configured model and, when `count_pending`, what the last walk counted
    /// for the domain. Read, never scored: a sweep never runs the model and
    /// never walks the candidates itself.
    ///
    /// An off check reads nothing at all, so it costs the sweep nothing and
    /// says nothing. With the check on, the counts are the pass's own (one
    /// assembly, [`Engine::contradiction_facts`], and one candidate walk),
    /// kept per domain when the walk publishes. They are used only while the
    /// domain still looks the way the walk saw it - the same digest over the
    /// stamps and, when an engram was still waiting for its lead vector, the
    /// same embedding coverage - and otherwise the domain is `uncounted`: a
    /// write since the last walk, a standalone process that never walked, or
    /// a walk that has not run yet reads as not counted, never as zero
    /// (lesson 37). Measured on a 2000-engram domain, walking the candidates
    /// here instead would have cost the sweep about 60 percent more.
    ///
    /// The acknowledgment path, which only needs the findings, passes
    /// `count_pending: false` and reads the rows alone.
    pub(crate) async fn sweep_contradictions(
        &self,
        domain_id: DomainId,
        domain: &str,
        count_pending: bool,
    ) -> Result<SweepContradictions> {
        let Some(model) = self.contradiction_model() else {
            return Ok(SweepContradictions::default());
        };
        let (rows, now) = {
            let store = self.store.lock().await;
            let rows = store
                .contradictions(
                    domain_id,
                    model.repo,
                    crystalline_index::sweep::CONTRADICTION_STORE_FLOOR,
                )
                .await?;
            let now = if count_pending {
                let stamps = store.file_stamps(domain_id).await?;
                let coverage = store
                    .embedding_coverage()
                    .await?
                    .embedded_for(&self.model_id);
                Some((
                    walk_digest(
                        model,
                        related_threshold(),
                        &self.model_id,
                        domain_id,
                        &stamps,
                    ),
                    coverage,
                ))
            } else {
                None
            };
            (rows, now)
        };
        let mut out = SweepContradictions {
            model: Some(model),
            rows,
            ..SweepContradictions::default()
        };
        let Some((digest, coverage)) = now else {
            return Ok(out);
        };
        let state = self.contradiction_state.lock().unwrap();
        match state
            .counted
            .get(domain)
            .filter(|c| c.digest == digest && c.coverage.is_none_or(|c| c == coverage))
        {
            Some(c) => {
                out.pending = c.pending;
                out.capped = c.capped;
                out.vectors_capped = c.vectors_capped;
                out.unembedded = c.unembedded;
            }
            None => out.uncounted = true,
        }
        // Read, never lifted: `load_blocked` is the walk's to clear.
        out.model_unavailable = state.load_failed == Some(model.repo);
        out.read_only = self.read_only;
        Ok(out)
    }

    /// The scorer for `model`, loaded on first use. Another profile's model is
    /// dropped before the new one loads, so two never sit in memory together.
    /// Handing out a loaded scorer does not count as use: only a walk that
    /// scored a pair does, so walks that retry a failing pair and score
    /// nothing let the model idle out.
    async fn scorer_for(
        &self,
        model: &'static NliModel,
    ) -> crystalline_index::Result<Arc<dyn ContradictionScorer>> {
        {
            let mut held = self.scorer.lock().unwrap();
            if let Some(h) = held.as_ref()
                && h.repo == model.repo
            {
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

/// Keep what a walk counted for the sweep: a domain it parsed gets a fresh
/// record, a domain it skipped as settled keeps its record with the pending
/// count moved to what is left. Called only under the walk's generation
/// guard, so a walk under an old profile never writes one.
fn record_counts(
    state: &mut ContradictionState,
    work: &[DomainWork],
    pending: &BTreeMap<String, usize>,
) {
    for w in work {
        let left = pending.get(&w.name).copied().unwrap_or(0);
        match &w.count {
            Some(count) => {
                state.counted.insert(
                    w.name.clone(),
                    DomainCount {
                        pending: left,
                        ..count.clone()
                    },
                );
            }
            None => {
                if let Some(kept) = state.counted.get_mut(&w.name) {
                    kept.pending = left;
                }
            }
        }
    }
}

/// Whether a failed load is a download whose wait is over at `now`.
fn load_retry_due(state: &ContradictionState, now: tokio::time::Instant) -> bool {
    state.load_retry_at.is_some_and(|at| now >= at)
}

/// How many failed pairs the state holds for `repo`.
fn failed_count(state: &ContradictionState, repo: &str) -> usize {
    state
        .failed
        .values()
        .map(|set| set.iter().filter(|f| f.repo == repo).count())
        .sum()
}

/// The key a failure of `a` against `b` under `model` is remembered by.
fn failed_key(
    model: &'static NliModel,
    a: &ContradictionFact,
    b: &ContradictionFact,
) -> FailedPair {
    FailedPair {
        repo: model.repo,
        a: a.id.0,
        b: b.id.0,
        checksum_a: a.checksum.clone(),
        checksum_b: b.checksum.clone(),
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
/// embedding model, the domain's id and every path with its checksum. Built
/// from the stamps alone, never from the vectors, so checking it costs one
/// narrow read. The id is there for a domain removed and added back under
/// its old name: its rows were cleared with it. The id alone is not the
/// guard, since an id can stay the same; the forget hook on removal is, and
/// the id only keeps a digest from matching a record of another domain.
fn walk_digest(
    model: &NliModel,
    threshold: f64,
    embedding_model: &str,
    domain: DomainId,
    stamps: &HashMap<String, FileStamp>,
) -> String {
    let mut h = Sha256::new();
    h.update(model.repo.as_bytes());
    h.update([0]);
    h.update(threshold.to_bits().to_le_bytes());
    h.update(embedding_model.as_bytes());
    h.update([0]);
    h.update(domain.0.to_le_bytes());
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
    fn the_walk_digest_moves_with_a_stamp_the_model_the_embedding_model_or_the_domain_only() {
        let full = nli_model(NliProfile::Full);
        // A second model, as a later release's table would carry.
        let other: &'static NliModel = Box::leak(Box::new(NliModel {
            repo: "example/another-nli-model",
            ..*full
        }));
        let d = DomainId(1);
        let stamps: HashMap<String, FileStamp> = [
            ("a.md".to_string(), stamp("1")),
            ("b.md".to_string(), stamp("2")),
        ]
        .into();
        let base = walk_digest(full, 0.8, "granite", d, &stamps);
        assert_eq!(base, walk_digest(full, 0.8, "granite", d, &stamps.clone()));
        let mut edited = stamps.clone();
        edited.insert("b.md".to_string(), stamp("3"));
        assert_ne!(base, walk_digest(full, 0.8, "granite", d, &edited));
        let mut touched = stamps.clone();
        touched.get_mut("a.md").unwrap().mtime = 99;
        assert_eq!(
            base,
            walk_digest(full, 0.8, "granite", d, &touched),
            "a touch that keeps the content keeps the digest"
        );
        assert_ne!(base, walk_digest(other, 0.8, "granite", d, &stamps));
        assert_ne!(base, walk_digest(full, 0.7, "granite", d, &stamps));
        assert_ne!(base, walk_digest(full, 0.8, "bge", d, &stamps));
        assert_ne!(
            base,
            walk_digest(full, 0.8, "granite", DomainId(2), &stamps),
            "the same files under another domain id are another domain"
        );
    }
}
