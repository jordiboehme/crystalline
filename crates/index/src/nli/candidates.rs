//! Which engram pairs the contradiction check scores and how their
//! observation lines are paired. Pure: the engine assembles the facts from the
//! base listing, the files and the stamps, and hands them in borrowed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use chrono::NaiveDate;
use sha2::{Digest, Sha256};

use super::OrderAggregation;
use super::period::names_period;
use crate::store::{ContradictionRow, EngramId, ScoredPair, is_current_status};
use crate::sweep::twins::find_twins_where;
use crate::sweep::{
    CONTRADICTION_STORE_FLOOR, FactObservation, MAX_RELATED_PAIRS, RELATED_THRESHOLD,
    SPECULATIVE_STATUSES, SweepOptions,
};

/// What candidate selection reads about one base engram.
#[derive(Debug, Clone, Copy)]
pub struct CandidateFacts<'a> {
    /// The engram's id.
    pub id: EngramId,
    /// The effective frontmatter status, lowercased.
    pub status: &'a str,
    /// Start of the validity window; absent is unbounded.
    pub valid_from: Option<NaiveDate>,
    /// End of the validity window; absent is unbounded.
    pub valid_to: Option<NaiveDate>,
    /// What the model reads of the engram, [`observations_digest`]: a pair
    /// stays scored while both digests match, whatever else in the files
    /// changed.
    pub checksum: &'a str,
    /// The lead vector for the active embedding model, when stored.
    pub lead_vector: Option<&'a [f32]>,
    /// The engram's observation bullets, category and tags stripped.
    pub observations: &'a [FactObservation],
}

/// One related pair, as indices into the facts slice. `a` has the lower id,
/// which is how the tables key a pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidatePair {
    /// The index of the engram with the lower id.
    pub a: usize,
    /// The index of the engram with the higher id.
    pub b: usize,
    /// The lead-vector cosine.
    pub cosine: f64,
}

/// One domain's related pairs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Candidates {
    /// The pairs, highest cosine first.
    pub pairs: Vec<CandidatePair>,
    /// How many vectors took part.
    pub compared: usize,
    /// The scope was over the vector cap and nothing was compared.
    pub capped: bool,
    /// More eligible, window-overlapping pairs cleared the threshold than
    /// `max_pairs`, so the less related ones were dropped. Exactly
    /// `max_pairs` is not full.
    pub full: bool,
}

/// Whether an engram takes part in `V302` as of `today`: a current status
/// that is not speculative, and not past its `valid_to`. Expired means
/// strictly before `today`; a window that ends today still counts, and an
/// absent `valid_to` never expires. The one rule the candidates, the lines
/// the walk embeds and the sweep's read all apply.
pub fn eligible(status: &str, valid_to: Option<NaiveDate>, today: NaiveDate) -> bool {
    is_current_status(status)
        && !SPECULATIVE_STATUSES.contains(&status)
        && !expired(valid_to, today)
}

/// Whether a validity window closed before `today`: strictly before, so a
/// window that ends today is still open, and an absent `valid_to` never
/// closes.
pub fn expired(valid_to: Option<NaiveDate>, today: NaiveDate) -> bool {
    valid_to.is_some_and(|to| to < today)
}

/// Whether `[from_a, to_a]` and `[from_b, to_b]` overlap; an absent bound is
/// unbounded.
pub fn windows_overlap(
    from_a: Option<NaiveDate>,
    to_a: Option<NaiveDate>,
    from_b: Option<NaiveDate>,
    to_b: Option<NaiveDate>,
) -> bool {
    let starts_by = |from: Option<NaiveDate>, to: Option<NaiveDate>| match (from, to) {
        (Some(f), Some(t)) => f <= t,
        _ => true,
    };
    starts_by(from_a, to_b) && starts_by(from_b, to_a)
}

/// [`RELATED_THRESHOLD`], or `CRYSTALLINE_NLI_RELATED` when the eval harness
/// sets it. Read once per process. Undocumented for users and absent from the
/// settings registry on purpose.
pub fn related_threshold() -> f64 {
    static VALUE: OnceLock<f64> = OnceLock::new();
    *VALUE.get_or_init(|| related_from(std::env::var("CRYSTALLINE_NLI_RELATED").ok().as_deref()))
}

/// [`MAX_RELATED_PAIRS`], or `CRYSTALLINE_NLI_MAX_PAIRS` when the eval harness
/// sets it. Read once per process.
pub fn max_related_pairs() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE
        .get_or_init(|| max_pairs_from(std::env::var("CRYSTALLINE_NLI_MAX_PAIRS").ok().as_deref()))
}

fn related_from(raw: Option<&str>) -> f64 {
    raw.and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| (0.0..=1.0).contains(v))
        .unwrap_or(RELATED_THRESHOLD)
}

fn max_pairs_from(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(MAX_RELATED_PAIRS)
}

