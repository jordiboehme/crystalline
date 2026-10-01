//! Which engram pairs the contradiction check scores and how their
//! observation lines are paired. Pure: the engine assembles the facts from the
//! base listing, the files and the stamps, and hands them in borrowed.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use chrono::NaiveDate;
use sha2::{Digest, Sha256};

use super::period::names_period;
use crate::store::{ContradictionRow, EngramId, ScoredPair, is_current_status};
use crate::sweep::twins::find_twins_where;
use crate::sweep::{
    CONTRADICTION_STORE_FLOOR, FactObservation, MAX_LINE_PAIRS_PER_PAIR, MAX_RELATED_PAIRS,
    RELATED_THRESHOLD, SPECULATIVE_STATUSES, SweepOptions,
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

/// One observation line of `a` against one of `b`, with both row hashes.
#[derive(Debug, Clone, PartialEq)]
pub struct LinePair<'a> {
    /// The line from the lower-id engram.
    pub a: &'a FactObservation,
    /// The line from the higher-id engram.
    pub b: &'a FactObservation,
    /// [`observation_hash`] of `a`'s text.
    pub hash_a: String,
    /// [`observation_hash`] of `b`'s text.
    pub hash_b: String,
}

/// Current and not speculative: the statuses a pair may be scored in.
pub fn eligible(status: &str) -> bool {
    is_current_status(status) && !SPECULATIVE_STATUSES.contains(&status)
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

/// Every eligible, window-overlapping pair of engrams with observations whose
/// lead vectors sit at or above `related_threshold`, at most `max_pairs`,
/// highest cosine first. The twin finder's all-pairs walk at a lower line, so
/// the vector cap and the bounded retention behave as `V301`'s do.
pub fn contradiction_candidates(
    facts: &[CandidateFacts<'_>],
    related_threshold: f64,
    max_pairs: usize,
) -> Candidates {
    let vectors: Vec<Option<&[f32]>> = facts
        .iter()
        .map(|f| {
            if eligible(f.status) && !f.observations.is_empty() {
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

/// The line pairs of one engram pair, in document order, at most
/// [`MAX_LINE_PAIRS_PER_PAIR`]: the first eight of each side when both exceed
/// eight, otherwise every line of the short side against as many of the long
/// side as the cap allows. A line with no words is skipped, and a repeated
/// hash pair (a copied bullet) is kept once, the first occurrence.
pub fn line_pairs<'a>(a: &'a [FactObservation], b: &'a [FactObservation]) -> Vec<LinePair<'a>> {
    let words = |o: &&FactObservation| !fold(&o.text).is_empty();
    let a: Vec<&FactObservation> = a.iter().filter(words).collect();
    let b: Vec<&FactObservation> = b.iter().filter(words).collect();
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let side = 8usize;
    let (na, nb) = if a.len() <= side {
        (a.len(), b.len().min(MAX_LINE_PAIRS_PER_PAIR / a.len()))
    } else if b.len() <= side {
        (a.len().min(MAX_LINE_PAIRS_PER_PAIR / b.len()), b.len())
    } else {
        (side, side)
    };
    let hashes_a: Vec<String> = a[..na].iter().map(|o| observation_hash(&o.text)).collect();
    let hashes_b: Vec<String> = b[..nb].iter().map(|o| observation_hash(&o.text)).collect();
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut out = Vec::with_capacity(na * nb);
    for (x, hx) in a[..na].iter().zip(&hashes_a) {
        for (y, hy) in b[..nb].iter().zip(&hashes_b) {
            if seen.insert((hx.as_str(), hy.as_str())) {
                out.push(LinePair {
                    a: x,
                    b: y,
                    hash_a: hx.clone(),
                    hash_b: hy.clone(),
                });
            }
        }
    }
    out
}

/// The scorer input for `lines`: each pair in both orders, `a` as premise
/// first, folded.
pub fn scorer_inputs(lines: &[LinePair<'_>]) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(lines.len() * 2);
    for lp in lines {
        let (x, y) = (fold(&lp.a.text), fold(&lp.b.text));
        out.push((x.clone(), y.clone()));
        out.push((y, x));
    }
    out
}

/// The rows to store for `lines`, given the scorer's probabilities in
/// [`scorer_inputs`] order: every line pair whose higher order is at or above
/// [`CONTRADICTION_STORE_FLOOR`], both orders kept, with the period hint.
pub fn score_rows(
    a: EngramId,
    b: EngramId,
    lines: &[LinePair<'_>],
    probabilities: &[f32],
) -> Vec<ContradictionRow> {
    // Two scores per line pair, one per reading order. A short answer would
    // zip away the tail silently and store the pair as scored.
    debug_assert_eq!(probabilities.len(), lines.len() * 2);
    lines
        .iter()
        .zip(probabilities.as_chunks::<2>().0)
        .filter(|(_, [ab, ba])| ab.max(*ba) >= CONTRADICTION_STORE_FLOOR)
        .map(|(lp, &[ab, ba])| ContradictionRow {
            a,
            b,
            line_a: lp.a.line,
            line_b: lp.b.line,
            hash_a: lp.hash_a.clone(),
            hash_b: lp.hash_b.clone(),
            score_ab: ab,
            score_ba: ba,
            similarity: 0.0,
            period: names_period(&lp.a.text) || names_period(&lp.b.text),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let found = contradiction_candidates(&views(&all), 0.80, 2000);
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
            contradiction_candidates(&views(&all), 0.80, 2000)
                .pairs
                .is_empty()
        );
    }

    #[test]
    fn the_kept_list_is_capped_and_says_so() {
        let all: Vec<Owned> = (1..=4).map(|i| owned(i, &[1.0, 0.0])).collect();
        let found = contradiction_candidates(&views(&all), 0.80, 3);
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
        let found = contradiction_candidates(&v, 0.80, 2000);
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
    fn line_pairs_are_capped_at_the_first_eight_of_each_side() {
        let a: Vec<FactObservation> = (1..=12).map(|i| obs(i, &format!("a{i}"))).collect();
        let b: Vec<FactObservation> = (1..=12).map(|i| obs(i, &format!("b{i}"))).collect();
        let pairs = line_pairs(&a, &b);
        assert_eq!(pairs.len(), MAX_LINE_PAIRS_PER_PAIR);
        assert!(pairs.iter().all(|p| p.a.line <= 8 && p.b.line <= 8));
        assert_eq!((pairs[0].a.line, pairs[0].b.line), (1, 1));
        assert_eq!((pairs[63].a.line, pairs[63].b.line), (8, 8));
        assert_eq!(
            line_pairs(&a[..3], &b).len(),
            36,
            "three against twelve all fit"
        );
        let long: Vec<FactObservation> = (1..=40).map(|i| obs(i, &format!("b{i}"))).collect();
        assert_eq!(
            line_pairs(&a[..2], &long).len(),
            64,
            "the long side is cut at 32"
        );
        assert!(line_pairs(&[], &b).is_empty());
    }

    /// Review focus 1: a copied bullet would collide on the table's primary
    /// key and wedge the pair; it is scored once.
    #[test]
    fn line_pairs_dedupe_a_repeated_line_by_its_hash_pair() {
        let a = vec![
            obs(5, "The build uses Node 18"),
            obs(9, "The  build uses Node 18"),
        ];
        let b = vec![obs(3, "The build uses Node 20")];
        let pairs = line_pairs(&a, &b);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].a.line, 5, "the first occurrence in document order");
        let blank = vec![obs(4, "   ")];
        assert!(
            line_pairs(&blank, &b).is_empty(),
            "an observation with no words is no line"
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
    fn scorer_inputs_are_both_orders_folded() {
        let a = vec![obs(1, "Node  18")];
        let b = vec![obs(2, "Node 20")];
        let lines = line_pairs(&a, &b);
        assert_eq!(
            scorer_inputs(&lines),
            vec![
                ("Node 18".to_string(), "Node 20".to_string()),
                ("Node 20".to_string(), "Node 18".to_string()),
            ]
        );
    }

    #[test]
    fn score_rows_keep_the_higher_order_above_the_floor_and_mark_periods() {
        let a = vec![
            obs(1, "Since 2024 the build uses Node 18"),
            obs(2, "Deploys run on Fridays"),
        ];
        let b = vec![obs(7, "The build uses Node 20")];
        let lines = line_pairs(&a, &b);
        let rows = score_rows(EngramId(1), EngramId(2), &lines, &[0.2, 0.9, 0.4, 0.3]);
        assert_eq!(
            rows.len(),
            1,
            "only the first line pair clears the floor in either order"
        );
        let r = &rows[0];
        assert_eq!((r.line_a, r.line_b), (1, 7));
        assert_eq!((r.score_ab, r.score_ba), (0.2, 0.9));
        assert!(r.period, "a line naming a year sets the hint");
        assert_eq!(
            r.hash_a,
            observation_hash("Since 2024 the build uses Node 18")
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "assertion `left == right` failed")]
    fn score_rows_refuses_a_short_answer_in_a_debug_build() {
        let a = vec![obs(1, "The build uses Node 18")];
        let b = vec![obs(7, "The build uses Node 20")];
        let lines = line_pairs(&a, &b);
        score_rows(EngramId(1), EngramId(2), &lines, &[0.9]);
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
        let found = contradiction_candidates(&views(&all), 0.80, 3);
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
        let found = contradiction_candidates(&views(&all), 0.80, 2);
        let mut ids: Vec<(i64, i64)> = found
            .pairs
            .iter()
            .map(|p| (all[p.a].id, all[p.b].id))
            .collect();
        ids.sort();
        assert_eq!(ids, vec![(1, 3), (2, 3)], "both overlapping pairs are kept");
        assert!(!found.full, "nothing that overlaps was dropped");
        let one = contradiction_candidates(&views(&all), 0.80, 1);
        assert_eq!(one.pairs.len(), 1);
        assert!(one.full, "two overlapping pairs, one kept");
    }
}
