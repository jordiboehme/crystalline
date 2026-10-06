//! `V302`: two observation lines a local NLI model read as a possible
//! contradiction. The rows are the daemon's; this module reads them and
//! re-filters them against what the engrams say now. A sweep scores nothing.

use std::collections::{BTreeMap, HashMap};

use super::{
    Class, Counterpart, EngramFacts, Finding, FindingCap, SweepInput, SweepReport, leader,
};
use crate::nli::candidates::{eligible, max_related_pairs, observation_hash, windows_overlap};
use crate::store::ContradictionRow;

/// The fix every `V302` row carries.
pub(crate) const FIX: &str = "read both then supersede the older (valid_to set to the real date), correct the stale line or acknowledge V302";

/// What a row's fix says first when either line names a period.
pub(crate) const PERIOD: &str = "One of these lines names a period; if both held at different times, the newer engram supersedes the older one, whose valid_to is set to the real date.";

/// Why nothing is counted or scored while the model cannot be loaded, and
/// where to read more.
const UNAVAILABLE: &str = "the contradiction model could not be loaded (crystalline status and crystalline doctor say why; setting evolve.contradictions again or restarting the daemon retries)";

/// [`UNAVAILABLE`] on a read-only daemon, which refuses the setting: only a
/// restart asks for the model again there.
const UNAVAILABLE_READ_ONLY: &str = "the contradiction model could not be loaded (crystalline status and crystalline doctor say why; the daemon is read-only, so restarting it retries)";

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

/// About how much of a line a `V302` finding quotes.
const LINE_CHARS: usize = 200;

/// `text` cut to [`LINE_CHARS`] characters, the last three replaced by
/// `...` when it was longer. Counted in characters, never bytes, so a cut
/// never lands inside one.
fn cut(text: &str) -> String {
    if text.chars().count() <= LINE_CHARS {
        return text.to_string();
    }
    let mut out: String = text.chars().take(LINE_CHARS - 3).collect();
    out.push_str("...");
    out
}

/// The anchor of an engram pair (the engram `V301` would pick, so the two
/// rules agree on where a pair lives) and the other.
fn anchor_of<'a>(a: &'a EngramFacts, b: &'a EngramFacts) -> (&'a EngramFacts, &'a EngramFacts) {
    let pair = [a, b];
    if leader(&pair, &[0, 1]).unwrap_or(0) == 0 {
        (a, b)
    } else {
        (b, a)
    }
}

/// The scope of one line pair: both addresses lower first, each hash beside
/// its engram, whichever engram anchors it.
fn scope_of(l: &Live<'_>) -> String {
    let parts = if l.a.address() <= l.b.address() {
        vec![
            l.a.address(),
            l.b.address(),
            l.row.hash_a.clone(),
            l.row.hash_b.clone(),
        ]
    } else {
        vec![
            l.b.address(),
            l.a.address(),
            l.row.hash_b.clone(),
            l.row.hash_a.clone(),
        ]
    };
    super::scope_for("V302", parts)
}

/// One line pair the detector emits: the evidence of its engram pair, or an
/// acknowledged line pair of it that the acknowledgment pass suppresses and
/// counts.
struct Pick<'a> {
    live: Live<'a>,
    scope: String,
    /// The other open line pairs of the engram pair; zero on an
    /// acknowledged one.
    more: usize,
}