/// Every eligible (as of `today`), window-overlapping pair of engrams with
/// observations whose lead vectors sit at or above `related_threshold`, at
/// most `max_pairs`, highest cosine first. The twin finder's all-pairs walk at a lower line, so
/// the vector cap and the bounded retention behave as `V301`'s do.
pub fn contradiction_candidates(
    facts: &[CandidateFacts<'_>],
    related_threshold: f64,
    max_pairs: usize,
    today: NaiveDate,
) -> Candidates {
    let vectors: Vec<Option<&[f32]>> = facts
        .iter()
        .map(|f| {
            if eligible(f.status, f.valid_to, today) && !f.observations.is_empty() {
                f.lead_vector
            } else {
                None
            }
        })
        .collect();
    // One slot over the cap, so a dropped pair shows as a surplus. The window
    // rule runs inside the walk, before retention, so a pair whose windows
    // never meet cannot take a slot from one that overlaps.
    let options = SweepOptions {
        twin_threshold: related_threshold,
        max_twin_pairs: max_pairs.saturating_add(1),
        ..SweepOptions::default()
    };
    let found = find_twins_where(&vectors, &options, |i, j| {
        let (a, b) = (&facts[i], &facts[j]);
        windows_overlap(a.valid_from, a.valid_to, b.valid_from, b.valid_to)
    });
    let full = found.pairs.len() > max_pairs;
    let pairs = found
        .pairs
        .into_iter()
        .take(max_pairs)
        .map(|p| {
            if facts[p.a].id.0 <= facts[p.b].id.0 {
                CandidatePair {
                    a: p.a,
                    b: p.b,
                    cosine: p.cosine,
                }
            } else {
                CandidatePair {
                    a: p.b,
                    b: p.a,
                    cosine: p.cosine,
                }
            }
        })
        .collect();
    Candidates {
        pairs,
        compared: found.compared,
        capped: found.capped,
        full,
    }
}

/// The candidates with no scored row whose checksums both still match.
/// `scored` is one model's rows, so a model change leaves everything pending.
pub fn pending_pairs(
    facts: &[CandidateFacts<'_>],
    candidates: &[CandidatePair],
    scored: &[ScoredPair],
) -> Vec<CandidatePair> {
    let done: HashMap<(i64, i64), (&str, &str)> = scored
        .iter()
        .map(|s| {
            (
                (s.a.0, s.b.0),
                (s.checksum_a.as_str(), s.checksum_b.as_str()),
            )
        })
        .collect();
    candidates
        .iter()
        .copied()
        .filter(|p| {
            let (a, b) = (&facts[p.a], &facts[p.b]);
            done.get(&(a.id.0, b.id.0)) != Some(&(a.checksum, b.checksum))
        })
        .collect()
}

/// The text with every whitespace run folded to one space and the ends
/// trimmed: what the model reads and what the row hash is taken of.
pub fn fold(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The freshness key of one engram for the contradiction check: the
/// lowercase hex sha256 of its observation texts, folded, in document order,
/// lines with no words left out (they are never paired). Only these texts
/// reach the model, so a frontmatter edit (an acknowledgment, a salience, a
/// provenance refresh) or a prose edit leaves the key and every scored pair
/// as they are, and a changed, added, removed or moved line changes it.
pub fn observations_digest(observations: &[FactObservation]) -> String {
    let mut h = Sha256::new();
    for o in observations {
        let folded = fold(&o.text);
        if folded.is_empty() {
            continue;
        }
        h.update(folded.as_bytes());
        h.update([0]);
    }
    crate::hex_lower(&h.finalize())
}

/// The lowercase hex sha256 of the folded text.
pub fn observation_hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(fold(text).as_bytes());
    crate::hex_lower(&h.finalize())
}

/// One kept line pair of an engram pair: indices into each engram's
/// observations, both row hashes and the line cosine.
#[derive(Debug, Clone, PartialEq)]
pub struct KeptLine {
    /// The index of the line in the lower-id engram's observations.
    pub ia: usize,
    /// The index of the line in the higher-id engram's observations.
    pub ib: usize,
    /// [`observation_hash`] of `a`'s line.
    pub hash_a: String,
    /// [`observation_hash`] of `b`'s line.
    pub hash_b: String,
    /// The cosine of the two lines' vectors.
    pub similarity: f32,
}

/// What decides which line pairs of an engram pair reach the model: the
/// embedding model the vectors come from, its floor and the per-pair limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineRules<'a> {
    /// The embedding model id the line vectors are stored under.
    pub embedding_model: &'a str,
    /// That model's line-similarity floor.
    pub floor: f64,
    /// [`crate::sweep::MAX_LINE_PAIRS_PER_ENGRAM_PAIR`].
    pub max_line_pairs: usize,
}

impl LineRules<'_> {
    /// The rules as one stable text, folded into the per-engram checksum and
    /// the walk digest, so a change of any of them rescores every pair once.
    pub fn key(&self) -> String {
        format!(
            "{}\u{0}{}\u{0}{}",
            self.embedding_model,
            self.floor.to_bits(),
            self.max_line_pairs
        )
    }
}

/// The per-engram checksum a scored pair is stored at: the observation digest
/// under the line rules. A changed line, a changed floor, limit or embedding
/// model each move it, so the pair is pending again and its rows are
/// replaced; a frontmatter or prose edit does not.
pub fn scoring_checksum(observations: &[FactObservation], rules: &LineRules<'_>) -> String {
    let mut h = Sha256::new();
    h.update(rules.key().as_bytes());
    h.update([0]);
    h.update(observations_digest(observations).as_bytes());
    crate::hex_lower(&h.finalize())
}

