//! `V302`: two observation lines a local NLI model read as a possible
//! contradiction. The rows are the daemon's; this module reads them and
//! re-filters them against what the engrams say now. A sweep scores nothing.

use std::collections::HashMap;

use super::{
    Class, Counterpart, EngramFacts, Finding, FindingCap, SweepInput, SweepReport, leader,
};
use crate::nli::candidates::{eligible, max_related_pairs, observation_hash, windows_overlap};
use crate::store::ContradictionRow;

/// The fix every `V302` row carries.
const FIX: &str = "read both then supersede or close a window or acknowledge V302";

/// What a row's fix says first when either line names a period.
const PERIOD: &str = "One of these lines names a period; if both held at different times, close the older engram's validity window.";

/// Why nothing is counted or scored while the model cannot be loaded, and
/// where to read more.
const UNAVAILABLE: &str = "the contradiction model could not be loaded (crystalline status and crystalline doctor say why; setting evolve.contradictions again retries)";

/// One stored row that still stands.
struct Live<'a> {
    row: &'a ContradictionRow,
    score: f32,
    a: &'a EngramFacts,
    b: &'a EngramFacts,
    line_a: usize,
    text_a: &'a str,
    line_b: usize,
    text_b: &'a str,
}

pub(super) fn detect_contradictions(input: &SweepInput, report: &mut SweepReport) {
    notes(input, report);
    if input.contradictions.is_empty() {
        return;
    }
    // Base rows only: a draft is a row of its own and is never scored.
    let by_id: HashMap<i64, &EngramFacts> = input
        .engrams
        .iter()
        .filter(|f| f.actor.is_empty())
        .map(|f| (f.id.0, f))
        .collect();
    let mut lines: HashMap<i64, HashMap<String, (usize, &str)>> = HashMap::new();
    let mut live: Vec<Live<'_>> = Vec::new();
    for row in &input.contradictions {
        let score = input
            .options
            .order_aggregation
            .combine(row.score_ab, row.score_ba);
        // A NaN never clears the line.
        if score.is_nan() || score < input.options.contradiction_threshold {
            continue;
        }
        let (Some(a), Some(b)) = (by_id.get(&row.a.0).copied(), by_id.get(&row.b.0).copied())
        else {
            continue;
        };
        if !eligible(&a.status)
            || !eligible(&b.status)
            || !windows_overlap(a.valid_from, a.valid_to, b.valid_from, b.valid_to)
        {
            continue;
        }
        // An edited line is a new question: its hash is gone from the engram.
        let Some((line_a, text_a)) = line_of(&mut lines, a, &row.hash_a) else {
            continue;
        };
        let Some((line_b, text_b)) = line_of(&mut lines, b, &row.hash_b) else {
            continue;
        };
        live.push(Live {
            row,
            score,
            a,
            b,
            line_a,
            text_a,
            line_b,
            text_b,
        });
    }
    live.sort_by(|x, y| {
        y.score.total_cmp(&x.score).then_with(|| {
            (x.row.a.0, x.row.b.0, x.line_a, x.line_b)
                .cmp(&(y.row.a.0, y.row.b.0, y.line_a, y.line_b))
        })
    });
    // Capped after the acknowledgments are known, never before: a pair the
    // anchor already acknowledged takes no slot, so ten acknowledged pairs
    // never hide the eleventh.
    let mut cap = FindingCap::new(input.options.max_contradiction_findings);
    for l in live {
        // The engram V301 would pick for the pair, so the two rules agree on
        // where a pair lives.
        let pair = [l.a, l.b];
        let lead = leader(&pair, &[0, 1]).unwrap_or(0);
        let (anchor, other, anchor_line, anchor_text, other_line, other_text) = if lead == 0 {
            (l.a, l.b, l.line_a, l.text_a, l.line_b, l.text_b)
        } else {
            (l.b, l.a, l.line_b, l.text_b, l.line_a, l.text_a)
        };
        // The scope is ordered by address, whichever engram anchors it, so
        // each hash stays beside its engram.
        let (lo, hi) = if l.a.address() <= l.b.address() {
            ((l.a, &l.row.hash_a), (l.b, &l.row.hash_b))
        } else {
            ((l.b, &l.row.hash_b), (l.a, &l.row.hash_a))
        };
        // Rounded once, so the text and the probability column say the
        // same number.
        let probability = (f64::from(l.score) * 100.0).round() / 100.0;
        let fix = if l.row.period {
            format!("{PERIOD} {FIX}")
        } else {
            FIX.to_string()
        };
        cap.push(
            report,
            anchor,
            Finding::about("V302", anchor)
                .with(
                    Class::Judgment,
                    format!(
                        "\"{anchor_text}\" ({}) against \"{other_text}\" ({}) read as a contradiction at probability {probability:.2}",
                        anchor.title, other.title
                    ),
                    format!(
                        "{} line {anchor_line}; {} line {other_line}; probability {probability:.2}; model {}",
                        anchor.address(),
                        other.address(),
                        input.contradiction_model
                    ),
                    fix,
                )
                .at_line(Some(anchor_line))
                .scoped(vec![
                    lo.0.address(),
                    hi.0.address(),
                    lo.1.clone(),
                    hi.1.clone(),
                ])
                .with_counterpart(Counterpart {
                    permalink: other.permalink.clone(),
                    title: other.title.clone(),
                    line: other_line,
                    probability,
                }),
        );
    }
    if cap.cut {
        report.truncations.push(format!(
            "V302 findings capped at {}",
            input.options.max_contradiction_findings
        ));
    }
}

