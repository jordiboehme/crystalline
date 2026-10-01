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
use crystalline_index::embed::line_similarity_floor;
use crystalline_index::nli::{
    CandidateFacts, CandidatePair, ContradictionScorer, KeptLine, LineRules,
    MAX_INFERENCES_PER_PASS, NLI_BATCH_SIZE, NLI_GROUP_LINE_PAIRS, NliModel, NliProfile,
    contradiction_candidates, eligible, eligible_lines, first_order_inputs, fold, length_order,
    line_rows, max_related_pairs, nli_model, observation_hash, pending_pairs, related_threshold,
    scoring_checksum, second_order_input, second_order_needed, similar_line_pairs,
};
use crystalline_index::sweep::{MAX_LINE_PAIRS_PER_ENGRAM_PAIR, ORDER_AGGREGATION};
use crystalline_index::{
    ContradictionRow, OBSERVATION_VECTOR_CHUNK, ObservationVector, ScoredPair,
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
    /// The scoring checksum ([`scoring_checksum`]): what the model reads of
    /// the engram under the line rules, which is what a scored pair and a
    /// parked failure are keyed by. A changed line, floor, limit or
    /// embedding model moves it; the file stamps only decide whether a
    /// domain is parsed again ([`walk_digest`]).
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
    /// The pairs this walk scores, each with its kept line pairs.
    pending: Vec<PairPlan>,
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
    /// What the walk's line step saw. `None` for a domain skipped as settled.
    lines: Option<LineCoverage>,
}

/// One pending engram pair and its kept line pairs; `lines` is `None` while
/// a line of either engram has no vector, and the pair then stays pending.
struct PairPlan {
    pair: CandidatePair,
    lines: Option<Vec<KeptLine>>,
}

impl PairPlan {
    /// How many kept line pairs the model reads for this pair; none while it
    /// waits for a line vector.
    fn line_count(&self) -> usize {
        self.lines.as_ref().map_or(0, Vec::len)
    }
}

/// What a walk's line step saw of one parsed domain.
pub(crate) struct LineCoverage {
    /// Distinct eligible lines.
    pub(crate) eligible: usize,
    /// How many of those carry a vector now.
    pub(crate) embedded: usize,
    /// Every eligible line's hash: the lines in use, for pruning. The same
    /// set the domain's settled record keeps.
    pub(crate) hashes: Arc<HashSet<String>>,
    /// Why some lines have no vector, when a batch was refused or no
    /// provider is loaded.
    pub(crate) error: Option<String>,
}