/// Every distinct observation line, folded and keyed by its row hash, of the
/// engrams that can take part in `V302` as of `today`: current, not
/// speculative, not past their `valid_to`, with observations (the candidate
/// rule's eligibility, without the lead vector). These are the lines the walk
/// keeps a vector for.
pub fn eligible_lines(facts: &[CandidateFacts<'_>], today: NaiveDate) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for f in facts
        .iter()
        .filter(|f| eligible(f.status, f.valid_to, today) && !f.observations.is_empty())
    {
        for o in f.observations {
            let folded = fold(&o.text);
            if folded.is_empty() {
                continue;
            }
            out.entry(observation_hash(&o.text)).or_insert(folded);
        }
    }
    out
}

/// The line pairs of one engram pair the model reads: every line of `a`
/// against every line of `b` by the cosine of their vectors, those at or
/// above `floor`, at most `limit`, the most similar first and document order
/// among equals. A line with no words is skipped, a repeated line inside one
/// engram counts once (its first occurrence), and two lines with the same
/// text are never a pair: a line does not contradict itself, and a copied
/// bullet would otherwise take a slot from two lines that differ.
///
/// `vectors` must come from the embedding model the rules name (the one
/// `floor` belongs to): a vector of another width reads as cosine 0, so the
/// pair would count as scored with no rows.
///
/// `None` when a line of either side has no vector in `vectors` yet: the pair
/// waits for it rather than reading as scored. Only the best `limit` are ever
/// held, so a long engram costs time, never memory.
pub fn similar_line_pairs(
    a: &[FactObservation],
    b: &[FactObservation],
    vectors: &HashMap<String, Vec<f32>>,
    floor: f64,
    limit: usize,
) -> Option<Vec<KeptLine>> {
    let sa = embedded_side(a, vectors)?;
    let sb = embedded_side(b, vectors)?;
    // (similarity, index into sa, index into sb), the best `limit` so far.
    let mut best: Vec<(f32, usize, usize)> = Vec::with_capacity(limit + 1);
    let order = |x: &(f32, usize, usize), y: &(f32, usize, usize)| {
        y.0.total_cmp(&x.0)
            .then_with(|| (x.1, x.2).cmp(&(y.1, y.2)))
    };
    for (x, (_, ha, va)) in sa.iter().enumerate() {
        for (y, (_, hb, vb)) in sb.iter().enumerate() {
            if ha == hb {
                continue;
            }
            let s = cosine(va, vb);
            if s.is_nan() || f64::from(s) < floor {
                continue;
            }
            best.push((s, x, y));
            best.sort_by(order);
            best.truncate(limit);
        }
    }
    Some(
        best.into_iter()
            .map(|(similarity, x, y)| KeptLine {
                ia: sa[x].0,
                ib: sb[y].0,
                hash_a: sa[x].1.clone(),
                hash_b: sb[y].1.clone(),
                similarity,
            })
            .collect(),
    )
}

/// One side's lines with words, deduplicated by hash in document order, each
/// with its index, hash and vector; `None` when one has no vector.
fn embedded_side<'v>(
    observations: &[FactObservation],
    vectors: &'v HashMap<String, Vec<f32>>,
) -> Option<Vec<(usize, String, &'v [f32])>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(observations.len());
    for (i, o) in observations.iter().enumerate() {
        if fold(&o.text).is_empty() {
            continue;
        }
        let hash = observation_hash(&o.text);
        if !seen.insert(hash.clone()) {
            continue;
        }
        let vector = vectors.get(&hash)?;
        out.push((i, hash, vector.as_slice()));
    }
    Some(out)
}

/// The cosine of two vectors; 0 for mismatched widths or a zero vector, so
/// neither is ever kept.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// The first reading order of each kept line pair, folded: `a`'s line as
/// premise.
pub fn first_order_inputs(
    a: &[FactObservation],
    b: &[FactObservation],
    lines: &[KeptLine],
) -> Vec<(String, String)> {
    lines
        .iter()
        .map(|l| (fold(&a[l.ia].text), fold(&b[l.ib].text)))
        .collect()
}

/// The second reading order of one kept line pair, folded: `b`'s line as
/// premise.
pub fn second_order_input(
    a: &[FactObservation],
    b: &[FactObservation],
    line: &KeptLine,
) -> (String, String) {
    (fold(&b[line.ib].text), fold(&a[line.ia].text))
}

/// Which line pairs need the second order, given the first: all of them
/// under `Mean`, those at or above [`CONTRADICTION_STORE_FLOOR`] under `Min`
/// (a NaN never is).
pub fn second_order_needed(first: &[f32], how: OrderAggregation) -> Vec<bool> {
    first
        .iter()
        .map(|p| !how.skips_second_order_below_floor() || *p >= CONTRADICTION_STORE_FLOOR)
        .collect()
}

/// The rows to store for one engram pair's kept line pairs, given both
/// orders (`second` is `None` where it was not read). Under `Min` a line pair
/// is stored when its first order reached [`CONTRADICTION_STORE_FLOOR`];
/// under `Mean` when either order did. Both orders, the line similarity and
/// the period hint are kept.
///
/// `None` when the answer is incomplete: `first` or `second` does not hold
/// exactly one entry per line pair, or a second order that
/// [`second_order_needed`] asks for is `None`. The caller then leaves the
/// pair pending; an incomplete answer never reads as a scored pair with no
/// rows. A second order that is not needed may be `None`.
#[allow(clippy::too_many_arguments)]
pub fn line_rows(
    a: EngramId,
    b: EngramId,
    obs_a: &[FactObservation],
    obs_b: &[FactObservation],
    lines: &[KeptLine],
    first: &[f32],
    second: &[Option<f32>],
    how: OrderAggregation,
) -> Option<Vec<ContradictionRow>> {
    // One score per order per line pair; a short answer would zip away the
    // tail silently and store the pair as scored, in release builds too.
    if first.len() != lines.len() || second.len() != lines.len() {
        return None;
    }
    let needed = second_order_needed(first, how);
    if needed.iter().zip(second).any(|(n, ba)| *n && ba.is_none()) {
        return None;
    }
    Some(
        lines
            .iter()
            .zip(first)
            .zip(second)
            .filter_map(|((l, &ab), ba)| {
                let ba = (*ba)?;
                let stored = if how.skips_second_order_below_floor() {
                    ab >= CONTRADICTION_STORE_FLOOR
                } else {
                    ab >= CONTRADICTION_STORE_FLOOR || ba >= CONTRADICTION_STORE_FLOOR
                };
                stored.then(|| ContradictionRow {
                    a,
                    b,
                    line_a: obs_a[l.ia].line,
                    line_b: obs_b[l.ib].line,
                    hash_a: l.hash_a.clone(),
                    hash_b: l.hash_b.clone(),
                    score_ab: ab,
                    score_ba: ba,
                    similarity: l.similarity,
                    period: names_period(&obs_a[l.ia].text) || names_period(&obs_b[l.ib].text),
                })
            })
            .collect(),
    )
}