pub(super) fn detect_contradictions(input: &SweepInput, report: &mut SweepReport) {
    notes(input, report);
    if input.contradiction_no_line_floor.is_some() || input.contradictions.is_empty() {
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
    // Every standing line pair, by engram pair. The rows come in narrow
    // (ids, lines, hashes, scores); the texts are the parsed engrams' own.
    let mut groups: BTreeMap<(i64, i64), Vec<Live<'_>>> = BTreeMap::new();
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
        // The same rule the walk applies, as of the sweep's date: an engram
        // that expired since the last walk never surfaces.
        if !eligible(&a.status, a.valid_to, input.today)
            || !eligible(&b.status, b.valid_to, input.today)
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
        let key = (row.a.0.min(row.b.0), row.a.0.max(row.b.0));
        groups.entry(key).or_default().push(Live {
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
    // One open finding per engram pair: its strongest line pair the anchor
    // has not acknowledged, with how many more stand. Every acknowledged
    // line pair of the pair is emitted too, so the acknowledgment pass counts
    // and suppresses it and the audit view still lists it.
    let mut picks: Vec<Pick<'_>> = Vec::new();
    for mut group in groups.into_values() {
        group.sort_by(|x, y| {
            y.score
                .total_cmp(&x.score)
                .then_with(|| (x.line_a, x.line_b).cmp(&(y.line_a, y.line_b)))
        });
        let (anchor, _) = anchor_of(group[0].a, group[0].b);
        let scoped: Vec<(String, bool)> = group
            .iter()
            .map(|l| {
                let scope = scope_of(l);
                let acked = super::matching_ack_scope(&anchor.acks, "V302", &scope).is_some();
                (scope, acked)
            })
            .collect();
        let open = scoped.iter().filter(|(_, acked)| !acked).count();
        let evidence = scoped.iter().position(|(_, acked)| !acked);
        for (i, (live, (scope, acked))) in group.into_iter().zip(scoped).enumerate() {
            if acked {
                picks.push(Pick {
                    live,
                    scope,
                    more: 0,
                });
            } else if Some(i) == evidence {
                picks.push(Pick {
                    live,
                    scope,
                    more: open - 1,
                });
            }
        }
    }
    picks.sort_by(|x, y| {
        let (x, y) = (&x.live, &y.live);
        y.score.total_cmp(&x.score).then_with(|| {
            (x.row.a.0, x.row.b.0, x.line_a, x.line_b)
                .cmp(&(y.row.a.0, y.row.b.0, y.line_a, y.line_b))
        })
    });
    // Capped after the acknowledgments are known, never before: a line pair
    // the anchor already acknowledged takes no slot, so ten acknowledged
    // pairs never hide the eleventh.
    let mut cap = FindingCap::new(input.options.max_contradiction_findings);
    for pick in picks {
        let (anchor, finding) = finding_for(input, pick);
        cap.push(report, anchor, finding);
    }
    if cap.cut {
        report.truncations.push(format!(
            "V302 findings capped at {}",
            input.options.max_contradiction_findings
        ));
    }
}

/// The `V302` finding for one picked line pair, and the engram it is about.
fn finding_for<'a>(input: &SweepInput, pick: Pick<'a>) -> (&'a EngramFacts, Finding) {
    let Pick {
        live: l,
        scope,
        more,
    } = pick;
    let (anchor, other) = anchor_of(l.a, l.b);
    let (anchor_line, anchor_text, other_line, other_text) = if std::ptr::eq(anchor, l.a) {
        (l.line_a, l.text_a, l.line_b, l.text_b)
    } else {
        (l.line_b, l.text_b, l.line_a, l.text_a)
    };
    // Rounded once, so the text and the columns say the same numbers.
    let probability = (f64::from(l.score) * 100.0).round() / 100.0;
    let similarity = (f64::from(l.row.similarity) * 100.0).round() / 100.0;
    let rest = match more {
        0 => String::new(),
        1 => ", and 1 more line pair".to_string(),
        n => format!(", and {n} more line pairs"),
    };
    let fix = if l.row.period {
        format!("{PERIOD} {FIX}")
    } else {
        FIX.to_string()
    };
    let finding = Finding::about("V302", anchor)
        .with(
            Class::Judgment,
            format!(
                "Possible contradiction: line {anchor_line} of \"{}\" and line {other_line} of \"{}\" read as contradicting at probability {probability:.2}, line similarity {similarity:.2}{rest}",
                anchor.title, other.title
            ),
            format!(
                "{} line {anchor_line}; {} line {other_line}; probability {probability:.2}; similarity {similarity:.2}; model {}",
                anchor.address(),
                other.address(),
                input.contradiction_model
            ),
            fix,
        )
        .at_line(Some(anchor_line))
        .scoped_as(scope)
        .with_counterpart(Counterpart {
            permalink: other.permalink.clone(),
            title: other.title.clone(),
            line: other_line,
            probability,
            similarity,
            anchor_text: cut(anchor_text),
            text: cut(other_text),
            more_line_pairs: more,
        });
    (anchor, finding)
}

/// The truncation lines that keep a quiet `V302` from reading as a clean
/// domain: what is not counted yet, what is not scored yet, what cannot be
/// counted until it is embedded and what is never scored. All zero and
/// `false` when the check is off, which says nothing.
fn notes(input: &SweepInput, report: &mut SweepReport) {
    // Without a floor the stored rows are not read and nothing is counted,
    // so this one line replaces every other.
    if let Some(model) = &input.contradiction_no_line_floor {
        report.truncations.push(format!(
            "V302 does not run: the embedding model '{model}' has no measured line-similarity floor"
        ));
        return;
    }
    if let Some(compared) = input.contradiction_vectors_capped {
        report.truncations.push(format!(
            "V302 skipped: {compared} lead vectors over the {} cap, no related pairs are scored",
            input.options.max_twin_vectors
        ));
    }
    // With the model unavailable the daemon runs no pass until the setting
    // is set again, so neither line may promise one.
    let unavailable = input.contradiction_model_unavailable;
    let reason = if input.contradiction_read_only {
        UNAVAILABLE_READ_ONLY
    } else {
        UNAVAILABLE
    };
    if input.contradictions_uncounted {
        report.truncations.push(if unavailable {
            format!("V302: related pairs not counted: {reason}")
        } else {
            "V302: related pairs not counted yet (the daemon counts them after embedding)"
                .to_string()
        });
    }
    if input.contradictions_pending > 0 {
        report.truncations.push(if unavailable {
            format!(
                "V302: {} related pairs not scored: {reason}",
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