impl LineCoverage {
    /// Whether every eligible line has its vector.
    fn complete(&self) -> bool {
        self.embedded >= self.eligible
    }
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
    /// and `embedding_pending` (a domain the last walk counted, settled or
    /// not, still has a possible candidate with no lead vector yet, so its
    /// pending count of zero is not the whole story). `device` is where the
    /// loaded model runs, in the embeddings line's words (`metal`,
    /// `cpu (fallback: ...)`), and `null` while no model is in memory (never
    /// loaded, dropped after its idle time, or a direct read with no
    /// worker): no guess at a device nothing runs on. `embedding_model` is
    /// the id the line vectors are stored under, `line_floor` that model's
    /// line-similarity floor (`null` when it has none), and
    /// `line_floor_missing` says the check is on but `V302` cannot run
    /// because the model has no floor. `lines_eligible` and
    /// `lines_embedded` are the distinct observation lines that can take part
    /// and how many of them carry a vector, as the last walk counted them,
    /// and `null` like `pending_pairs` until a walk ran or while the check is
    /// off.
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
        // Every domain a walk counted, settled or not: a domain that cannot
        // settle because its lines cannot be embedded either (the provider
        // is down) still waits for a lead vector, and must say so.
        let embedding_pending = state.counted.values().any(|c| c.coverage.is_some());
        let last_error = state.last_error.clone();
        let (lines_embedded, lines_eligible) = match (model, &state.pending) {
            (Some(_), Some(_)) => (
                Some(
                    state
                        .counted
                        .values()
                        .map(|c| c.lines_embedded)
                        .sum::<usize>(),
                ),
                Some(
                    state
                        .counted
                        .values()
                        .map(|c| c.lines_eligible)
                        .sum::<usize>(),
                ),
            ),
            _ => (None, None),
        };
        drop(state);
        let line_floor = self.line_floor();
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
            "embedding_model": self.model_id,
            "line_floor": line_floor,
            "line_floor_missing": model.is_some() && line_floor.is_none(),
            "lines_embedded": lines_embedded,
            "lines_eligible": lines_eligible,
        }))
    }

    /// The line-similarity floor of the embedding model this engine stores
    /// vectors under; `None` when that model has none, and `V302` then does
    /// not run.
    pub fn line_floor(&self) -> Option<f64> {
        line_similarity_floor(&self.model_id)
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
        // Switched off: the next pass clears. Switched on: the next pass
        // embeds and scores. Either way it is asked for now, not at the next
        // write.
        self.contradiction_data_cleared
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.request_contradictions();
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
        if self.line_floor().is_none() {
            return false;
        }
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
            self.clear_contradiction_data_once().await?;
            return Ok(ContradictionOutcome::Off);
        };
        if self.line_floor().is_none() {
            *self.scorer.lock().unwrap() = None;
            return Ok(ContradictionOutcome::NoLineFloor);
        }
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

    /// The off switch: delete the stored scores and the line vectors, once
    /// per process and once per setting change. On a store this instance
    /// owns alone, the whole of both tables: the first off pass after a start
    /// runs before the startup sync claims any file domain, so a scope read
    /// then would hold the virtual domains only and leave the file domains'
    /// rows behind for good. On a shared database, only the scores of the
    /// domains in this instance's scope, and no line vectors, which other
    /// instances' domains may use. A failure leaves the flag unset, so the
    /// next off pass tries again.
    async fn clear_contradiction_data_once(&self) -> Result<()> {
        use std::sync::atomic::Ordering;
        if self.contradiction_data_cleared.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let done = async {
            let store = self.store.lock().await;
            if store.shares_database() {
                let scope = self.embed_scope(&*store).await?;
                store.clear_contradictions(scope.as_deref()).await?;
            } else {
                store.clear_contradictions(None).await?;
                store.clear_observation_vectors().await?;
            }
            Ok::<(), EngineError>(())
        }
        .await;
        if done.is_err() {
            self.contradiction_data_cleared
                .store(false, Ordering::SeqCst);
        }
        done
    }

    /// Delete the line vectors no current eligible line uses, after a walk
    /// that parsed every domain in scope; and, once per process, every
    /// vector of another embedding model. Only on a store this instance owns
    /// alone: on a shared database another instance's domains use vectors
    /// this one never sees.
    async fn prune_line_vectors(&self, work: &[DomainWork]) -> Result<usize> {
        use std::sync::atomic::Ordering;
        if !self.line_vectors_model_pruned.swap(true, Ordering::SeqCst) {
            let done = {
                let store = self.store.lock().await;
                store
                    .delete_observation_vectors_except(&self.model_id)
                    .await
            };
            if let Err(e) = done {
                // Tried again on the next full walk.
                self.line_vectors_model_pruned
                    .store(false, Ordering::SeqCst);
                return Err(e.into());
            }
        }
        // A parsed domain's settle record carries the lines this walk saw, a
        // skipped one's the lines of the walk that settled it, which its
        // unchanged digest says are still the lines in use.
        let in_use: HashSet<&str> = work
            .iter()
            .flat_map(|w| w.settle.line_hashes.iter().map(String::as_str))
            .collect();
        let stored = {
            let store = self.store.lock().await;
            store.observation_vector_hashes(&self.model_id).await?
        };
        let unused: Vec<String> = stored
            .into_iter()
            .filter(|h| !in_use.contains(h.as_str()))
            .collect();
        if unused.is_empty() {
            return Ok(0);
        }
        let store = self.store.lock().await;
        Ok(store
            .delete_observation_vectors(&self.model_id, &unused)
            .await? as usize)
    }

    /// Give every eligible line of `facts` a vector: read which already have
    /// one, embed the rest in batches through the active provider (the same
    /// device as the chunks) and store them. The store lock is taken per read
    /// and per batch write, never across an embed. A batch the provider
    /// refuses, or the store does not take, is skipped and named in `error`;
    /// its lines stay without a vector, so the domain does not settle and the
    /// next walk tries again.
    async fn embed_lines(
        &self,
        model: &'static NliModel,
        facts: &[ContradictionFact],
    ) -> Result<LineCoverage> {
        let lines = {
            let views: Vec<CandidateFacts<'_>> =
                facts.iter().map(ContradictionFact::view).collect();
            eligible_lines(&views)
        };
        let hashes: Vec<String> = lines.keys().cloned().collect();
        let present = if hashes.is_empty() {
            HashSet::new()
        } else {
            let store = self.store.lock().await;
            store
                .observation_vectors_present(&self.model_id, &hashes)
                .await?
        };
        let missing: Vec<(&String, &String)> = lines
            .iter()
            .filter(|(h, _)| !present.contains(*h))
            .collect();
        let mut embedded = present.len();
        let mut error = None;
        if !missing.is_empty() {
            match self.provider() {
                None => {
                    error = Some(
                        "observation lines could not be embedded: the embedding provider is not loaded yet"
                            .to_string(),
                    )
                }
                Some(provider) => {
                    let started = std::time::Instant::now();
                    let mut stored = 0usize;
                    for batch in missing.chunks(EMBED_BATCH) {
                        // Off, or another profile, ends the step at once.
                        if self.contradiction_model().map(|m| m.repo) != Some(model.repo) {
                            break;
                        }
                        let texts: Vec<String> =
                            batch.iter().map(|(_, t)| (*t).clone()).collect();
                        match provider.embed(&texts).await {
                            Ok(vectors) if vectors.len() == batch.len() => {
                                let rows: Vec<ObservationVector> = batch
                                    .iter()
                                    .zip(vectors)
                                    .map(|((hash, _), vector)| ObservationVector {
                                        hash: (*hash).clone(),
                                        vector,
                                    })
                                    .collect();
                                // A refused write is one batch's failure, not
                                // the walk's: its lines stay without a vector.
                                // The profile is read again under the store
                                // lock, which the off switch's clear takes
                                // too, so nothing lands after that clear.
                                let written = {
                                    let store = self.store.lock().await;
                                    if self.contradiction_model().map(|m| m.repo)
                                        != Some(model.repo)
                                    {
                                        None
                                    } else {
                                        Some(
                                            store
                                                .store_observation_vectors(&self.model_id, &rows)
                                                .await,
                                        )
                                    }
                                };
                                match written {
                                    None => break,
                                    Some(Ok(())) => stored += rows.len(),
                                    Some(Err(e)) => {
                                        error = Some(format!(
                                            "observation line vectors could not be stored: {e}"
                                        ))
                                    }
                                }
                            }
                            Ok(vectors) => {
                                error = Some(format!(
                                    "observation lines could not be embedded: the provider returned {} vectors for {} lines",
                                    vectors.len(),
                                    batch.len()
                                ))
                            }
                            Err(e) => {
                                error =
                                    Some(format!("observation lines could not be embedded: {e}"))
                            }
                        }
                    }
                    embedded += stored;
                    if stored > 0 {
                        tracing::info!(
                            lines = stored,
                            ms = started.elapsed().as_millis() as u64,
                            "embedded observation lines for the contradiction check"
                        );
                    }
                }
            }
        }
        Ok(LineCoverage {
            eligible: lines.len(),
            embedded,
            hashes: Arc::new(lines.into_keys().collect()),
            error,
        })
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
        let (work, complete) = self.contradiction_work(model, retry).await?;
        let line_error = work
            .iter()
            .find_map(|w| w.lines.as_ref().and_then(|l| l.error.clone()));
        // Pruned here, before the return below for a walk with nothing
        // pending: that is most walks after a delete, the one that leaves
        // vectors unused. Only after a walk that knows every domain's lines
        // in use and embedded every line, and never on a shared database.
        let shared = self.store.lock().await.shares_database();
        if complete && line_error.is_none() && !shared {
            match self.prune_line_vectors(&work).await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(vectors = n, "pruned unused observation line vectors"),
                Err(e) => {
                    tracing::warn!("could not prune unused observation line vectors: {e}")
                }
            }
        }
        let mut pending: BTreeMap<String, usize> = work
            .iter()
            .map(|w| (w.name.clone(), w.pending.len() + w.known_failing))
            .collect();
        let mut failures: HashMap<String, HashSet<FailedPair>> = work
            .iter()
            .map(|w| (w.name.clone(), w.failures.clone()))
            .collect();
        // A pair whose line vectors are missing waits and costs nothing, so a
        // walk where every pending pair waits has nothing to score.
        if work
            .iter()
            .all(|w| w.pending.iter().all(|p| p.lines.is_none()))
        {
            let remaining = pending.values().sum();
            self.publish_walk(model, generation, &work, pending, failures, line_error);
            return Ok(ContradictionOutcome::Scored {
                pairs: 0,
                line_pairs: 0,
                remaining,
            });
        }
        // The model is loaded only when some pair has a line pair to read. A
        // pair with none above the floor, the common case for a related
        // pair, is stored empty without it.
        let needs_model = work
            .iter()
            .any(|w| w.pending.iter().any(|p| p.line_count() > 0));
        let scorer = if needs_model {
            match self.scorer_for(model).await {
                Ok(scorer) => {
                    let mut state = self.contradiction_state.lock().unwrap();
                    state.load_backoff = None;
                    state.load_retry_at = None;
                    Some(scorer)
                }
                Err(e) => {
                    let current = self.contradiction_model().map(|m| m.repo);
                    let mut state = self.contradiction_state.lock().unwrap();
                    if state.generation == generation && current == Some(model.repo) {
                        if matches!(e, IndexError::NliFetch(_)) {
                            // A download that failed is tried again later,
                            // each failure waiting twice as long, up to an
                            // hour.
                            let wait = state.load_backoff.map_or(NLI_FETCH_RETRY_FIRST, |w| {
                                (w * 2).min(NLI_FETCH_RETRY_MAX)
                            });
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
            }
        } else {
            None
        };
        let _activity = ActivityState::begin(&self.activity, "contradictions", None);
        let started = std::time::Instant::now();
        let mut budget = MAX_INFERENCES_PER_PASS;
        let (mut pairs_done, mut lines_done, mut batches) = (0usize, 0usize, 0usize);
        let mut batch_error: Option<String> = None;
        'domains: for w in &work {
            // Whole pairs within the budget, in the order the walk found
            // them; a pair whose line vectors are missing waits and costs
            // nothing.
            let mut take: Vec<&PairPlan> = Vec::new();
            let mut out_of_budget = false;
            for plan in &w.pending {
                let Some(lines) = &plan.lines else {
                    continue;
                };
                if lines.len() > budget {
                    out_of_budget = true;
                    break;
                }
                budget -= lines.len();
                take.push(plan);
            }
            for group in groups_of(&take, NLI_GROUP_LINE_PAIRS) {
                // The profile is read between groups, so off, or another
                // profile, ends this walk at once and lets go of the model.
                if self.contradiction_model().map(|m| m.repo) != Some(model.repo) {
                    break 'domains;
                }
                let (results, n) = self
                    .score_and_store_group(model, w.id, &w.facts, group, scorer.as_ref())
                    .await;
                batches += n;
                let set = failures.entry(w.name.clone()).or_default();
                for (plan, done) in group.iter().zip(results) {
                    let (a, b) = (&w.facts[plan.pair.a], &w.facts[plan.pair.b]);
                    let key = failed_key(model, a, b);
                    // One failing batch or store write fails its own pair,
                    // which stays pending and unstored, and the walk goes on
                    // with the others: a pair is stored whole or not at all.
                    match done {
                        Ok(true) => {
                            set.remove(&key);
                            pairs_done += 1;
                            lines_done += plan.line_count();
                            if let Some(left) = pending.get_mut(&w.name) {
                                *left -= 1;
                            }
                        }
                        // The profile moved before the write: nothing was
                        // stored, so nothing is counted, and the walk ends.
                        Ok(false) => break 'domains,
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
                        }
                    }
                }
            }
            if out_of_budget {
                break 'domains;
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
        self.publish_walk(
            model,
            generation,
            &work,
            pending,
            failures,
            batch_error.or(line_error),
        );
        Ok(ContradictionOutcome::Scored {
            pairs: pairs_done,
            line_pairs: lines_done,
            remaining,
        })
    }

    /// Score a group of pending pairs and store each one. A scoring failure
    /// for the whole group is retried pair by pair, so one bad pair fails
    /// alone (lesson 2); a store failure fails its own pair only. Returns one
    /// result per pair, in order (`Ok(false)` for a pair not stored because
    /// the profile moved, and for every pair after it), and the batches the
    /// group took. With no `scorer`, only pairs with no kept line pair are
    /// stored; the walk loads the model whenever another pair is pending.
    async fn score_and_store_group(
        &self,
        model: &'static NliModel,
        domain: DomainId,
        facts: &[ContradictionFact],
        group: &[&PairPlan],
        scorer: Option<&Arc<dyn ContradictionScorer>>,
    ) -> (Vec<Result<bool>>, usize) {
        let items: Vec<GroupItem<'_>> = group
            .iter()
            .map(|p| {
                (
                    &facts[p.pair.a],
                    &facts[p.pair.b],
                    p.lines.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        let mut batches = 0usize;
        let scored: Vec<Result<OrderScores>> = match scorer {
            None => items
                .iter()
                .map(|(_, _, lines)| {
                    if lines.is_empty() {
                        Ok((Vec::new(), Vec::new()))
                    } else {
                        Err(EngineError::Internal(
                            "a pair with line pairs to read reached the store without a contradiction model"
                                .to_string(),
                        ))
                    }
                })
                .collect(),
            Some(scorer) => match score_group(scorer, &items).await {
                Ok((ab, ba, n)) => {
                    batches += n;
                    ab.into_iter().zip(ba).map(Ok).collect()
                }
                Err(_) if items.len() > 1 => {
                    let mut one_by_one = Vec::with_capacity(items.len());
                    for item in &items {
                        match score_group(scorer, std::slice::from_ref(item)).await {
                            Ok((mut ab, mut ba, n)) => {
                                batches += n;
                                one_by_one.push(Ok((ab.remove(0), ba.remove(0))));
                            }
                            Err(e) => one_by_one.push(Err(e)),
                        }
                    }
                    one_by_one
                }
                Err(e) => vec![Err(e)],
            },
        };
        let mut out = Vec::with_capacity(items.len());
        let mut moved = false;
        for ((plan, (a, b, lines)), result) in group.iter().zip(&items).zip(scored) {
            if moved {
                out.push(Ok(false));
                continue;
            }
            let done = match result {
                Err(e) => Err(e),
                Ok((ab, ba)) => match line_rows(
                    a.id,
                    b.id,
                    &a.observations,
                    &b.observations,
                    lines,
                    &ab,
                    &ba,
                    ORDER_AGGREGATION,
                ) {
                    // An incomplete answer never reads as a scored pair with
                    // no rows: the pair stays pending.
                    None => Err(EngineError::Internal(format!(
                        "the contradiction scores of engrams {} and {} do not match their {} line pairs",
                        a.id.0,
                        b.id.0,
                        lines.len()
                    ))),
                    Some(rows) => {
                        self.store_pair(model, domain, plan.pair.cosine, a, b, &rows)
                            .await
                    }
                },
            };
            moved = matches!(done, Ok(false));
            out.push(done);
        }
        (out, batches)
    }

    /// Record one scored pair: its pair row at the current checksums and its
    /// line rows. The store lock is taken for the write only, never across a
    /// batch. `false` when the profile moved and nothing was stored.
    async fn store_pair(
        &self,
        model: &'static NliModel,
        domain: DomainId,
        cosine: f64,
        a: &ContradictionFact,
        b: &ContradictionFact,
        rows: &[ContradictionRow],
    ) -> Result<bool> {
        let scored = ScoredPair {
            a: a.id,
            b: b.id,
            checksum_a: a.checksum.clone(),
            checksum_b: b.checksum.clone(),
        };
        let store = self.store.lock().await;
        // Read again under the store lock, which the off switch's clear takes
        // too: a pair scored while the check was turned off is not stored
        // after that clear. The walk then publishes nothing, since the
        // profile moved.
        if self.contradiction_model().map(|m| m.repo) != Some(model.repo) {
            return Ok(false);
        }
        store
            .replace_contradictions(
                domain,
                &scored,
                cosine,
                model.repo,
                &Utc::now().to_rfc3339(),
                rows,
            )
            .await?;
        Ok(true)
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
            // A line without its vector keeps the domain unsettled, so the
            // next walk parses it again and tries the embedding once more.
            let lines_complete = w.lines.as_ref().is_none_or(LineCoverage::complete);
            if left == failing.len() && lines_complete {
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

    /// The first half of a walk: every domain in scope with its pending pairs
    /// and its lines embedded. Known failures are left alone unless `retry`.
    /// The `bool` says the walk knows every known domain's lines in use:
    /// each was parsed, or skipped as settled with the lines its settle
    /// record keeps; none was out of this instance's scope or skipped for a
    /// missing content source or domain row. Only such a walk may prune.
    async fn contradiction_work(
        &self,
        model: &'static NliModel,
        retry: bool,
    ) -> Result<(Vec<DomainWork>, bool)> {
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
        // `score_contradictions` answers `NoLineFloor` before any walk.
        let Some(floor) = self.line_floor() else {
            return Err(EngineError::Internal(
                "a contradiction walk started without a line-similarity floor".to_string(),
            ));
        };
        let rules = LineRules {
            embedding_model: &self.model_id,
            floor,
            max_line_pairs: MAX_LINE_PAIRS_PER_ENGRAM_PAIR,
        };
        let mut out = Vec::new();
        let mut complete = true;
        for name in names {
            let Ok(source) = self.content_source(&name) else {
                complete = false;
                continue;
            };
            let (domain_id, stamps) = {
                let store = self.store.lock().await;
                let Some(domain_id) = store.domain_id(&name).await? else {
                    complete = false;
                    continue;
                };
                (domain_id, store.file_stamps(domain_id).await?)
            };
            // Out of scope is not parsed either: a daemon's file domains are
            // out of its scope until the startup sync claims them, and their
            // lines must not read as unused meanwhile.
            if scope.as_ref().is_some_and(|ids| !ids.contains(&domain_id)) {
                complete = false;
                continue;
            }
            let digest = walk_digest(model, threshold, &rules, domain_id, &stamps);
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
                // Skipped, but its lines in use are known: its settle record
                // carries them, so the walk stays complete.
                out.push(DomainWork {
                    name,
                    id: domain_id,
                    facts: Vec::new(),
                    pending: Vec::new(),
                    known_failing: known.len(),
                    failures: known,
                    settle,
                    count: None,
                    lines: None,
                });
                continue;
            }
            let facts = self
                .contradiction_facts(&source, &name, domain_id, &stamps, true, &rules)
                .await?;
            let lines = self.embed_lines(model, &facts).await?;
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
                line_hashes: Arc::clone(&lines.hashes),
            };
            let mut count = DomainCount {
                digest: settle.digest.clone(),
                coverage: settle.coverage,
                pending: 0,
                capped: false,
                vectors_capped: None,
                unembedded,
                lines_eligible: lines.eligible,
                lines_embedded: lines.embedded,
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
            let (facts, pending) = self.plan_pairs(facts, pending, floor).await?;
            out.push(DomainWork {
                name,
                id: domain_id,
                facts,
                pending,
                known_failing,
                failures,
                settle,
                count: Some(count),
                lines: Some(lines),
            });
        }
        Ok((out, complete))
    }

    /// Pair the lines of each pending engram pair by similarity
    /// ([`similar_line_pairs`]): read the vectors of every line of the
    /// engrams in `pending` (the store lock per run of
    /// [`OBSERVATION_VECTOR_CHUNK`] hashes, so a large domain never holds it
    /// for long), then pair on a blocking thread, since every line of one
    /// engram meets every line of the other. The facts move in and come back.
    async fn plan_pairs(
        &self,
        facts: Vec<ContradictionFact>,
        pending: Vec<CandidatePair>,
        floor: f64,
    ) -> Result<(Vec<ContradictionFact>, Vec<PairPlan>)> {
        if pending.is_empty() {
            return Ok((facts, Vec::new()));
        }
        let wanted: Vec<String> = {
            let mut set: HashSet<String> = HashSet::new();
            for p in &pending {
                for f in [&facts[p.a], &facts[p.b]] {
                    for o in &f.observations {
                        if !fold(&o.text).is_empty() {
                            set.insert(observation_hash(&o.text));
                        }
                    }
                }
            }
            set.into_iter().collect()
        };
        // Only the vectors of the model the floor belongs to.
        let mut vectors: HashMap<String, Vec<f32>> = HashMap::with_capacity(wanted.len());
        for run in wanted.chunks(OBSERVATION_VECTOR_CHUNK) {
            let store = self.store.lock().await;
            vectors.extend(store.observation_vectors(&self.model_id, run).await?);
        }
        tokio::task::spawn_blocking(move || {
            let plans: Vec<PairPlan> = pending
                .into_iter()
                .map(|pair| PairPlan {
                    lines: similar_line_pairs(
                        &facts[pair.a].observations,
                        &facts[pair.b].observations,
                        &vectors,
                        floor,
                        MAX_LINE_PAIRS_PER_ENGRAM_PAIR,
                    ),
                    pair,
                })
                .collect();
            (facts, plans)
        })
        .await
        .map_err(|e| EngineError::Internal(format!("contradiction line pairing failed: {e}")))
    }

    /// The contradiction check's facts for one domain's base engrams: the
    /// listing, each engram parsed through `source` (frontmatter status and
    /// window, observations), its scoring checksum under `rules`, and, when
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
        rules: &LineRules<'_>,
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
                checksum: scoring_checksum(&observations, rules),
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
        // No measured floor for this embedding model: V302 does not run, and
        // rows another model's floor left behind are not read.
        let Some(floor) = self.line_floor() else {
            return Ok(SweepContradictions {
                model: Some(model),
                ..SweepContradictions::default()
            });
        };
        let rules = LineRules {
            embedding_model: &self.model_id,
            floor,
            max_line_pairs: MAX_LINE_PAIRS_PER_ENGRAM_PAIR,
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
                    walk_digest(model, related_threshold(), &rules, domain_id, &stamps),
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

/// One engram pair of a scoring group: both engrams and the kept line pairs.
type GroupItem<'f> = (&'f ContradictionFact, &'f ContradictionFact, &'f [KeptLine]);

/// One pair's scores: the first order per kept line pair, and the second
/// where it was read.
type OrderScores = (Vec<f32>, Vec<Option<f32>>);

/// Cut `plans` into groups of whole pairs holding about `lines` kept line
/// pairs each; a pair with no kept line pair rides along at no cost.
fn groups_of<'p>(plans: &'p [&'p PairPlan], lines: usize) -> Vec<&'p [&'p PairPlan]> {
    let mut out = Vec::new();
    let (mut start, mut held) = (0usize, 0usize);
    for (i, plan) in plans.iter().enumerate() {
        held += plan.line_count();
        if held >= lines {
            out.push(&plans[start..=i]);
            start = i + 1;
            held = 0;
        }
    }
    if start < plans.len() {
        out.push(&plans[start..]);
    }
    out
}

/// Score `inputs` in batches of [`NLI_BATCH_SIZE`], sorted by length first
/// ([`length_order`]) so a batch pads to a near neighbour, each batch on a
/// blocking thread of its own, so no blocking unit outlasts one batch. The
/// probabilities come back in the order of `inputs`, with the batch count. A
/// batch that fails, or answers with the wrong number of scores, fails the
/// call.
async fn run_scorer(
    scorer: Arc<dyn ContradictionScorer>,
    inputs: Vec<(String, String)>,
) -> Result<(Vec<f32>, usize)> {
    let order = length_order(&inputs);
    let mut out = vec![f32::NAN; inputs.len()];
    let mut batches = 0usize;
    for run in order.chunks(NLI_BATCH_SIZE) {
        let batch: Vec<(String, String)> = run.iter().map(|&i| inputs[i].clone()).collect();
        let (scorer, expected) = (Arc::clone(&scorer), batch.len());
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
        for (&i, s) in run.iter().zip(scores) {
            out[i] = s;
        }
        batches += 1;
    }
    Ok((out, batches))
}

/// Both reading orders for a group of engram pairs: the first order of every
/// kept line pair in one run, then the second order only where
/// [`second_order_needed`] says so. Per pair, the first-order scores, the
/// second-order scores (`None` where not read), and the group's batch count.
async fn score_group(
    scorer: &Arc<dyn ContradictionScorer>,
    group: &[GroupItem<'_>],
) -> Result<(Vec<Vec<f32>>, Vec<Vec<Option<f32>>>, usize)> {
    let mut first = Vec::new();
    let mut spans = Vec::with_capacity(group.len());
    for (a, b, lines) in group {
        let start = first.len();
        first.extend(first_order_inputs(&a.observations, &b.observations, lines));
        spans.push(start..first.len());
    }
    let (ab_all, mut batches) = run_scorer(Arc::clone(scorer), first).await?;
    let mut second = Vec::new();
    let mut slots: Vec<(usize, usize)> = Vec::new();
    for (p, ((a, b, lines), span)) in group.iter().zip(&spans).enumerate() {
        let needed = second_order_needed(&ab_all[span.clone()], ORDER_AGGREGATION);
        for (i, need) in needed.into_iter().enumerate() {
            if need {
                second.push(second_order_input(
                    &a.observations,
                    &b.observations,
                    &lines[i],
                ));
                slots.push((p, i));
            }
        }
    }
    let (ba_all, more) = run_scorer(Arc::clone(scorer), second).await?;
    batches += more;
    let mut ba: Vec<Vec<Option<f32>>> = spans.iter().map(|s| vec![None; s.len()]).collect();
    for ((p, i), v) in slots.into_iter().zip(ba_all) {
        ba[p][i] = Some(v);
    }
    let ab = spans.iter().map(|s| ab_all[s.clone()].to_vec()).collect();
    Ok((ab, ba, batches))
}

/// What a domain looked like to a walk: the NLI model, the related line, the
/// line rules (whose key carries the embedding model), the domain's id and
/// every path with its checksum. Built
/// from the stamps alone, never from the vectors, so checking it costs one
/// narrow read. The id is there for a domain removed and added back under
/// its old name: its rows were cleared with it. The id alone is not the
/// guard, since an id can stay the same; the forget hook on removal is, and
/// the id only keeps a digest from matching a record of another domain.
fn walk_digest(
    model: &NliModel,
    threshold: f64,
    rules: &LineRules<'_>,
    domain: DomainId,
    stamps: &HashMap<String, FileStamp>,
) -> String {
    let mut h = Sha256::new();
    h.update(model.repo.as_bytes());
    h.update([0]);
    h.update(threshold.to_bits().to_le_bytes());
    h.update(rules.key().as_bytes());
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
    fn the_walk_digest_moves_with_a_stamp_the_models_the_rules_or_the_domain_only() {
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
        let rules = LineRules {
            embedding_model: "granite",
            floor: 0.86,
            max_line_pairs: 4,
        };
        let base = walk_digest(full, 0.8, &rules, d, &stamps);
        assert_eq!(base, walk_digest(full, 0.8, &rules, d, &stamps.clone()));
        let mut edited = stamps.clone();
        edited.insert("b.md".to_string(), stamp("3"));
        assert_ne!(base, walk_digest(full, 0.8, &rules, d, &edited));
        let mut touched = stamps.clone();
        touched.get_mut("a.md").unwrap().mtime = 99;
        assert_eq!(
            base,
            walk_digest(full, 0.8, &rules, d, &touched),
            "a touch that keeps the content keeps the digest"
        );
        assert_ne!(base, walk_digest(other, 0.8, &rules, d, &stamps));
        assert_ne!(base, walk_digest(full, 0.7, &rules, d, &stamps));
        let bge = LineRules {
            embedding_model: "bge",
            ..rules
        };
        assert_ne!(base, walk_digest(full, 0.8, &bge, d, &stamps));
        let higher = LineRules {
            floor: 0.88,
            ..rules
        };
        assert_ne!(base, walk_digest(full, 0.8, &higher, d, &stamps));
        let more = LineRules {
            max_line_pairs: 5,
            ..rules
        };
        assert_ne!(base, walk_digest(full, 0.8, &more, d, &stamps));
        assert_ne!(
            base,
            walk_digest(full, 0.8, &rules, DomainId(2), &stamps),
            "the same files under another domain id are another domain"
        );
    }

    /// Scores each pair by the premise's length and records the order the
    /// premises arrived in.
    struct ByLength(std::sync::Mutex<Vec<String>>);

    impl ContradictionScorer for ByLength {
        fn score(&self, pairs: &[(String, String)]) -> crystalline_index::Result<Vec<f32>> {
            let mut seen = self.0.lock().unwrap();
            Ok(pairs
                .iter()
                .map(|(p, _)| {
                    seen.push(p.clone());
                    p.len() as f32 / 100.0
                })
                .collect())
        }
        fn model_repo(&self) -> &str {
            "repo/x"
        }
    }

    /// 11c: the batches see the inputs shortest first, and the scores come
    /// back in the order the inputs were given.
    #[tokio::test]
    async fn the_scorer_sees_inputs_by_length_and_answers_in_input_order() {
        let premises: Vec<String> = (0..40).map(|i| "x".repeat(1 + (i * 7) % 40)).collect();
        let inputs: Vec<(String, String)> = premises
            .iter()
            .map(|p| (p.clone(), "h".to_string()))
            .collect();
        let scorer = Arc::new(ByLength(std::sync::Mutex::new(Vec::new())));
        let (scores, batches) = run_scorer(scorer.clone(), inputs).await.unwrap();
        assert_eq!(batches, 3, "40 inputs in batches of 16");
        let expected: Vec<f32> = premises.iter().map(|p| p.len() as f32 / 100.0).collect();
        assert_eq!(scores, expected);
        {
            let seen = scorer.0.lock().unwrap();
            assert!(
                seen.windows(2).all(|w| w[0].len() <= w[1].len()),
                "shortest first"
            );
        }
        assert_eq!(
            run_scorer(scorer.clone(), Vec::new()).await.unwrap(),
            (Vec::new(), 0)
        );
    }
}