/// The indices of `inputs` sorted by the byte length of premise plus
/// hypothesis, ties in input order, so a batch pads to a near neighbour
/// rather than to the longest line of the pass. Byte length is enough:
/// the loader cuts every line to `MAX_LINE_TOKENS` anyway, which also bounds
/// the last, longest batch (lesson 3 is about unbounded chunks).
pub fn length_order(inputs: &[(String, String)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..inputs.len()).collect();
    order.sort_by_key(|&i| inputs[i].0.len() + inputs[i].1.len());
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep::MAX_LINE_PAIRS_PER_ENGRAM_PAIR;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn obs(line: usize, text: &str) -> FactObservation {
        FactObservation {
            line,
            text: text.to_string(),
        }
    }

    /// The digest follows what the model reads: the folded texts in order.
    /// Line numbers (a line moved down by prose above it), whitespace runs
    /// and wordless lines are not part of it; a changed or reordered text is.
    #[test]
    fn the_observations_digest_follows_the_folded_texts_in_order() {
        let base = observations_digest(&[obs(5, "Node 18"), obs(6, "Retries back off")]);
        assert_eq!(
            base,
            observations_digest(&[
                obs(9, "Node  18 "),
                obs(10, " "),
                obs(11, "Retries back off")
            ])
        );
        assert_ne!(
            base,
            observations_digest(&[obs(5, "Node 20"), obs(6, "Retries back off")])
        );
        assert_ne!(
            base,
            observations_digest(&[obs(5, "Retries back off"), obs(6, "Node 18")])
        );
        assert_ne!(
            base,
            observations_digest(&[obs(5, "Node 18Retries back off")])
        );
    }

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    /// An owned fact, so the borrowed view the functions take has something
    /// to borrow from.
    struct Owned {
        id: i64,
        status: String,
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
        checksum: String,
        vector: Option<Vec<f32>>,
        obs: Vec<FactObservation>,
    }

    impl Owned {
        fn view(&self) -> CandidateFacts<'_> {
            CandidateFacts {
                id: EngramId(self.id),
                status: &self.status,
                valid_from: self.from,
                valid_to: self.to,
                checksum: &self.checksum,
                lead_vector: self.vector.as_deref(),
                observations: &self.obs,
            }
        }
    }

    fn owned(id: i64, v: &[f32]) -> Owned {
        Owned {
            id,
            status: "stable".to_string(),
            from: None,
            to: None,
            checksum: format!("c{id}"),
            vector: Some(unit(v)),
            obs: vec![obs(5, &format!("fact {id}"))],
        }
    }

    fn views(all: &[Owned]) -> Vec<CandidateFacts<'_>> {
        all.iter().map(Owned::view).collect()
    }

    /// A date before every window in these tests, so only the status and
    /// window rules decide.
    fn long_ago() -> NaiveDate {
        day("2000-01-01")
    }

    /// Task 5b: past its `valid_to` is out, ending today is in, no
    /// `valid_to` never expires, and the status rule still holds.
    #[test]
    fn eligibility_reads_the_status_and_whether_the_window_closed_before_today() {
        let today = day("2026-10-01");
        assert!(eligible("stable", None, today));
        assert!(eligible("stable", Some(day("2026-10-01")), today));
        assert!(eligible("current", Some(day("2027-01-01")), today));
        assert!(!eligible("stable", Some(day("2026-09-30")), today));
        assert!(!eligible("draft", None, today));
        assert!(!eligible("superseded", None, today));
        assert!(expired(Some(day("2026-09-30")), today));
        assert!(!expired(Some(today), today));
        assert!(!expired(None, today));
    }

    #[test]
    fn an_expired_engram_is_neither_a_candidate_nor_a_line_to_embed() {
        let mut expired = owned(1, &[1.0, 0.0]);
        expired.to = Some(day("2026-09-30"));
        expired.obs = vec![obs(1, "Expired line")];
        let mut ends_today = owned(2, &[1.0, 0.0]);
        ends_today.to = Some(day("2026-10-01"));
        ends_today.obs = vec![obs(1, "Ends today")];
        let mut open = owned(3, &[1.0, 0.0]);
        open.obs = vec![obs(1, "Open")];
        let all = vec![expired, ends_today, open];
        let today = day("2026-10-01");
        let found = contradiction_candidates(&views(&all), 0.80, 2000, today);
        let ids: Vec<(i64, i64)> = found
            .pairs
            .iter()
            .map(|p| (all[p.a].id, all[p.b].id))
            .collect();
        assert_eq!(ids, vec![(2, 3)]);
        let lines = eligible_lines(&views(&all), today);
        let texts: Vec<&str> = lines.values().map(String::as_str).collect();
        assert_eq!(texts.len(), 2);
        assert!(!texts.contains(&"Expired line"), "{texts:?}");
        // The same engrams a day earlier: all three take part.
        let before = contradiction_candidates(&views(&all), 0.80, 2000, day("2026-09-30"));
        assert_eq!(before.pairs.len(), 3);
    }

    #[test]
    fn windows_overlap_treats_absent_bounds_as_unbounded() {
        assert!(windows_overlap(None, None, None, None));
        assert!(windows_overlap(
            Some(day("2026-01-01")),
            None,
            None,
            Some(day("2026-01-01"))
        ));
        assert!(!windows_overlap(
            None,
            Some(day("2026-01-01")),
            Some(day("2026-01-02")),
            None
        ));
        assert!(windows_overlap(
            Some(day("2026-01-01")),
            Some(day("2026-02-01")),
            Some(day("2026-02-01")),
            Some(day("2026-03-01"))
        ));
    }

    #[test]
    fn candidates_take_related_eligible_overlapping_pairs_only() {
        let a = owned(1, &[1.0, 0.0, 0.0]);
        let b = owned(2, &[0.95, 0.3, 0.0]); // cosine about 0.95
        let mut c = owned(3, &[0.95, 0.3, 0.0]);
        c.status = "draft".to_string(); // speculative
        let mut d = owned(4, &[0.95, 0.3, 0.0]);
        d.to = Some(day("2025-12-31"));
        let mut e = owned(5, &[1.0, 0.0, 0.0]);
        e.from = Some(day("2026-01-01")); // opens after d closed
        let far = owned(6, &[0.0, 1.0, 0.0]); // cosine 0
        let mut unembedded = owned(7, &[0.95, 0.3, 0.0]);
        unembedded.vector = None;
        let mut retired = owned(8, &[1.0, 0.0, 0.0]);
        retired.status = "superseded".to_string();
        let all = vec![a, b, c, d, e, far, unembedded, retired];
        let found = contradiction_candidates(&views(&all), 0.80, 2000, long_ago());
        let ids: Vec<(i64, i64)> = found
            .pairs
            .iter()
            .map(|p| (all[p.a].id, all[p.b].id))
            .collect();
        assert!(ids.contains(&(1, 2)), "{ids:?}");
        assert!(
            ids.contains(&(1, 4)),
            "an unbounded window overlaps a closed one"
        );
        assert!(ids.contains(&(1, 5)));
        assert!(!ids.contains(&(4, 5)), "d closed before e opened");
        for skipped in [3, 6, 7, 8] {
            assert!(
                !ids.iter().any(|(x, y)| *x == skipped || *y == skipped),
                "{skipped}: {ids:?}"
            );
        }
        assert!(
            found.pairs.iter().all(|p| all[p.a].id < all[p.b].id),
            "a is the lower id"
        );
        assert!(
            found.pairs.windows(2).all(|w| w[0].cosine >= w[1].cosine),
            "highest cosine first"
        );
        assert!(!found.capped && !found.full);
    }

    /// Review focus 2: an engram with nothing to pair contributes nothing,
    /// so it can never sit in the pending set and keep the worker asking.
    #[test]
    fn an_engram_without_observations_is_never_a_candidate() {
        let a = owned(1, &[1.0, 0.0]);
        let mut silent = owned(2, &[1.0, 0.0]);
        silent.obs.clear();
        let all = vec![a, silent];
        assert!(
            contradiction_candidates(&views(&all), 0.80, 2000, long_ago())
                .pairs
                .is_empty()
        );
    }

    #[test]
    fn the_kept_list_is_capped_and_says_so() {
        let all: Vec<Owned> = (1..=4).map(|i| owned(i, &[1.0, 0.0])).collect();
        let found = contradiction_candidates(&views(&all), 0.80, 3, long_ago());
        assert_eq!(found.pairs.len(), 3);
        assert!(found.full, "six related pairs, three kept");
    }

    #[test]
    fn pending_is_the_candidates_without_a_matching_scored_row() {
        let all = vec![
            owned(1, &[1.0, 0.0]),
            owned(2, &[1.0, 0.0]),
            owned(3, &[1.0, 0.0]),
        ];
        let v = views(&all);
        let found = contradiction_candidates(&v, 0.80, 2000, long_ago());
        assert_eq!(found.pairs.len(), 3);
        let scored = vec![
            ScoredPair {
                a: EngramId(1),
                b: EngramId(2),
                checksum_a: "c1".into(),
                checksum_b: "c2".into(),
            },
            ScoredPair {
                a: EngramId(1),
                b: EngramId(3),
                checksum_a: "old".into(),
                checksum_b: "c3".into(),
            },
        ];
        let left = pending_pairs(&v, &found.pairs, &scored);
        let mut ids: Vec<(i64, i64)> = left.iter().map(|p| (all[p.a].id, all[p.b].id)).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![(1, 3), (2, 3)],
            "an edited checksum re-queues; an unscored pair is pending"
        );
        assert_eq!(
            pending_pairs(&v, &found.pairs, &[]).len(),
            3,
            "another model's rows are not passed in, so everything is pending"
        );
    }

    #[test]
    fn the_row_hash_survives_whitespace_and_never_the_words() {
        assert_eq!(
            observation_hash("The build  uses\tNode 18 "),
            observation_hash("The build uses Node 18")
        );
        assert_ne!(
            observation_hash("The build uses Node 18"),
            observation_hash("The build uses Node 20")
        );
        assert_eq!(observation_hash("x").len(), 64);
    }

    #[test]
    fn the_measurement_overrides_parse_or_fall_back() {
        assert_eq!(related_from(None), RELATED_THRESHOLD);
        assert_eq!(related_from(Some("0.70")), 0.70);
        assert_eq!(
            related_from(Some("1.5")),
            RELATED_THRESHOLD,
            "out of range is ignored"
        );
        assert_eq!(related_from(Some("x")), RELATED_THRESHOLD);
        assert_eq!(max_pairs_from(None), MAX_RELATED_PAIRS);
        assert_eq!(max_pairs_from(Some("1000000")), 1_000_000);
        assert_eq!(max_pairs_from(Some("0")), MAX_RELATED_PAIRS);
    }
    /// Exactly `max_pairs` related pairs is a complete list, not a full one:
    /// `full` says pairs were dropped, never that the cap was merely met.
    #[test]
    fn exactly_max_pairs_is_not_full() {
        let all: Vec<Owned> = (1..=3).map(|i| owned(i, &[1.0, 0.0])).collect();
        let found = contradiction_candidates(&views(&all), 0.80, 3, long_ago());
        assert_eq!(found.pairs.len(), 3);
        assert!(!found.full, "three related pairs, three kept, none dropped");
    }

    /// The window rule applies before the cap: a closer pair whose windows
    /// never meet takes no slot, so it can neither evict an overlapping pair
    /// nor make the list read as full.
    #[test]
    fn a_non_overlapping_pair_takes_no_slot_under_the_cap() {
        let mut early = owned(1, &[1.0, 0.0]);
        early.to = Some(day("2025-12-31"));
        let mut late = owned(2, &[1.0, 0.0]);
        late.from = Some(day("2026-01-01"));
        let near = owned(3, &[0.9, 0.436]); // cosine about 0.9 to both
        let all = vec![early, late, near];
        let found = contradiction_candidates(&views(&all), 0.80, 2, long_ago());
        let mut ids: Vec<(i64, i64)> = found
            .pairs
            .iter()
            .map(|p| (all[p.a].id, all[p.b].id))
            .collect();
        ids.sort();
        assert_eq!(ids, vec![(1, 3), (2, 3)], "both overlapping pairs are kept");
        assert!(!found.full, "nothing that overlaps was dropped");
        let one = contradiction_candidates(&views(&all), 0.80, 1, long_ago());
        assert_eq!(one.pairs.len(), 1);
        assert!(one.full, "two overlapping pairs, one kept");
    }

    fn vectors_for(lines: &[(&str, &[f32])]) -> HashMap<String, Vec<f32>> {
        lines
            .iter()
            .map(|(text, v)| (observation_hash(text), unit(v)))
            .collect()
    }

    #[test]
    fn only_lines_at_or_above_the_floor_are_kept_most_similar_first() {
        let a = vec![
            obs(3, "Staging deploys need a manual trigger"),
            obs(4, "Logs go to Loki"),
        ];
        let b = vec![
            obs(7, "Staging applies without manual approval"),
            obs(8, "Grafana reads Loki"),
            obs(9, "The cafeteria opens at eight"),
        ];
        let v = vectors_for(&[
            ("Staging deploys need a manual trigger", &[1.0, 0.0, 0.0]),
            ("Logs go to Loki", &[0.0, 1.0, 0.0]),
            (
                "Staging applies without manual approval",
                &[0.95, 0.31, 0.0],
            ),
            ("Grafana reads Loki", &[0.1, 0.99, 0.0]),
            ("The cafeteria opens at eight", &[0.0, 0.0, 1.0]),
        ]);
        let kept = similar_line_pairs(&a, &b, &v, 0.86, 4).expect("every line has a vector");
        let at: Vec<(usize, usize)> = kept.iter().map(|k| (a[k.ia].line, b[k.ib].line)).collect();
        assert_eq!(
            at,
            vec![(4, 8), (3, 7)],
            "most similar first, the unrelated line never"
        );
        assert!(kept.windows(2).all(|w| w[0].similarity >= w[1].similarity));
        assert!(kept.iter().all(|k| f64::from(k.similarity) >= 0.86));
        assert_eq!(
            kept[1].hash_a,
            observation_hash("Staging deploys need a manual trigger")
        );
        assert!(
            similar_line_pairs(&a, &b, &v, 0.999, 4).unwrap().is_empty(),
            "nothing at the floor is a scored pair with no rows, not a missing one"
        );
    }

    #[test]
    fn at_most_the_limit_is_kept_and_ties_keep_document_order() {
        let a: Vec<FactObservation> = (1..=3).map(|i| obs(i, &format!("a{i}"))).collect();
        let b: Vec<FactObservation> = (1..=3).map(|i| obs(10 + i, &format!("b{i}"))).collect();
        let same: Vec<(String, Vec<f32>)> = a
            .iter()
            .chain(&b)
            .map(|o| (observation_hash(&o.text), vec![1.0, 0.0]))
            .collect();
        let v: HashMap<String, Vec<f32>> = same.into_iter().collect();
        let kept = similar_line_pairs(&a, &b, &v, 0.86, MAX_LINE_PAIRS_PER_ENGRAM_PAIR).unwrap();
        assert_eq!(kept.len(), 4, "nine pairs at 1.0, four kept");
        let at: Vec<(usize, usize)> = kept.iter().map(|k| (k.ia, k.ib)).collect();
        assert_eq!(at, vec![(0, 0), (0, 1), (0, 2), (1, 0)]);
    }

    /// Review focus 2: every line of a long engram takes part, the cut is the
    /// limit and not a window of the first eight lines.
    #[test]
    fn a_long_engram_finds_its_match_beyond_the_old_window_and_keeps_four() {
        let a: Vec<FactObservation> = (1..=150)
            .map(|i| obs(i, &format!("filler line {i}")))
            .collect();
        let b = vec![obs(5, "The target subject line")];
        let mut v: HashMap<String, Vec<f32>> = a
            .iter()
            .map(|o| (observation_hash(&o.text), unit(&[0.0, 1.0])))
            .collect();
        v.insert(observation_hash("filler line 120"), unit(&[1.0, 0.05]));
        v.insert(
            observation_hash("The target subject line"),
            unit(&[1.0, 0.0]),
        );
        let kept = similar_line_pairs(&a, &b, &v, 0.86, 4).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(a[kept[0].ia].line, 120);
    }

    /// Review focus 1: a line never contradicts itself, so the same text on
    /// both sides takes no slot from two lines that differ.
    #[test]
    fn identical_lines_never_take_a_slot() {
        // Each shared bullet on an axis of its own, so two different shared
        // bullets are unrelated and only a bullet against its own copy (the
        // same text) would sit at 1.0.
        let axis = |i: usize| -> Vec<f32> {
            let mut v = vec![0.0f32; 7];
            v[i] = 1.0;
            v
        };
        let shared: Vec<FactObservation> = (1..=5)
            .map(|i| obs(i, &format!("shared bullet {i}")))
            .collect();
        let mut a = shared.clone();
        a.push(obs(9, "The build uses Node 18"));
        let mut b = shared.clone();
        b.push(obs(9, "The build uses Node 20"));
        let mut v: HashMap<String, Vec<f32>> = shared
            .iter()
            .enumerate()
            .map(|(i, o)| (observation_hash(&o.text), axis(i)))
            .collect();
        v.insert(observation_hash("The build uses Node 18"), axis(5));
        let mut node_20 = axis(5);
        node_20[6] = 0.1;
        v.insert(observation_hash("The build uses Node 20"), unit(&node_20));
        let kept = similar_line_pairs(&a, &b, &v, 0.86, 4).unwrap();
        assert!(kept.iter().all(|k| k.hash_a != k.hash_b), "{kept:?}");
        assert!(
            kept.iter().any(|k| a[k.ia].line == 9 && b[k.ib].line == 9),
            "the two lines that differ are scored: {kept:?}"
        );
    }

    /// A copied bullet inside one engram is one line: the pair keys would
    /// collide on the primary key otherwise.
    #[test]
    fn a_repeated_line_inside_one_engram_is_paired_once() {
        let a = vec![
            obs(5, "The build uses Node 18"),
            obs(9, "The  build uses Node 18"),
        ];
        let b = vec![obs(3, "The build uses Node 20")];
        let v = vectors_for(&[
            ("The build uses Node 18", &[1.0, 0.0]),
            ("The build uses Node 20", &[0.99, 0.1]),
        ]);
        let kept = similar_line_pairs(&a, &b, &v, 0.86, 4).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            a[kept[0].ia].line, 5,
            "the first occurrence in document order"
        );
        let blank = vec![obs(4, "   ")];
        assert!(
            similar_line_pairs(&blank, &b, &v, 0.86, 4)
                .unwrap()
                .is_empty()
        );
    }

    /// A line with no vector yet makes the pair wait, never read as clean.
    #[test]
    fn a_missing_vector_makes_the_pair_wait() {
        let a = vec![obs(1, "The build uses Node 18")];
        let b = vec![obs(2, "The build uses Node 20"), obs(3, "Not embedded yet")];
        let v = vectors_for(&[
            ("The build uses Node 18", &[1.0, 0.0]),
            ("The build uses Node 20", &[1.0, 0.0]),
        ]);
        assert_eq!(similar_line_pairs(&a, &b, &v, 0.86, 4), None);
    }

    #[test]
    fn eligible_lines_are_the_distinct_folded_lines_of_engrams_that_can_take_part() {
        let mut one = owned(1, &[1.0, 0.0]);
        one.obs = vec![obs(3, "Node  18"), obs(4, " "), obs(5, "Shared")];
        let mut two = owned(2, &[1.0, 0.0]);
        two.obs = vec![obs(7, "Shared")];
        let mut draft = owned(3, &[1.0, 0.0]);
        draft.status = "draft".to_string();
        draft.obs = vec![obs(1, "Speculative")];
        let mut unembedded = owned(4, &[1.0, 0.0]);
        unembedded.vector = None;
        unembedded.obs = vec![obs(1, "No lead vector yet")];
        let all = vec![one, two, draft, unembedded];
        let lines = eligible_lines(&views(&all), long_ago());
        let texts: Vec<&str> = lines.values().map(String::as_str).collect();
        assert_eq!(lines.len(), 3, "{texts:?}");
        assert_eq!(lines[&observation_hash("Node 18")], "Node 18", "folded");
        assert!(
            lines.contains_key(&observation_hash("No lead vector yet")),
            "eligibility is status and lines, not the lead vector"
        );
        assert!(!lines.contains_key(&observation_hash("Speculative")));
    }

    #[test]
    fn the_scoring_checksum_moves_with_the_rules() {
        let o = vec![obs(5, "Node 18")];
        let rules = LineRules {
            embedding_model: "granite",
            floor: 0.86,
            max_line_pairs: 4,
        };
        let base = scoring_checksum(&o, &rules);
        assert_eq!(
            base,
            scoring_checksum(&[obs(9, "Node  18")], &rules),
            "lines and spaces do not move it"
        );
        assert_ne!(
            base,
            scoring_checksum(
                &o,
                &LineRules {
                    floor: 0.88,
                    ..rules
                }
            )
        );
        assert_ne!(
            base,
            scoring_checksum(
                &o,
                &LineRules {
                    max_line_pairs: 5,
                    ..rules
                }
            )
        );
        assert_ne!(
            base,
            scoring_checksum(
                &o,
                &LineRules {
                    embedding_model: "bge",
                    ..rules
                }
            )
        );
        assert_ne!(base, scoring_checksum(&[obs(5, "Node 20")], &rules));
        assert_ne!(
            base,
            observations_digest(&o),
            "never the plain digest a pair stored before"
        );
    }

    fn kept(
        ia: usize,
        ib: usize,
        a: &[FactObservation],
        b: &[FactObservation],
        s: f32,
    ) -> KeptLine {
        KeptLine {
            ia,
            ib,
            hash_a: observation_hash(&a[ia].text),
            hash_b: observation_hash(&b[ib].text),
            similarity: s,
        }
    }

    #[test]
    fn the_inputs_are_the_folded_lines_a_first_then_b_first() {
        let a = vec![obs(1, "Node  18")];
        let b = vec![obs(2, "Node 20")];
        let lines = vec![kept(0, 0, &a, &b, 0.9)];
        assert_eq!(
            first_order_inputs(&a, &b, &lines),
            vec![("Node 18".to_string(), "Node 20".to_string())]
        );
        assert_eq!(
            second_order_input(&a, &b, &lines[0]),
            ("Node 20".to_string(), "Node 18".to_string())
        );
    }

    #[test]
    fn the_second_order_follows_the_aggregation() {
        let first = [0.2, 0.5, 0.93, f32::NAN];
        assert_eq!(
            second_order_needed(&first, OrderAggregation::Min),
            vec![false, true, true, false],
            "under Min only a first order at or above the store floor can fire"
        );
        assert_eq!(
            second_order_needed(&first, OrderAggregation::Mean),
            vec![true; 4]
        );

        let a = vec![
            obs(1, "Since 2024 the build uses Node 18"),
            obs(2, "Deploys run on Fridays"),
        ];
        let b = vec![obs(7, "The build uses Node 20")];
        let lines = vec![kept(0, 0, &a, &b, 0.91), kept(1, 0, &a, &b, 0.87)];
        // Min: the first order decides; the second line pair's first order
        // is below the floor, so it was never read the other way.
        let rows = line_rows(
            EngramId(1),
            EngramId(2),
            &a,
            &b,
            &lines,
            &[0.9, 0.3],
            &[Some(0.4), None],
            OrderAggregation::Min,
        )
        .expect("the one needed second order was read");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!((r.line_a, r.line_b), (1, 7));
        assert_eq!((r.score_ab, r.score_ba, r.similarity), (0.9, 0.4, 0.91));
        assert!(r.period, "a line naming a year sets the hint");
        // Mean: both orders were read, either one at the floor stores.
        let rows = line_rows(
            EngramId(1),
            EngramId(2),
            &a,
            &b,
            &lines,
            &[0.3, 0.2],
            &[Some(0.6), Some(0.1)],
            OrderAggregation::Mean,
        )
        .expect("both orders were read");
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].score_ab, rows[0].score_ba), (0.3, 0.6));
    }

    /// A reading order the aggregation needs and nobody read keeps the pair
    /// pending: it must never come back as a clean pair with no rows.
    #[test]
    fn a_missing_needed_order_or_a_short_answer_keeps_the_pair_pending() {
        let a = vec![
            obs(1, "The build uses Node 18"),
            obs(2, "Deploys run on Fridays"),
        ];
        let b = vec![obs(7, "The build uses Node 20")];
        let lines = vec![kept(0, 0, &a, &b, 0.91), kept(1, 0, &a, &b, 0.87)];
        let rows = |first: &[f32], second: &[Option<f32>], how| {
            line_rows(EngramId(1), EngramId(2), &a, &b, &lines, first, second, how)
        };
        // Min, first order at the floor, second missing.
        assert_eq!(
            rows(&[0.5, 0.3], &[None, None], OrderAggregation::Min),
            None
        );
        // Mean needs both orders of every line pair.
        assert_eq!(
            rows(&[0.3, 0.2], &[Some(0.6), None], OrderAggregation::Mean),
            None
        );
        // A short answer in either order.
        assert_eq!(
            rows(&[0.9], &[Some(0.9), None], OrderAggregation::Min),
            None
        );
        assert_eq!(rows(&[0.9, 0.3], &[Some(0.9)], OrderAggregation::Min), None);
        assert_eq!(
            rows(
                &[0.9, 0.3, 0.1],
                &[Some(0.9), None, None],
                OrderAggregation::Min
            ),
            None
        );
        // Not needed and not read: the pair is scored, with the rows it earns.
        assert_eq!(
            rows(&[0.3, 0.2], &[None, None], OrderAggregation::Min),
            Some(Vec::new())
        );
        let stored = rows(&[0.2, 0.7], &[None, Some(0.8)], OrderAggregation::Min)
            .expect("the only needed order was read");
        assert_eq!(stored.len(), 1);
        assert_eq!((stored[0].line_a, stored[0].line_b), (2, 7));
        // A NaN first order is never needed under Min.
        assert_eq!(
            rows(&[f32::NAN, 0.2], &[None, None], OrderAggregation::Min),
            Some(Vec::new())
        );
    }

    #[test]
    fn length_order_sorts_by_length_and_keeps_ties_in_input_order() {
        let inputs = vec![
            (
                "a much longer premise".to_string(),
                "and hypothesis".to_string(),
            ),
            ("short".to_string(), "one".to_string()),
            ("tie".to_string(), "abc".to_string()),
            ("mid length".to_string(), "text".to_string()),
            ("abc".to_string(), "tie".to_string()),
        ];
        assert_eq!(length_order(&inputs), vec![2, 4, 1, 3, 0]);
        assert!(length_order(&[]).is_empty());
    }
}