/// The truncation lines that keep a quiet `V302` from reading as a clean
/// domain: what is not counted yet, what is not scored yet, what cannot be
/// counted until it is embedded and what is never scored. All zero and
/// `false` when the check is off, which says nothing.
fn notes(input: &SweepInput, report: &mut SweepReport) {
    if let Some(compared) = input.contradiction_vectors_capped {
        report.truncations.push(format!(
            "V302 skipped: {compared} lead vectors over the {} cap, no related pairs are scored",
            input.options.max_twin_vectors
        ));
    }
    // With the model unavailable the daemon runs no pass until the setting
    // is set again, so neither line may promise one.
    let unavailable = input.contradiction_model_unavailable;
    if input.contradictions_uncounted {
        report.truncations.push(if unavailable {
            format!("V302: related pairs not counted: {UNAVAILABLE}")
        } else {
            "V302: related pairs not counted yet (the daemon counts them after embedding)"
                .to_string()
        });
    }
    if input.contradictions_pending > 0 {
        report.truncations.push(if unavailable {
            format!(
                "V302: {} related pairs not scored: {UNAVAILABLE}",
                input.contradictions_pending
            )
        } else {
            format!(
                "V302: {} related pairs not scored yet (the daemon scores them after embedding)",
                input.contradictions_pending
            )
        });
    }
    if input.contradiction_unembedded > 0 {
        report.truncations.push(format!(
            "V302: {} engrams have no embedding yet, their related pairs are not counted",
            input.contradiction_unembedded
        ));
    }
    if input.contradiction_candidates_capped {
        report.truncations.push(format!(
            "V302: related pairs capped at {}, the least related are never scored",
            max_related_pairs()
        ));
    }
}

/// The current line and text of the observation in `f` whose hash is `hash`,
/// the first one in document order. Each engram's hashes are computed once.
fn line_of<'a>(
    cache: &mut HashMap<i64, HashMap<String, (usize, &'a str)>>,
    f: &'a EngramFacts,
    hash: &str,
) -> Option<(usize, &'a str)> {
    let map = cache.entry(f.id.0).or_insert_with(|| {
        let mut m = HashMap::new();
        for o in &f.observations {
            m.entry(observation_hash(&o.text))
                .or_insert((o.line, o.text.as_str()));
        }
        m
    });
    map.get(hash).copied()
}
