//! The consolidation sweep: the `V` rule family behind `evolve`.
//!
//! `verify` answers "is this well formed?" and `doctor` answers "is the
//! machinery healthy?". Neither answers "is the knowledge still true, and is
//! it well organized?". This module is that third question, expressed as
//! detectors over prepared facts.
//!
//! Three families, one letter each way:
//!
//! - `V0xx` **temporal and lifecycle** - a validity window that closed, a
//!   staleness date that elapsed, a replacement that landed without the
//!   retirement being finished;
//! - `V1xx` **structural integrity** - unresolved references, one-sided
//!   reciprocal relations, orphans, stubs, oversized engrams;
//! - `V2xx` **redundancy and drift** - near-duplicate bodies, colliding titles,
//!   tag spellings that drifted apart.
//!
//! `V3xx` is reserved for semantic contradiction between engram pairs and is
//! deliberately not implemented here: this sweep detects by dates, links and
//! graph shape, never by meaning, so it can never confirm a contradiction.
//!
//! # Detect and guide, never auto-consolidate
//!
//! Nothing in this module mutates anything. It returns a ranked queue with the
//! evidence for each item and a prescribed next action, and the existing write
//! tools stay the only way to act on it. Automatic consolidation of a memory
//! store degrades it: each pass rewrites the products of earlier passes and
//! small abstraction errors compound until utility falls below the no-memory
//! baseline. Selecting a bounded working region and handing it back for review
//! is the shape that holds up.
//!
//! Every finding therefore carries a server-computed [`Class`]:
//! [`Class::Mechanical`] completes intent the archive already records, so an
//! agent may just do it and summarize once, while [`Class::Judgment`] changes
//! what the archive claims and has to be proposed and agreed one at a time.
//! Classifying here rather than in prose is what makes that rule assertable.
//!
//! # Purity
//!
//! The detectors are pure functions over [`SweepInput`]. No store call, no
//! clock read, no file access: the engine assembles the facts and passes
//! `today` in, so a run is reproducible and every rule is unit testable. This
//! follows the tag clusterer, which is likewise a store-free detector library
//! living in the index crate because its inputs speak the index's vocabulary.

pub mod dedupe;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::NaiveDate;
use crystalline_core::similarity::{dice_coefficient, normalize};
use serde::Serialize;

use crate::store::{
    AttachmentRow, EdgeKind, EngramId, GraphEdge, GraphNode, GraphSlice, TagAlias, TagCount,
    is_current_status, is_retired_status,
};
use crate::vocab::{tag_clusters, tag_clusters_with_aliases};

// ---------------------------------------------------------------------------
// Thresholds
// ---------------------------------------------------------------------------

/// How old an engram must be, in days since `recorded_at`, before `V003` will
/// call it unverified. A high floor on purpose: `V003` is the rule most likely
/// to flood an evergreen archive on day one, so it only speaks about knowledge
/// nobody has looked at in half a year.
pub const DEFAULT_MIN_AGE_DAYS: i64 = 180;

/// How old an engram must be, in days since `recorded_at`, before `V104` will
/// call it an orphan. Short, because a freshly captured engram is expected to
/// be unlinked for a while: two weeks is long enough that the capture session
/// is over and nobody came back to wire it in.
pub const ORPHAN_MIN_AGE_DAYS: i64 = 14;

/// The bigram Dice score at or above which two bodies count as near-duplicates
/// for `V201`. Higher than verify's `Q004` section threshold because a whole
/// engram has far more room to agree by accident than one section does.
pub const DUP_THRESHOLD: f64 = 0.80;

/// The shortest normalized body `V201` will score. Below this the Dice
/// coefficient is dominated by common English bigrams and two unrelated stubs
/// look like twins.
pub const MIN_DUP_BODY_CHARS: usize = 200;

/// The share of a domain's engrams that must carry at least one resolved edge
/// before `V104` runs at all. Under this the domain simply is not a linked
/// graph, and calling every engram in it an orphan says nothing: a flat domain
/// is a style, not a defect.
pub const ORPHAN_DENSITY_GATE: f64 = 0.25;

/// How many `V003` findings one run may emit, oldest first. The rest are
/// reported as a truncation rather than dropped silently, so the next run over
/// the same scope surfaces the following batch once these are handled.
pub const V003_CAP: usize = 10;

/// The largest MinHash bucket `V201` will expand into candidate pairs. A
/// bucket bigger than this is shared boilerplate, not a duplicate set, and
/// expanding it costs a quadratic blowup for no findings.
pub const MAX_BUCKET: usize = 64;

/// The global ceiling on `V201` candidate pairs per run. The hard bound that
/// keeps a large corpus from turning near-duplicate detection into an
/// unbounded similarity pass.
pub const MAX_CANDIDATE_PAIRS: usize = 50_000;

/// How many hash functions make up one MinHash signature.
pub const MINHASH_HASHES: usize = 32;

/// How many bands the signature is split into for blocking. More bands means
/// more candidate pairs and higher recall at a higher cost.
pub const MINHASH_BANDS: usize = 8;

/// How many signature values make up one band. `MINHASH_BANDS *
/// MINHASH_BAND_ROWS` equals [`MINHASH_HASHES`]; 8 bands of 4 puts the
/// blocking S-curve's steep region a little under the Dice threshold, so a
/// true duplicate almost always reaches verification.
pub const MINHASH_BAND_ROWS: usize = 4;

/// The default approximate token budget for `V105`, matching verify's `Q002`
/// default so the two rules agree on what oversized means. `0` disables the
/// rule for an engram.
pub const DEFAULT_TOKEN_BUDGET: usize = 2500;

/// The fewest non-blank body lines outside fenced code an engram needs before
/// `V106` stops calling it a stub. The same predicate as verify's `Q001`.
pub const MIN_CONTENT_LINES: usize = 3;

/// How similar an existing title must be to an unresolved link target before
/// `V102` calls the repair mechanical. Near-exact on purpose: at this score the
/// intended target is not in doubt, so completing the spelling changes nothing
/// the archive claims.
pub const TITLE_CANDIDATE_THRESHOLD: f64 = 0.90;

/// The resolved inbound degree at or above which an engram counts as a hub and
/// its findings gain [`HUB_BOOST`]. Wrong knowledge that many engrams cite
/// matters more than wrong knowledge nothing cites.
pub const HUB_INBOUND_DEGREE: usize = 3;

/// The priority a hub's finding gains.
pub const HUB_BOOST: i64 = 5;

/// The priority a finding gains when a person wrote the engram themselves.
/// Somebody bothering to sit down and capture it is a salience-like signal the
/// frontmatter rarely carries explicitly, so the queue treats human-authored
/// knowledge as worth more attention. Sized between the salience cap of
/// [`MAX_SALIENCE_BOOST`] and the [`HUB_BOOST`]: stronger than being widely
/// cited, weaker than a top salience score somebody set on purpose.
pub const HUMAN_AUTHORED_BOOST: i64 = 8;

/// The most salience alone can add to a finding's priority, reached at the top
/// of the 0 to 10 frontmatter salience scale.
pub const MAX_SALIENCE_BOOST: i64 = 10;

/// The priority ceiling. Base plus both boosts can exceed it, so the sum is
/// clamped.
pub const MAX_PRIORITY: u8 = 100;

/// Statuses that mark knowledge as not yet asserted. These are exempt from the
/// lifecycle rules: a draft is allowed to be stale, unverified and out of date,
/// because it never claimed to be true in the first place.
pub const SPECULATIVE_STATUSES: [&str; 4] = ["draft", "proposed", "idea", "poc"];

/// Engram types the orphan rule never speaks about. A manifest and a schema are
/// structural files; being unlinked is their normal shape.
pub const ORPHAN_EXEMPT_TYPES: [&str; 2] = ["manifest", "schema"];

/// The reciprocal relation pairs `V103` checks, forward first. A resolved
/// forward edge without its converse is a half-wired relation.
pub const RECIPROCAL_PAIRS: [(&str, &str); 2] = [
    ("supersedes", "superseded_by"),
    ("summarizes", "summarized_by"),
];

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// The three detector families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    /// `V0xx`: validity windows, staleness and the supersede lifecycle.
    Temporal,
    /// `V1xx`: references, reciprocity, orphans, stubs and size.
    Structure,
    /// `V2xx`: duplicate content, colliding titles and tag drift.
    Redundancy,
}

impl Family {
    /// Every family, in catalog order.
    pub const ALL: [Family; 3] = [Family::Temporal, Family::Structure, Family::Redundancy];

    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Temporal => "temporal",
            Family::Structure => "structure",
            Family::Redundancy => "redundancy",
        }
    }

    /// Parse a wire name, case-insensitively. `None` for an unknown value, so
    /// a caller can name the valid set in its error.
    pub fn parse(s: &str) -> Option<Family> {
        let s = s.trim().to_ascii_lowercase();
        Family::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

impl std::fmt::Display for Family {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much authority acting on a finding needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    /// The fix completes intent the archive already records. An agent may
    /// apply it directly and summarize once at the end.
    Mechanical,
    /// The fix changes what the archive claims. Read the engram, propose it and
    /// wait for a yes, one at a time.
    Judgment,
}

impl Class {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Mechanical => "mechanical",
            Class::Judgment => "judgment",
        }
    }
}

impl std::fmt::Display for Class {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One rule's fixed metadata: what it is called, which family it belongs to,
/// what it scores before ranking and the instruction the response repeats once
/// per rule in its legend rather than once per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RuleInfo {
    /// The rule id, for example `V001`.
    pub id: &'static str,
    /// The family the rule belongs to.
    pub family: Family,
    /// The priority before the salience and hub boosts.
    pub base: u8,
    /// A few words naming what the rule detects.
    pub summary: &'static str,
    /// The prescribed action, written for the agent working the queue.
    pub instruction: &'static str,
}

/// The full rule catalog, in id order. The single place a base priority or a
/// prescribed action is written down.
pub const RULES: [RuleInfo; 19] = [
    RuleInfo {
        id: "V001",
        family: Family::Temporal,
        base: 85,
        summary: "expired window still current",
        instruction: "The validity window closed but the engram still reads as current. Read it. If it still holds extend or remove valid_to. If it ended retire it and wire the supersede pair when something replaced it.",
    },
    RuleInfo {
        id: "V002",
        family: Family::Temporal,
        base: 70,
        summary: "staleness elapsed",
        instruction: "The review date passed with no verification since. Re-check the claim. Unchanged: record a verified entry and push stale_after forward. Changed: reconcile in place or supersede with a replacement.",
    },
    RuleInfo {
        id: "V003",
        family: Family::Temporal,
        base: 25,
        summary: "aging never verified",
        instruction: "Old knowledge nobody has ever confirmed and with no staleness bound to make a future sweep notice. Confirm it still holds and record a verified entry or set a stale_after so the next sweep has a real date to compare.",
    },
    RuleInfo {
        id: "V004",
        family: Family::Temporal,
        base: 65,
        summary: "superseded without successor",
        instruction: "The engram says it was superseded but names no successor that resolves. Name the successor with a superseded_by relation or switch to a retirement status that implies none. Verify's T005 sees only the missing field; this also sees a relation that fails to resolve.",
    },
    RuleInfo {
        id: "V005",
        family: Family::Temporal,
        base: 90,
        summary: "supersedes target still current",
        instruction: "The replacement landed but the retirement was never finished. Complete it: set the old engram's status, append a superseded_by relation pointing at the replacement and set valid_to when the end date is known.",
    },
    RuleInfo {
        id: "V006",
        family: Family::Temporal,
        base: 50,
        summary: "human capture never reviewed",
        instruction: "A person captured this directly and nobody has reviewed it since. Read it and verify the claim, align its tags with the vocabulary and its aliases, wire relations both ways into the neighbourhood it belongs to, then record a verified entry. Propose any wording change and wait for a yes - the words are the human's.",
    },
    RuleInfo {
        id: "V007",
        family: Family::Temporal,
        base: 50,
        summary: "attachment never analyzed",
        instruction: "Read the attachment through the crystalline attachment resource (resources/read on crystalline://<domain>/<path>). Capture what it teaches as an engram, or confirm an engram embedding it already covers it. On that engram set analyzes: <path> and analyzed_hash to the attachment's current sha256, and wire relations into its neighbourhood. Creating or changing knowledge: ask before acting.",
    },
    RuleInfo {
        id: "V008",
        family: Family::Temporal,
        base: 60,
        summary: "attachment changed since analysis",
        instruction: "The attachment changed after this engram captured it. Re-read it, update the engram to match what it now shows, set analyzed_hash to the current hash and record a verified entry. Propose wording changes and wait for a yes.",
    },
    RuleInfo {
        id: "V101",
        family: Family::Structure,
        base: 55,
        summary: "live reference to retired",
        instruction: "A current engram points at retired knowledge. Repoint it at the successor named in the evidence or keep the link deliberately as a historical citation and say so in the text.",
    },
    RuleInfo {
        id: "V102",
        family: Family::Structure,
        base: 50,
        summary: "unresolved reference",
        instruction: "A link points at nothing. Fix the spelling, add a domain prefix or capture the engram the link expects. Never auto-create the target.",
    },
    RuleInfo {
        id: "V103",
        family: Family::Structure,
        base: 35,
        summary: "one-sided reciprocal",
        instruction: "One half of a reciprocal relation pair is missing. Append the inverse relation on the counterpart so the graph reads the same from both ends.",
    },
    RuleInfo {
        id: "V104",
        family: Family::Structure,
        base: 30,
        summary: "orphan",
        instruction: "Nothing links to this engram and it links to nothing. Link it into the neighbourhood its tags suggest or confirm that standalone is intended.",
    },
    RuleInfo {
        id: "V105",
        family: Family::Structure,
        base: 60,
        summary: "oversized",
        instruction: "The engram is over its token budget. Split it by granularity: the distilled summary stays, the full text becomes a type source engram under sources/ and the two link both ways. Verify's Q002 flags the same size.",
    },
    RuleInfo {
        id: "V106",
        family: Family::Structure,
        base: 45,
        summary: "stub",
        instruction: "Almost no content beyond the frontmatter. Enrich it, fold it into the engram that owns the topic or retire it. Verify's Q001 flags the same shape.",
    },
    RuleInfo {
        id: "V107",
        family: Family::Structure,
        base: 45,
        summary: "dangling attachment reference",
        instruction: "The file is gone. Restore it (Fluid upload or archive import) or remove or correct the reference; either changes what the engram shows a reader, so ask first.",
    },
    RuleInfo {
        id: "V108",
        family: Family::Structure,
        base: 55,
        summary: "orphaned attachment",
        instruction: "An attachment without a reference from an engram should not exist. Delete it - or, if it should have been knowledge, analyze it into an engram that references and claims it first. Deleting is irreversible, so ask before acting. Delete with delete_engram using the assets/ path as the identifier, or in Fluid. One limit worth knowing: nothing records reference history, so a file nobody ever referenced and a file an edit stopped referencing look the same here, and the resolution is the same either way.",
    },
    RuleInfo {
        id: "V201",
        family: Family::Redundancy,
        base: 80,
        summary: "near-duplicate content",
        instruction: "Several engrams say close to the same thing. Read them all, merge into the richest one, then supersede or delete the others after repointing every inbound link. Detection is lexical, so a pure paraphrase will not appear here.",
    },
    RuleInfo {
        id: "V202",
        family: Family::Redundancy,
        base: 55,
        summary: "near-duplicate title",
        instruction: "Two engrams in one domain carry near-identical titles, which makes both link resolution and recall ambiguous. Merge them or retitle for genuine disambiguation.",
    },
    RuleInfo {
        id: "V203",
        family: Family::Redundancy,
        base: 30,
        summary: "tag drift",
        instruction: "One concept is spelled several ways in the tag vocabulary. Hand the user the exact merge command and let them run it. Never bulk-rewrite tags across engrams.",
    },
];

/// The catalog entry for a rule id, or `None` when the id is unknown.
pub fn rule_info(id: &str) -> Option<&'static RuleInfo> {
    RULES.iter().find(|r| r.id == id)
}

/// The separator a scope joins its parts with. A scope is stored in an engram's
/// frontmatter and compared as one string, never rendered as a table cell, so
/// it uses the ordinary comma rather than the semicolon the evidence columns
/// avoid commas for.
const SCOPE_SEPARATOR: &str = ", ";

/// The stable discriminator for a finding: the evidence an acknowledgment was
/// given for, in a form that survives an unrelated edit and changes the moment
/// the evidence does.
///
/// One match for the whole catalog, deliberately. The rules that carry a scope
/// pass their material in and this decides the shape:
///
/// - `V101` (the retired targets), `V102` (the unresolved targets), `V103` (the
///   counterparts), `V107` (the missing attachment paths), `V201` (the cluster
///   members) and `V202` (the colliding titles) are **sets**, so the parts are
///   sorted and deduplicated before joining: reordering the links in a body must
///   not re-raise an acknowledged finding, while a new member must;
/// - `V007` and `V008` name **one attachment path**, so the first part is the
///   whole scope;
/// - every other rule's identity is just (engram, rule) - the plain temporal
///   rules, orphans, stubs, size, tag drift and the anchorless orphaned
///   attachment - and carries an empty scope, which matches whatever the engram
///   looks like next time.
fn scope_for(rule: &str, mut parts: Vec<String>) -> String {
    match rule {
        "V101" | "V102" | "V103" | "V107" | "V201" | "V202" => {
            parts.sort();
            parts.dedup();
            parts.join(SCOPE_SEPARATOR)
        }
        "V007" | "V008" => parts.into_iter().next().unwrap_or_default(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// One acknowledgment read off an engram's `evolve_ack` frontmatter: somebody
/// looked at a finding, ruled it intentional and said so in the engram itself.
///
/// The stamp the file also carries (`by` and `at`) is deliberately absent: the
/// detectors decide whether an acknowledgment still matches, never who wrote it
/// or when, and leaving provenance out of the fact keeps that honest.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AckEntry {
    /// The rule id the acknowledgment silences, for example `V101`.
    pub rule: String,
    /// The evidence it was given for, as [`scope_for`] renders it. `None` is
    /// the generous fallback a hand-written entry gets: it matches whatever the
    /// finding's scope turns out to be.
    pub scope: Option<String>,
    /// Why it is intentional, in the acknowledger's own words.
    pub note: Option<String>,
}

/// How many findings acknowledgments suppressed on one run, whole and per
/// family, so a sweep never silently shrinks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AckCounts {
    /// Every suppressed finding.
    pub total: usize,
    /// Suppressed `V0xx` findings.
    pub temporal: usize,
    /// Suppressed `V1xx` findings.
    pub structure: usize,
    /// Suppressed `V2xx` findings.
    pub redundancy: usize,
}

impl AckCounts {
    /// Count one suppressed finding of `family`.
    fn add(&mut self, family: Family) {
        self.total += 1;
        match family {
            Family::Temporal => self.temporal += 1,
            Family::Structure => self.structure += 1,
            Family::Redundancy => self.redundancy += 1,
        }
    }

    /// The per-family counts in catalog order, for a caller shaping them into a
    /// map.
    pub fn by_family(&self) -> [(Family, usize); 3] {
        [
            (Family::Temporal, self.temporal),
            (Family::Structure, self.structure),
            (Family::Redundancy, self.redundancy),
        ]
    }
}

/// Everything the rules read about one engram, resolved once by the engine so
/// no detector ever touches a store or a file.
///
/// The engine builds this from the parsed engram plus graph data. Two notes
/// bind that assembly:
///
/// - the temporal fields are the **effective** values, read through the
///   frontmatter accessors that fold the legacy spellings (`written_at`,
///   `stale_on`, `latest_verified`), never the raw keys;
/// - [`EngramFacts::inbound`] and [`EngramFacts::outbound`] are whole-index
///   resolved degrees, not degrees within whatever graph slice accompanies
///   them, so chunking the slice never turns a linked engram into an orphan.
#[derive(Debug, Clone, PartialEq)]
pub struct EngramFacts {
    /// The engram's index id, the key every graph edge speaks in.
    pub id: EngramId,
    /// The domain the engram lives in.
    pub domain: String,
    /// The domain-relative permalink.
    pub permalink: String,
    /// The title.
    pub title: String,
    /// The domain-relative file path, for a human pointing an editor at it.
    pub path: String,
    /// The exact frontmatter `status`, lowercased.
    pub status: String,
    /// The frontmatter `type`, lowercased.
    pub engram_type: String,
    /// The tags in use on this engram.
    pub tags: Vec<String>,
    /// The raw `salience` frontmatter value, `None` when absent. Feeds the
    /// ranking boost only.
    pub salience: Option<f64>,
    /// `recorded_at`, the date every age comparison uses. An engram without one
    /// has no knowable age and is never flagged for being old.
    pub recorded_at: Option<NaiveDate>,
    /// Start of the validity window. Absent means always valid, and absence is
    /// never itself a finding.
    pub valid_from: Option<NaiveDate>,
    /// End of the validity window. Absent means valid forever, and absence is
    /// never itself a finding.
    pub valid_to: Option<NaiveDate>,
    /// The effective staleness date: `stale_after`, falling back to the legacy
    /// `review_after`.
    pub stale_on: Option<NaiveDate>,
    /// The date of the newest verification, from the `verified` trail or the
    /// legacy `last_verified` date.
    pub verified_on: Option<NaiveDate>,
    /// The body text, frontmatter excluded.
    pub body: String,
    /// The approximate token count, `body.chars() / 4`, the same estimate
    /// verify's `Q002` uses.
    pub tokens: usize,
    /// The resolved token budget for this engram: a per-file override, then the
    /// domain default, then [`DEFAULT_TOKEN_BUDGET`]. `0` disables `V105`.
    pub token_budget: usize,
    /// Resolved inbound edge count across the whole index.
    pub inbound: usize,
    /// Resolved outbound edge count across the whole index.
    pub outbound: usize,
    /// The verbatim `generated.by` frontmatter actor, `None` for an engram
    /// captured before write provenance was recorded or imported from
    /// somewhere that never wrote it. Absence fails quiet everywhere: no rule
    /// treats an unknown author as a finding.
    pub generated_by: Option<String>,
    /// The `assets/` path the engram's `analyzes` frontmatter key claims, with
    /// the reserved folder segment already folded to its canonical spelling.
    /// Setting it is the agent's act of saying "I read this file and this
    /// engram is what it teaches".
    pub analyzes: Option<String>,
    /// The `analyzed_hash` frontmatter key: the attachment's sha256 as it was
    /// when the engram captured it. Absent means the claim never recorded a
    /// hash, which `V008` reads as nothing to compare rather than as a change.
    pub analyzed_hash: Option<String>,
    /// The distinct `assets/` paths the body links to, from
    /// `crystalline_core::find_asset_refs`: fenced code skipped, fragments
    /// stripped, in order of first appearance.
    pub asset_refs: Vec<String>,
    /// The `evolve_ack` entries the engram carries: the findings somebody ruled
    /// intentional. Malformed entries never reach here - the engine skips them
    /// rather than failing a sweep over a hand-edited line.
    pub acks: Vec<AckEntry>,
}

impl EngramFacts {
    /// A minimal fact set: a stable engram with no dates, no tags, no body and
    /// the default token budget. The engine overwrites the fields it knows and
    /// the tests build fixtures from it.
    pub fn new(
        id: EngramId,
        domain: impl Into<String>,
        permalink: impl Into<String>,
    ) -> EngramFacts {
        let permalink = permalink.into();
        EngramFacts {
            id,
            domain: domain.into(),
            title: permalink.clone(),
            path: format!("{permalink}.md"),
            permalink,
            status: "stable".to_string(),
            engram_type: "engram".to_string(),
            tags: Vec::new(),
            salience: None,
            recorded_at: None,
            valid_from: None,
            valid_to: None,
            stale_on: None,
            verified_on: None,
            body: String::new(),
            tokens: 0,
            token_budget: DEFAULT_TOKEN_BUDGET,
            inbound: 0,
            outbound: 0,
            generated_by: None,
            analyzes: None,
            analyzed_hash: None,
            asset_refs: Vec::new(),
            acks: Vec::new(),
        }
    }

    /// Whether `generated.by` names a person (`human:<name>`, the OKF actor
    /// convention). Absence is never itself a finding. The prefix is taken with
    /// `get`, so an actor whose sixth byte lands inside a multi-byte character
    /// simply does not match instead of panicking.
    pub fn is_human_authored(&self) -> bool {
        self.generated_by
            .as_deref()
            .and_then(|by| by.get(..6))
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("human:"))
    }

    /// Whether the status says this is what holds now.
    pub fn is_current(&self) -> bool {
        is_current_status(&self.status)
    }

    /// Whether the status says the knowledge has been retired. Retirement is
    /// terminal: only `V004` ever speaks about a retired engram.
    pub fn is_retired(&self) -> bool {
        is_retired_status(&self.status)
    }

    /// Whether the status says the knowledge was never asserted in the first
    /// place, which exempts it from the lifecycle rules.
    pub fn is_speculative(&self) -> bool {
        SPECULATIVE_STATUSES.contains(&self.status.as_str())
    }

    /// Days between `recorded_at` and `today`, or `None` when the engram
    /// records no date to measure from.
    pub fn age_days(&self, today: NaiveDate) -> Option<i64> {
        self.recorded_at
            .map(|d| today.signed_duration_since(d).num_days())
    }

    /// The `domain/permalink` address, the form findings cite.
    pub fn address(&self) -> String {
        format!("{}/{}", self.domain, self.permalink)
    }
}

/// A reference that names a target the index could not resolve: the `V102`
/// input. The engine fills this from the store's unresolved-reference query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedRef {
    /// The engram the reference was written in.
    pub from: EngramId,
    /// The relation type, or `links_to` for a prose wikilink.
    pub rel_type: String,
    /// Whether the reference is a relation bullet or a prose wikilink.
    pub kind: EdgeKind,
    /// The `[[domain:Target]]` prefix as written, when the reference carried
    /// one.
    pub target_domain: Option<String>,
    /// The target text exactly as written inside the brackets. Findings quote
    /// it verbatim so a repair never has to guess the string to replace.
    pub target: String,
    /// The one-based line the reference sits on, when known.
    pub line: Option<usize>,
}

/// Every threshold the sweep uses, so a caller can tune a run without editing
/// the catalog. [`SweepOptions::default`] carries the documented constants.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepOptions {
    /// See [`DEFAULT_MIN_AGE_DAYS`].
    pub min_age_days: i64,
    /// See [`ORPHAN_MIN_AGE_DAYS`].
    pub orphan_min_age_days: i64,
    /// See [`DUP_THRESHOLD`].
    pub dup_threshold: f64,
    /// See [`MIN_DUP_BODY_CHARS`].
    pub min_dup_body_chars: usize,
    /// See [`ORPHAN_DENSITY_GATE`].
    pub orphan_density_gate: f64,
    /// See [`V003_CAP`].
    pub v003_cap: usize,
    /// See [`MAX_BUCKET`].
    pub max_bucket: usize,
    /// See [`MAX_CANDIDATE_PAIRS`].
    pub max_candidate_pairs: usize,
    /// See [`MINHASH_HASHES`].
    pub minhash_hashes: usize,
    /// See [`MINHASH_BANDS`]. `minhash_bands * minhash_rows` should equal
    /// `minhash_hashes`; a shorter product simply leaves the tail of the
    /// signature out of blocking.
    pub minhash_bands: usize,
    /// See [`MINHASH_BAND_ROWS`].
    pub minhash_rows: usize,
}

impl Default for SweepOptions {
    fn default() -> SweepOptions {
        SweepOptions {
            min_age_days: DEFAULT_MIN_AGE_DAYS,
            orphan_min_age_days: ORPHAN_MIN_AGE_DAYS,
            dup_threshold: DUP_THRESHOLD,
            min_dup_body_chars: MIN_DUP_BODY_CHARS,
            orphan_density_gate: ORPHAN_DENSITY_GATE,
            v003_cap: V003_CAP,
            max_bucket: MAX_BUCKET,
            max_candidate_pairs: MAX_CANDIDATE_PAIRS,
            minhash_hashes: MINHASH_HASHES,
            minhash_bands: MINHASH_BANDS,
            minhash_rows: MINHASH_BAND_ROWS,
        }
    }
}

/// One domain's assembled sweep input.
///
/// Scoped to a single domain because two rules are domain-relative: `V104`'s
/// density gate measures one domain's link density and `V202` only calls a
/// title collision within one domain. An unscoped sweep runs this once per
/// domain, which also bounds memory.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepInput {
    /// The domain being swept.
    pub domain: String,
    /// The date every temporal rule compares against. Passed in rather than
    /// read from the clock so a run is reproducible and testable.
    pub today: NaiveDate,
    /// The engrams in scope. Findings only ever attach to one of these.
    pub engrams: Vec<EngramFacts>,
    /// The resolved graph around them. Nodes may include targets in other
    /// domains, which is how a cross-domain reference gets a status.
    pub graph: GraphSlice,
    /// References whose target did not resolve.
    pub unresolved: Vec<UnresolvedRef>,
    /// The domain's tag vocabulary, for `V203`.
    pub tags: Vec<TagCount>,
    /// The declared tag aliases, folded out before clustering so a pair an
    /// alias already explains is not reported again.
    pub tag_aliases: Vec<TagAlias>,
    /// Every registered domain name. `V102` uses it to tell an unregistered
    /// target domain apart from a target that simply does not exist.
    pub known_domains: Vec<String>,
    /// Every attachment the domain holds, metadata only. The attachment rules
    /// compare this list against what the engrams reference and claim, in both
    /// directions.
    pub attachments: Vec<AttachmentRow>,
    /// Return the findings acknowledgments suppressed anyway, each marked
    /// [`Finding::acknowledged`] with the scope and note that silenced it. An
    /// audit view: the queue a run hands out drops them.
    pub include_acknowledged: bool,
    /// The thresholds for this run.
    pub options: SweepOptions,
}

impl SweepInput {
    /// An empty input for `domain` evaluated as of `today`, with default
    /// options.
    pub fn new(domain: impl Into<String>, today: NaiveDate) -> SweepInput {
        SweepInput {
            domain: domain.into(),
            today,
            engrams: Vec::new(),
            graph: GraphSlice::default(),
            unresolved: Vec::new(),
            tags: Vec::new(),
            tag_aliases: Vec::new(),
            known_domains: Vec::new(),
            attachments: Vec::new(),
            include_acknowledged: false,
            options: SweepOptions::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// One item in the queue: what is wrong, on what evidence and what to do.
///
/// Every text field is a flat scalar so the whole queue renders as one tabular
/// block. The fields avoid commas by convention and separate list items with
/// semicolons, which keeps the rendered cells unquoted and cheap.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// The rule id, for example `V005`.
    pub rule: &'static str,
    /// The family the rule belongs to.
    pub family: Family,
    /// The ranked priority, 0 to 100. See [`priority`].
    pub priority: u8,
    /// How much authority acting on this needs.
    pub class: Class,
    /// The domain the finding is about.
    pub domain: String,
    /// The engram the finding attaches to. Empty for a domain-level finding
    /// such as `V203`, which is about the vocabulary rather than one engram.
    pub permalink: String,
    /// The engram title. Empty alongside an empty permalink.
    pub title: String,
    /// The one-based line the finding points at, when it has one.
    pub line: Option<usize>,
    /// What is wrong, in one clause.
    pub finding: String,
    /// The facts the rule fired on, so the reader can check the call without
    /// opening the file.
    pub evidence: String,
    /// The one variable piece of the fix: the field assignment, the verbatim
    /// link text or the exact command. The prose instruction lives once per
    /// rule in [`RuleInfo::instruction`] rather than being repeated here.
    pub fix: String,
    /// The evidence discriminator an acknowledgment is matched against, per
    /// [`scope_for`]. Internal: it exists so an acknowledgment can hold while
    /// the evidence holds, and no surface renders it as a column.
    #[serde(skip)]
    pub scope: String,
    /// An acknowledgment matched and this finding is only here because the
    /// caller asked for the suppressed ones.
    pub acknowledged: bool,
    /// An acknowledgment for this rule exists on the engram but was given for
    /// different evidence, so it no longer matches. The finding is returned as
    /// usual: an acknowledgment that stopped applying is worth saying out loud
    /// rather than pretending it never happened.
    pub ack_stale: bool,
    /// The note the matching or stale acknowledgment carries, when it has one.
    pub ack_note: Option<String>,
}

impl Finding {
    /// A finding about `fact`, with family and base priority taken from the
    /// catalog and the ranking boosts already applied.
    fn about(rule: &'static str, fact: &EngramFacts) -> Finding {
        let info = rule_info(rule).expect("every emitted rule is in the catalog");
        Finding {
            rule: info.id,
            family: info.family,
            priority: priority(
                info.base,
                fact.salience,
                fact.inbound,
                fact.is_human_authored(),
            ),
            class: Class::Judgment,
            domain: fact.domain.clone(),
            permalink: fact.permalink.clone(),
            title: fact.title.clone(),
            line: None,
            finding: String::new(),
            evidence: String::new(),
            fix: String::new(),
            scope: String::new(),
            acknowledged: false,
            ack_stale: false,
            ack_note: None,
        }
    }

    /// A finding about a domain rather than an engram: no permalink, no title
    /// and no ranking boost to draw on.
    fn about_domain(rule: &'static str, domain: &str) -> Finding {
        let info = rule_info(rule).expect("every emitted rule is in the catalog");
        Finding {
            rule: info.id,
            family: info.family,
            priority: priority(info.base, None, 0, false),
            class: Class::Judgment,
            domain: domain.to_string(),
            permalink: String::new(),
            title: String::new(),
            line: None,
            finding: String::new(),
            evidence: String::new(),
            fix: String::new(),
            scope: String::new(),
            acknowledged: false,
            ack_stale: false,
            ack_note: None,
        }
    }

    /// A finding about an attachment rather than an engram: the path is the
    /// subject and the permalink stays empty, because no engram is the thing
    /// the reader has to open. Fabricating one would be a link to knowledge
    /// that does not exist, which is exactly what the finding is about.
    fn about_attachment(rule: &'static str, domain: &str, path: &str) -> Finding {
        Finding {
            title: path.to_string(),
            ..Finding::about_domain(rule, domain)
        }
    }

    /// Fill in the class and the three text columns.
    fn with(mut self, class: Class, finding: String, evidence: String, fix: String) -> Finding {
        self.class = class;
        self.finding = finding;
        self.evidence = evidence;
        self.fix = fix;
        self
    }

    /// Point the finding at a source line.
    fn at_line(mut self, line: Option<usize>) -> Finding {
        self.line = line;
        self
    }

    /// Record the evidence this finding fired on, which [`scope_for`] shapes
    /// into the scope an acknowledgment is matched against. A rule that says
    /// nothing here keeps the empty scope, which any acknowledgment matches.
    fn scoped(mut self, parts: impl IntoIterator<Item = String>) -> Finding {
        self.scope = scope_for(self.rule, parts.into_iter().collect());
        self
    }
}

/// The outcome of one domain's sweep: the ranked queue plus whatever a cap cut
/// out of it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SweepReport {
    /// How many engrams were examined.
    pub engrams_scanned: usize,
    /// The findings, already ranked by [`rank`].
    pub findings: Vec<Finding>,
    /// One line per cap that fired, phrased for a reader: what was cut and how
    /// much of it there was. Empty on a complete run.
    pub truncations: Vec<String>,
    /// How many findings acknowledgments suppressed, whole and per family. A
    /// suppressed finding leaves this trace whether or not
    /// [`SweepInput::include_acknowledged`] put it back in the queue.
    pub acknowledged: AckCounts,
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// A finding's priority:
/// `clamp(base + salience_boost + hub_boost + human_boost, 0, 100)`.
///
/// `salience_boost` is `clamp(round(salience), 0, 10)`, so a highly salient
/// engram's problems rise without a low-base rule ever outranking a high-base
/// one by more than the boost. `hub_boost` adds [`HUB_BOOST`] once the resolved
/// inbound degree reaches [`HUB_INBOUND_DEGREE`]. `human_boost` adds
/// [`HUMAN_AUTHORED_BOOST`] when a person wrote the engram, whatever the rule
/// that fired. A non-finite or negative salience contributes nothing.
pub fn priority(base: u8, salience: Option<f64>, inbound: usize, human_authored: bool) -> u8 {
    let raw = salience.unwrap_or(0.0);
    let boost = if raw.is_finite() {
        raw.round().clamp(0.0, MAX_SALIENCE_BOOST as f64) as i64
    } else {
        0
    };
    let hub = if inbound >= HUB_INBOUND_DEGREE {
        HUB_BOOST
    } else {
        0
    };
    let human = if human_authored {
        HUMAN_AUTHORED_BOOST
    } else {
        0
    };
    (i64::from(base) + boost + hub + human).clamp(0, i64::from(MAX_PRIORITY)) as u8
}

/// Sort findings into queue order: priority descending, then rule, domain and
/// permalink ascending. The sort is stable, so findings a rule emitted in a
/// deterministic order keep that order when every key ties.
pub fn rank(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.rule.cmp(b.rule))
            .then_with(|| a.domain.cmp(&b.domain))
            .then_with(|| a.permalink.cmp(&b.permalink))
    });
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Run every rule over one domain's facts and return the ranked queue.
///
/// Pure: no store, no clock, no files. Given the same input the output is
/// byte-identical every time.
pub fn detect(input: &SweepInput) -> SweepReport {
    let mut report = SweepReport {
        engrams_scanned: input.engrams.len(),
        ..SweepReport::default()
    };
    let graph = Graph::build(input);

    detect_lifecycle(input, &graph, &mut report);
    detect_structure(input, &graph, &mut report);
    detect_redundancy(input, &mut report);
    detect_attachments(input, &mut report);

    apply_acknowledgments(input, &mut report);
    rank(&mut report.findings);
    report
}

/// Drop the findings an acknowledgment silences, count them, and mark the ones
/// whose acknowledgment no longer matches.
///
/// The three outcomes for a finding whose anchor engram carries an `evolve_ack`
/// for its rule:
///
/// - the entry's scope is absent, or equals the finding's scope: **suppressed**,
///   counted, and returned only when the caller asked for the suppressed ones;
/// - the entry's scope differs: **returned**, flagged [`Finding::ack_stale`]
///   and carrying the old note, because "somebody ruled this intentional and
///   the evidence has since changed" is a different thing to read than a fresh
///   finding;
/// - no entry for the rule: untouched.
///
/// A finding with no anchor engram - `V203`'s vocabulary, `V108`'s attachment -
/// has nowhere to hang an acknowledgment and passes through.
fn apply_acknowledgments(input: &SweepInput, report: &mut SweepReport) {
    let acks: HashMap<&str, &[AckEntry]> = input
        .engrams
        .iter()
        .filter(|f| !f.acks.is_empty())
        .map(|f| (f.permalink.as_str(), f.acks.as_slice()))
        .collect();
    if acks.is_empty() {
        return;
    }

    let mut counts = AckCounts::default();
    let include = input.include_acknowledged;
    report.findings.retain_mut(|finding| {
        let Some(entries) = acks.get(finding.permalink.as_str()) else {
            return true;
        };
        let mut for_rule = entries
            .iter()
            .filter(|a| a.rule.eq_ignore_ascii_case(finding.rule));
        // The generous match wins over a stale one: an engram carrying both a
        // scope-less entry and an outdated scoped one is acknowledged.
        let matching = for_rule
            .clone()
            .find(|a| a.scope.as_deref().is_none_or(|s| s == finding.scope));
        match matching.or_else(|| for_rule.next()) {
            Some(ack) if matching.is_some() => {
                counts.add(finding.family);
                finding.acknowledged = true;
                finding.ack_note = ack.note.clone();
                include
            }
            Some(stale) => {
                finding.ack_stale = true;
                finding.ack_note = stale.note.clone();
                true
            }
            None => true,
        }
    });
    report.acknowledged = counts;
}

/// The edge and node lookups every rule shares, built once per run.
struct Graph<'a> {
    facts: HashMap<i64, &'a EngramFacts>,
    nodes: HashMap<i64, &'a GraphNode>,
    outbound: HashMap<i64, Vec<&'a GraphEdge>>,
    inbound: HashMap<i64, Vec<&'a GraphEdge>>,
}

impl<'a> Graph<'a> {
    fn build(input: &'a SweepInput) -> Graph<'a> {
        let mut facts = HashMap::new();
        for f in &input.engrams {
            facts.insert(f.id.0, f);
        }
        let mut nodes = HashMap::new();
        for n in &input.graph.nodes {
            nodes.insert(n.id.0, n);
        }
        let mut outbound: HashMap<i64, Vec<&GraphEdge>> = HashMap::new();
        let mut inbound: HashMap<i64, Vec<&GraphEdge>> = HashMap::new();
        for e in &input.graph.edges {
            outbound.entry(e.from.0).or_default().push(e);
            inbound.entry(e.to.0).or_default().push(e);
        }
        Graph {
            facts,
            nodes,
            outbound,
            inbound,
        }
    }

    /// The resolved edges leaving an engram, in input order.
    fn outbound_of(&self, id: EngramId) -> &[&'a GraphEdge] {
        self.outbound.get(&id.0).map_or(&[], |v| v.as_slice())
    }

    /// The resolved edges arriving at an engram, in input order.
    fn inbound_of(&self, id: EngramId) -> &[&'a GraphEdge] {
        self.inbound.get(&id.0).map_or(&[], |v| v.as_slice())
    }

    /// The status of an engram, from the in-scope facts when it is one and from
    /// the graph nodes otherwise. `None` when the id is outside both.
    fn status(&self, id: EngramId) -> Option<&str> {
        self.facts
            .get(&id.0)
            .map(|f| f.status.as_str())
            .or_else(|| self.nodes.get(&id.0).map(|n| n.status.as_str()))
    }

    /// The `domain/permalink` address of an engram, from either table.
    fn address(&self, id: EngramId) -> String {
        if let Some(f) = self.facts.get(&id.0) {
            return f.address();
        }
        match self.nodes.get(&id.0) {
            Some(n) => format!("{}/{}", n.domain, n.permalink),
            None => format!("id {}", id.0),
        }
    }

    /// The `[[Target]]` text that addresses `id` from inside `domain`: bare
    /// when both sit in the same domain, prefixed when they do not.
    fn link_text(&self, id: EngramId, domain: &str) -> String {
        let (d, title) = if let Some(f) = self.facts.get(&id.0) {
            (f.domain.as_str(), f.title.as_str())
        } else if let Some(n) = self.nodes.get(&id.0) {
            (n.domain.as_str(), n.title.as_str())
        } else {
            return format!("id {}", id.0);
        };
        if d == domain {
            format!("[[{title}]]")
        } else {
            format!("[[{d}:{title}]]")
        }
    }

    /// Whether a resolved relation edge of `rel_type` leaves `id`.
    fn has_outbound_rel(&self, id: EngramId, rel_type: &str) -> bool {
        self.outbound_of(id)
            .iter()
            .any(|e| e.kind == EdgeKind::Relation && e.rel_type == rel_type)
    }

    /// The engrams that declare `rel_type` pointing at `id`.
    fn inbound_rel_sources(&self, id: EngramId, rel_type: &str) -> Vec<EngramId> {
        self.inbound_of(id)
            .iter()
            .filter(|e| e.kind == EdgeKind::Relation && e.rel_type == rel_type)
            .map(|e| e.from)
            .collect()
    }

    /// The successor of a retired engram, from either direction of the
    /// supersede pair.
    fn successor(&self, id: EngramId) -> Option<EngramId> {
        if let Some(src) = self.inbound_rel_sources(id, "supersedes").first() {
            return Some(*src);
        }
        self.outbound_of(id)
            .iter()
            .find(|e| e.kind == EdgeKind::Relation && e.rel_type == "superseded_by")
            .map(|e| e.to)
    }
}

/// The `V0xx` family.
fn detect_lifecycle(input: &SweepInput, graph: &Graph<'_>, report: &mut SweepReport) {
    let today = input.today;

    for fact in &input.engrams {
        // V004 is the one rule that speaks about retired knowledge, so it runs
        // before the retirement guard.
        if fact.status == "superseded" && !graph.has_outbound_rel(fact.id, "superseded_by") {
            let dangling = input.unresolved.iter().find(|u| {
                u.from == fact.id && u.rel_type == "superseded_by" && u.kind == EdgeKind::Relation
            });
            let (evidence, line) = match dangling {
                Some(u) => (
                    format!(
                        "status=superseded; superseded_by [[{}]] does not resolve",
                        u.target
                    ),
                    u.line,
                ),
                None => (
                    "status=superseded; no superseded_by relation on the engram".to_string(),
                    None,
                ),
            };
            report.findings.push(
                Finding::about("V004", fact)
                    .with(
                        Class::Judgment,
                        "retired as superseded but names no successor that resolves".to_string(),
                        evidence,
                        "add `- superseded_by [[Successor]]` or set_frontmatter status=deprecated"
                            .to_string(),
                    )
                    .at_line(line),
            );
        }

        if fact.is_retired() {
            continue;
        }

        // V005: the replacement landed, the retirement did not.
        if fact.is_current() {
            let sources = graph.inbound_rel_sources(fact.id, "supersedes");
            if !sources.is_empty() {
                let addrs = join_semis(sources.iter().map(|s| graph.address(*s)));
                report.findings.push(Finding::about("V005", fact).with(
                    Class::Mechanical,
                    format!("still {} but already superseded by {addrs}", fact.status),
                    format!(
                        "status={}; superseded by {addrs}; inbound refs {}",
                        fact.status, fact.inbound
                    ),
                    "set_frontmatter status=superseded".to_string(),
                ));
            }
        }

        // V001: the window closed but the status did not. Suppressed when an
        // inbound supersedes exists, because V005 already owns that engram and
        // one engram must never draw two findings for one underlying fact.
        if let Some(valid_to) = fact.valid_to
            && valid_to < today
            && fact.is_current()
            && graph.inbound_rel_sources(fact.id, "supersedes").is_empty()
        {
            report.findings.push(Finding::about("V001", fact).with(
                Class::Judgment,
                format!(
                    "validity window ended {valid_to} but the status is still {}",
                    fact.status
                ),
                format!(
                    "valid_to={valid_to}; today={today}; status={}; inbound refs {}",
                    fact.status, fact.inbound
                ),
                "set_frontmatter valid_to=<later date> or status=superseded".to_string(),
            ));
        }

        if fact.is_speculative() {
            continue;
        }

        // V002: the review date elapsed with no verification since.
        if let Some(stale_on) = fact.stale_on
            && stale_on <= today
            && fact.verified_on.is_none_or(|v| v < stale_on)
        {
            let seen = match fact.verified_on {
                Some(v) => format!("last verified {v}"),
                None => "never verified".to_string(),
            };
            report.findings.push(Finding::about("V002", fact).with(
                Class::Judgment,
                format!("stale since {stale_on} with no verification recorded since"),
                format!("stale_after={stale_on}; today={today}; {seen}"),
                "set_frontmatter verified=<actor> or stale_after=<later date>".to_string(),
            ));
        }

        // V006: a person captured it and nobody has looked at it since. An
        // engram written today is left alone because it is still being worked
        // on. Once it passes the age floor V003 may speak about the same
        // engram: that overlap is accepted rather than suppressed, because the
        // two prescribe different work and V006 outranks V003 in the queue.
        if fact.is_human_authored()
            && fact.is_current()
            && fact.verified_on.is_none()
            && fact.recorded_at.is_some_and(|d| d < today)
        {
            let evidence = format!(
                "generated.by {}; recorded {}; no verified entry",
                fact.generated_by.as_deref().unwrap_or(""),
                fact.recorded_at.map(|d| d.to_string()).unwrap_or_default(),
            );
            report.findings.push(
                Finding::about("V006", fact).with(
                    Class::Judgment,
                    "captured by a person and never reviewed".to_string(),
                    evidence,
                    "review, then record a verified entry (edit_engram set_frontmatter verified)"
                        .to_string(),
                ),
            );
        }
    }

    detect_aging(input, report);
}

/// `V003`, which needs the whole domain in hand before it can pick the oldest.
fn detect_aging(input: &SweepInput, report: &mut SweepReport) {
    let mut candidates: Vec<(i64, &EngramFacts)> = Vec::new();
    for fact in &input.engrams {
        if !fact.is_current() || fact.is_speculative() {
            continue;
        }
        if fact.verified_on.is_some() || fact.stale_on.is_some() {
            continue;
        }
        let Some(age) = fact.age_days(input.today) else {
            continue;
        };
        if age > input.options.min_age_days {
            candidates.push((age, fact));
        }
    }
    // Oldest first, ties broken by address so the cut is deterministic.
    candidates.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.address().cmp(&b.1.address()))
    });

    let total = candidates.len();
    let cap = input.options.v003_cap;
    if total > cap {
        report
            .truncations
            .push(format!("V003 capped at the {cap} oldest of {total}"));
    }
    for (age, fact) in candidates.into_iter().take(cap) {
        let recorded = fact
            .recorded_at
            .map(|d| d.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        report.findings.push(Finding::about("V003", fact).with(
            Class::Judgment,
            format!("{age} days old with no verification and no staleness bound"),
            format!("recorded_at={recorded}; age {age} days; no verified entry; no stale_after"),
            "set_frontmatter verified=<actor> or stale_after=<date>".to_string(),
        ));
    }
}

/// The `V1xx` family.
fn detect_structure(input: &SweepInput, graph: &Graph<'_>, report: &mut SweepReport) {
    let linked = input
        .engrams
        .iter()
        .filter(|f| f.inbound + f.outbound > 0)
        .count();
    // A domain nobody links inside is a style, not a defect, so V104 is skipped
    // whole rather than firing on every engram in it.
    let density = if input.engrams.is_empty() {
        0.0
    } else {
        linked as f64 / input.engrams.len() as f64
    };
    let orphans_enabled = density >= input.options.orphan_density_gate;

    for fact in &input.engrams {
        if fact.is_retired() {
            continue;
        }

        // V105: over the token budget.
        if fact.token_budget > 0 && fact.tokens > fact.token_budget {
            report.findings.push(Finding::about("V105", fact).with(
                Class::Judgment,
                format!(
                    "about {} tokens over the {} token budget",
                    fact.tokens, fact.token_budget
                ),
                format!(
                    "tokens={}; budget={}; same size verify Q002 flags",
                    fact.tokens, fact.token_budget
                ),
                "split: keep the distilled summary and move the full text to sources/".to_string(),
            ));
        }

        // V101: a live engram pointing at retired knowledge.
        if fact.is_current() {
            let mut seen: BTreeSet<i64> = BTreeSet::new();
            let mut targets: Vec<String> = Vec::new();
            // The retired engrams themselves, which is what an acknowledgment
            // of this finding was given for: a link repointed or a new retired
            // target added changes it, rewording the paragraph around it does
            // not.
            let mut retired: Vec<String> = Vec::new();
            let mut repoint: Option<String> = None;
            for edge in graph.outbound_of(fact.id) {
                if edge.rel_type == "supersedes" || edge.to == fact.id {
                    continue;
                }
                let Some(status) = graph.status(edge.to) else {
                    continue;
                };
                if !is_retired_status(status) || !seen.insert(edge.to.0) {
                    continue;
                }
                let successor = graph.successor(edge.to);
                retired.push(graph.address(edge.to));
                targets.push(format!(
                    "{} is {status} via {}{}",
                    graph.address(edge.to),
                    edge.rel_type,
                    match successor {
                        Some(s) => format!(" replaced by {}", graph.address(s)),
                        None => " with no known successor".to_string(),
                    }
                ));
                if repoint.is_none()
                    && let Some(s) = successor
                {
                    repoint = Some(graph.link_text(s, &fact.domain));
                }
            }
            if !targets.is_empty() {
                let count = targets.len();
                report.findings.push(
                    Finding::about("V101", fact)
                        .with(
                            Class::Judgment,
                            format!("references {count} retired engram(s) while still current"),
                            join_semis(targets.into_iter()),
                            match repoint {
                                Some(link) => format!("repoint at {link}"),
                                None => {
                                    "repoint at the successor or keep it as a historical citation"
                                        .to_string()
                                }
                            },
                        )
                        .scoped(retired),
                );
            }
        }

        // V106: almost nothing beyond the frontmatter.
        let lines = content_line_count(&fact.body);
        if lines < MIN_CONTENT_LINES {
            report.findings.push(Finding::about("V106", fact).with(
                Class::Judgment,
                format!("only {lines} non-blank body line(s) beyond the frontmatter"),
                format!(
                    "content lines {lines}; need at least {MIN_CONTENT_LINES}; same shape verify Q001 flags"
                ),
                "enrich it; fold it into the engram that owns the topic or retire it".to_string(),
            ));
        }

        // V104: nothing points here and it points nowhere.
        if orphans_enabled
            && fact.inbound + fact.outbound == 0
            && !ORPHAN_EXEMPT_TYPES.contains(&fact.engram_type.as_str())
            && let Some(age) = fact.age_days(input.today)
            && age > input.options.orphan_min_age_days
        {
            let recorded = fact
                .recorded_at
                .map(|d| d.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let tags = if fact.tags.is_empty() {
                "no tags".to_string()
            } else {
                join_semis(fact.tags.iter().map(|t| format!("#{t}")))
            };
            report.findings.push(Finding::about("V104", fact).with(
                Class::Judgment,
                format!("no resolved links in or out after {age} days"),
                format!("recorded_at={recorded}; age {age} days; tags {tags}"),
                match fact.tags.first() {
                    Some(tag) => format!("link it to a neighbour tagged #{tag}"),
                    None => "link it or confirm standalone is intended".to_string(),
                },
            ));
        }
    }

    detect_unresolved(input, graph, report);
    detect_reciprocal(input, graph, report);
}

/// `V102`: references the index could not resolve.
///
/// The rule emits one finding per broken reference, but every finding on one
/// engram carries the **same** scope: that engram's whole set of unresolved
/// targets. One acknowledgment therefore covers what is broken now - which is
/// what a person ruling "these links point outside on purpose" means - and stops
/// matching the moment the set changes, so a newly broken link is raised rather
/// than swallowed by an older entry.
fn detect_unresolved(input: &SweepInput, graph: &Graph<'_>, report: &mut SweepReport) {
    let mut targets: HashMap<i64, Vec<String>> = HashMap::new();
    for reference in &input.unresolved {
        targets
            .entry(reference.from.0)
            .or_default()
            .push(reference.target.clone());
    }

    for reference in &input.unresolved {
        let Some(fact) = graph.facts.get(&reference.from.0) else {
            continue;
        };
        if fact.is_retired() {
            continue;
        }
        let unregistered = reference
            .target_domain
            .as_ref()
            .filter(|d| !input.known_domains.iter().any(|k| k == *d));

        let (evidence, class, fix) = match unregistered {
            Some(domain) => (
                format!(
                    "rel_type={}; target domain `{domain}` is not a registered domain",
                    reference.rel_type
                ),
                Class::Judgment,
                format!("[[{}]]", reference.target),
            ),
            None => {
                let scope = reference
                    .target_domain
                    .as_deref()
                    .unwrap_or(fact.domain.as_str());
                match title_candidate(input, graph, scope, &reference.target) {
                    Some(candidate) => (
                        format!(
                            "rel_type={}; nothing titled `{}` in {scope}; nearest is `{candidate}`",
                            reference.rel_type, reference.target
                        ),
                        Class::Mechanical,
                        format!("[[{}]] -> [[{candidate}]]", reference.target),
                    ),
                    None => (
                        format!(
                            "rel_type={}; nothing titled `{}` in {scope}; no near match",
                            reference.rel_type, reference.target
                        ),
                        Class::Judgment,
                        format!("[[{}]]", reference.target),
                    ),
                }
            }
        };

        report.findings.push(
            Finding::about("V102", fact)
                .with(
                    class,
                    format!("unresolved reference [[{}]]", reference.target),
                    evidence,
                    fix,
                )
                .at_line(reference.line)
                .scoped(targets.get(&reference.from.0).cloned().unwrap_or_default()),
        );
    }
}

/// The nearest existing title to an unresolved target inside `scope`, when it
/// is near enough that the intended target is not in doubt.
fn title_candidate(
    input: &SweepInput,
    graph: &Graph<'_>,
    scope: &str,
    target: &str,
) -> Option<String> {
    let wanted = normalize(target);
    if wanted.is_empty() {
        return None;
    }
    let mut best: Option<(f64, String)> = None;
    let mut consider = |domain: &str, title: &str, permalink: &str| {
        if domain != scope || title.is_empty() {
            return;
        }
        let score = dice_coefficient(&wanted, &normalize(title))
            .max(dice_coefficient(&wanted, &normalize(permalink)));
        if score >= TITLE_CANDIDATE_THRESHOLD
            && best
                .as_ref()
                .is_none_or(|(b, t)| score > *b || (score == *b && title < t.as_str()))
        {
            best = Some((score, title.to_string()));
        }
    };
    for fact in &input.engrams {
        consider(&fact.domain, &fact.title, &fact.permalink);
    }
    for node in &input.graph.nodes {
        if graph.facts.contains_key(&node.id.0) {
            continue;
        }
        consider(&node.domain, &node.title, &node.permalink);
    }
    best.map(|(_, title)| title)
}

/// `V103`: a reciprocal relation wired from one end only.
fn detect_reciprocal(input: &SweepInput, graph: &Graph<'_>, report: &mut SweepReport) {
    for fact in &input.engrams {
        if fact.is_retired() {
            continue;
        }
        for (forward, inverse) in RECIPROCAL_PAIRS {
            // A current engram with an inbound supersedes is V005's, and V005
            // already prescribes appending the back-link, so the supersede half
            // stays quiet rather than drawing a second finding for one fact. A
            // retired one is V004's and never reaches here at all.
            if forward == "supersedes" && fact.is_current() {
                continue;
            }
            let sources: Vec<EngramId> = graph
                .inbound_rel_sources(fact.id, forward)
                .into_iter()
                .filter(|src| {
                    !graph.outbound_of(fact.id).iter().any(|e| {
                        e.kind == EdgeKind::Relation && e.rel_type == inverse && e.to == *src
                    })
                })
                .collect();
            let Some(first) = sources.first() else {
                continue;
            };
            let counterparts: Vec<String> = sources.iter().map(|s| graph.address(*s)).collect();
            let addrs = join_semis(counterparts.iter());
            report.findings.push(
                Finding::about("V103", fact)
                    .with(
                        Class::Mechanical,
                        format!(
                            "{addrs} declares {forward} but the {inverse} back-link is missing"
                        ),
                        format!(
                            "{addrs} -{forward}-> {}; no {inverse} pointing back",
                            fact.address()
                        ),
                        format!(
                            "append `- {inverse} {}`",
                            graph.link_text(*first, &fact.domain)
                        ),
                    )
                    // The counterpart is the pair, so acknowledging a one-sided
                    // relation says nothing about the next one.
                    .scoped(counterparts),
            );
        }
    }
}

/// The `V2xx` family.
fn detect_redundancy(input: &SweepInput, report: &mut SweepReport) {
    // Retired engrams are excluded from both duplicate rules: a supersede pair
    // is supposed to say close to the same thing, and reporting it as
    // redundancy would fight the retirement that already happened.
    let live: Vec<&EngramFacts> = input.engrams.iter().filter(|f| !f.is_retired()).collect();

    let bodies: Vec<&str> = live.iter().map(|f| f.body.as_str()).collect();
    let dupes = dedupe::cluster_near_duplicates(&bodies, &input.options);
    if dupes.skipped_buckets > 0 {
        report.truncations.push(format!(
            "V201 skipped {} candidate block(s) over {} members",
            dupes.skipped_buckets, input.options.max_bucket
        ));
    }
    if dupes.capped {
        report.truncations.push(format!(
            "V201 candidate pairs capped at {}",
            input.options.max_candidate_pairs
        ));
    }

    // Which cluster each live engram landed in, for V202's suppression.
    let mut cluster_of: HashMap<usize, usize> = HashMap::new();
    for (cid, members) in dupes.clusters.iter().enumerate() {
        for m in members {
            cluster_of.insert(*m, cid);
        }
    }

    for members in &dupes.clusters {
        let Some(lead) = leader(&live, members) else {
            continue;
        };
        let others: Vec<String> = members
            .iter()
            .filter(|m| live[**m].id != live[lead].id)
            .map(|m| live[*m].address())
            .collect();
        // The whole cluster, the lead included, so the scope reads the same
        // whichever member the finding happened to hang on.
        let cluster: Vec<String> = members.iter().map(|m| live[*m].address()).collect();
        report.findings.push(
            Finding::about("V201", live[lead])
                .with(
                    Class::Judgment,
                    format!("near-duplicate of {} other engram(s)", others.len()),
                    format!(
                        "dice at or above {:.2}; also in the cluster: {}",
                        input.options.dup_threshold,
                        join_semis(others.into_iter())
                    ),
                    "merge the others into this one then supersede them".to_string(),
                )
                .scoped(cluster),
        );
    }

    detect_title_collisions(&live, &cluster_of, report);
    detect_tag_drift(input, report);
}

/// `V202`: same-domain titles that collide after the vocabulary fold.
fn detect_title_collisions(
    live: &[&EngramFacts],
    cluster_of: &HashMap<usize, usize>,
    report: &mut SweepReport,
) {
    // Group by normalized title first, so exact collisions are found even
    // though the tag clusterer only reports multi-name groups.
    let mut by_title: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, fact) in live.iter().enumerate() {
        let key = normalize(&fact.title);
        if key.is_empty() {
            continue;
        }
        by_title.entry(key).or_default().push(i);
    }

    // Then run the same separator and plural fold the tag vocabulary uses over
    // the distinct titles, so `Deploy pipeline` and `Deploy pipelines` join one
    // group too.
    let names: Vec<String> = by_title.keys().cloned().collect();
    let counts: Vec<TagCount> = names
        .iter()
        .map(|n| TagCount {
            name: n.clone(),
            engrams: 1,
            observations: 0,
        })
        .collect();
    let mut group_of: HashMap<String, usize> = HashMap::new();
    for (i, name) in names.iter().enumerate() {
        group_of.insert(name.clone(), i);
    }
    for cluster in tag_clusters(&counts) {
        let Some(root) = cluster.tags.first().and_then(|t| group_of.get(t).copied()) else {
            continue;
        };
        for name in cluster.tags {
            group_of.insert(name, root);
        }
    }

    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for name in &names {
        let root = group_of[name];
        groups
            .entry(root)
            .or_default()
            .extend(by_title[name].iter().copied());
    }

    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        // Suppressed when V201 already reported the whole group as one
        // duplicate cluster: the merge it prescribes covers the titles too.
        let first_cluster = cluster_of.get(&members[0]);
        if first_cluster.is_some() && members.iter().all(|m| cluster_of.get(m) == first_cluster) {
            continue;
        }
        let Some(lead) = leader(live, members) else {
            continue;
        };
        let listed = join_semis(
            members
                .iter()
                .map(|m| format!("{} ({})", live[*m].title, live[*m].address())),
        );
        report.findings.push(
            Finding::about("V202", live[lead])
                .with(
                    Class::Judgment,
                    format!("title collides with {} other engram(s)", members.len() - 1),
                    listed,
                    "merge them or retitle for disambiguation".to_string(),
                )
                // The colliding titles: retitling one ends the collision this
                // acknowledgment was about, a third engram joining starts a new
                // one.
                .scoped(members.iter().map(|m| live[*m].title.clone())),
        );
    }
}

/// `V203`: tag spellings that drifted apart, straight from the vocabulary
/// clusterer the `vocabulary` tool and `crystalline doctor` already use.
///
/// Usage is the tag's total: the engrams carrying it on their frontmatter plus
/// the observations carrying it inline. A tag used only on observations has an
/// engram count of zero, and counting only engrams would both print evidence
/// reading `on 0 engram(s)` and hand the canonical pick to an arbitrary member,
/// pointing the prescribed merge the wrong way.
fn detect_tag_drift(input: &SweepInput, report: &mut SweepReport) {
    let usage: HashMap<&str, i64> = input
        .tags
        .iter()
        .map(|t| (t.name.as_str(), t.engrams + t.observations))
        .collect();
    for cluster in tag_clusters_with_aliases(&input.tags, &input.tag_aliases) {
        // The most used spelling wins; ties go to the lexicographically first,
        // which the cluster's own sort already provides.
        let Some(canonical) = cluster
            .tags
            .iter()
            .max_by_key(|t| {
                (
                    usage.get(t.as_str()).copied().unwrap_or(0),
                    std::cmp::Reverse(t.as_str()),
                )
            })
            .cloned()
        else {
            continue;
        };
        let others: Vec<&String> = cluster.tags.iter().filter(|t| **t != canonical).collect();
        if others.is_empty() {
            continue;
        }
        report.findings.push(
            Finding::about_domain("V203", &input.domain).with(
                Class::Judgment,
                format!(
                    "{} tag spellings look like one tag ({})",
                    cluster.tags.len(),
                    cluster.reason
                ),
                join_semis(cluster.tags.iter().map(|t| {
                    format!(
                        "#{t} used {} time(s)",
                        usage.get(t.as_str()).copied().unwrap_or(0)
                    )
                })),
                join_semis(
                    others
                        .into_iter()
                        .map(|t| format!("crystalline tags merge {t} {canonical}")),
                ),
            ),
        );
    }
}

/// The four attachment rules: `V007` and `V008` in the temporal family,
/// `V107` and `V108` in the structural one.
///
/// One pass rather than two halves filed under their families, because all
/// four read the same pair of indexes - which engram references or claims
/// which path, and which paths the domain actually holds - and building that
/// twice would be bookkeeping no reader benefits from.
///
/// Who counts as a referent is the load-bearing part, and it matches what the
/// engine's move already decides when it works out whether an attachment may
/// leave a domain:
///
/// - a body reference is what [`crystalline_core::find_asset_refs`] found in
///   the body and a claim is the `analyzes` frontmatter key, compared as exact
///   paths;
/// - **`V108` counts live and retired engrams alike.** Retired knowledge is
///   still knowledge, and a file its text shows a reader is part of somebody's
///   teaching whatever the status says;
/// - every rule that asks an agent to act on an engram - `V007`'s anchor,
///   `V008` and `V107` - speaks only about a live one, since prescribing work
///   on retired knowledge is what retirement already answered.
///
/// `V007` and `V108` are disjoint by construction: `V007` needs a live body
/// reference, `V108` needs no reference of any kind. Both wait until the
/// attachment's modified date is behind `today`, so an upload is never nagged
/// about in the minutes between the file arriving and the engram that embeds
/// it being saved.
fn detect_attachments(input: &SweepInput, report: &mut SweepReport) {
    let rows: BTreeMap<&str, &AttachmentRow> = input
        .attachments
        .iter()
        .map(|row| (row.path.as_str(), row))
        .collect();

    // Every path anything references or claims, whatever its status, plus the
    // two live-only views the acting rules need.
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    let mut claimed_live: BTreeSet<&str> = BTreeSet::new();
    let mut first_referent: BTreeMap<&str, &EngramFacts> = BTreeMap::new();

    for fact in &input.engrams {
        let claim = fact.analyzes.as_deref();
        for path in fact.asset_refs.iter().map(String::as_str).chain(claim) {
            referenced.insert(path);
        }
        if fact.is_retired() {
            continue;
        }
        if let Some(claim) = claim {
            claimed_live.insert(claim);
        }
        for path in &fact.asset_refs {
            // Input order decides the anchor, so the same domain always hangs
            // the finding on the same engram.
            first_referent.entry(path.as_str()).or_insert(fact);
        }

        // V107: a reference or a claim naming a file the domain does not hold.
        // One finding per engram rather than per path, because the work is one
        // pass over that engram's references.
        let mut missing: BTreeMap<&str, (bool, bool)> = BTreeMap::new();
        for path in &fact.asset_refs {
            if !rows.contains_key(path.as_str()) {
                missing.entry(path.as_str()).or_default().0 = true;
            }
        }
        // A claim on its own counts, not only a claim beside a body reference:
        // an `analyzes` pointing at nothing is a promise the engram cannot
        // keep, and the evidence below names the claim as where the missing
        // path was written, which is what tells the two cases apart.
        if let Some(claim) = claim
            && !rows.contains_key(claim)
        {
            missing.entry(claim).or_default().1 = true;
        }
        if !missing.is_empty() {
            let listed = join_semis(missing.iter().map(|(path, (body, claimed))| {
                let how = match (body, claimed) {
                    (true, true) => "referenced in the body and claimed by analyzes",
                    (true, false) => "referenced in the body",
                    _ => "claimed by analyzes",
                };
                format!("{path} {how}")
            }));
            let (found, held) = if missing.len() == 1 {
                (
                    "points at an attachment that is not there".to_string(),
                    format!("nothing in {} holds that path", fact.domain),
                )
            } else {
                (
                    format!("points at {} attachments that are not there", missing.len()),
                    format!("nothing in {} holds those paths", fact.domain),
                )
            };
            report.findings.push(
                Finding::about("V107", fact)
                    .with(
                        Class::Judgment,
                        found,
                        format!("{listed}; {held}"),
                        join_semis(missing.keys()),
                    )
                    .scoped(missing.keys().map(|p| (*p).to_string())),
            );
        }

        // V008: the engram captured one version of the file and the file has
        // moved on. A claim with no recorded hash is nothing to compare, never
        // a change.
        if let Some(claim) = claim
            && let Some(row) = rows.get(claim)
            && let Some(recorded) = fact
                .analyzed_hash
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
            && !recorded.eq_ignore_ascii_case(&row.sha256)
        {
            report.findings.push(
                Finding::about("V008", fact)
                    .with(
                        Class::Judgment,
                        "the attachment changed after this engram captured it".to_string(),
                        format!(
                            "analyzes {claim}; analyzed_hash {}.. but the attachment is now {}..",
                            short_hash(recorded),
                            short_hash(&row.sha256)
                        ),
                        format!("analyzed_hash: {}", row.sha256),
                    )
                    // The path, not the hash: acknowledging "this engram is
                    // allowed to lag this file" should survive the file
                    // changing again, and stop when the claim moves.
                    .scoped([claim.to_string()]),
            );
        }
    }

    for (path, row) in &rows {
        // A stamp nobody can read is a stamp no grace period can measure, so
        // the file is left alone rather than called stale on a guess.
        let Some(modified) = attachment_date(&row.modified) else {
            continue;
        };
        if modified >= input.today {
            continue;
        }
        let described = format!(
            "{path}; {}, {} bytes; modified {modified}",
            row.mime, row.size
        );

        // V108: nothing in the domain says this file is part of its teaching.
        if !referenced.contains(path) {
            report
                .findings
                .push(Finding::about_attachment("V108", &input.domain, path).with(
                    Class::Judgment,
                    "no engram references or claims this attachment".to_string(),
                    format!("{described}; no engram references or claims it"),
                    format!("delete {path} or analyze it into an engram that references it"),
                ));
            continue;
        }

        // V007: an engram shows the file to a reader but nothing says anybody
        // read it into knowledge.
        if claimed_live.contains(path) {
            continue;
        }
        let Some(anchor) = first_referent.get(path) else {
            continue;
        };
        report.findings.push(
            Finding::about("V007", anchor)
                .with(
                    Class::Judgment,
                    "shown to a reader but never captured as knowledge".to_string(),
                    format!("{described}; no engram claims it via analyzes"),
                    format!("analyzes: {path} and analyzed_hash: {}", row.sha256),
                )
                .scoped([(*path).to_string()]),
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The date an attachment's RFC 3339 `modified` stamp falls on, or `None` when
/// the stamp does not parse. Absence fails quiet: a row nobody can date is
/// never called orphaned or unanalyzed.
fn attachment_date(modified: &str) -> Option<NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(modified.trim())
        .ok()
        .map(|dt| dt.date_naive())
}

/// The first eight characters of a hash, which is as much as evidence needs to
/// show that two hashes differ. Shorter input is quoted whole rather than
/// padded, so a hand-written `analyzed_hash` reads back as what was written.
fn short_hash(hex: &str) -> String {
    hex.chars().take(8).collect()
}

/// The member a cluster finding attaches to: the highest salience, ties broken
/// by the lowest address so the pick is deterministic. Returns an index into
/// `live`.
fn leader(live: &[&EngramFacts], members: &[usize]) -> Option<usize> {
    members.iter().copied().max_by(|a, b| {
        let sa = live[*a].salience.filter(|s| s.is_finite()).unwrap_or(0.0);
        let sb = live[*b].salience.filter(|s| s.is_finite()).unwrap_or(0.0);
        sa.total_cmp(&sb)
            .then_with(|| live[*b].address().cmp(&live[*a].address()))
    })
}

/// Join items with `; `, the separator every list-bearing cell uses. Commas
/// are avoided so a rendered table cell stays unquoted.
fn join_semis(items: impl Iterator<Item = impl std::fmt::Display>) -> String {
    items.map(|i| i.to_string()).collect::<Vec<_>>().join("; ")
}

/// The `Q001` predicate: non-blank body lines outside fenced code. The core
/// parser's line walker is crate-private, so the fence tracking is mirrored
/// here, including its rule that a closing fence must match the opening
/// character, be at least as long and carry nothing after it.
fn content_line_count(body: &str) -> usize {
    let mut fence: Option<(char, usize)> = None;
    let mut count = 0usize;
    for raw in body.split('\n') {
        let text = raw.trim_end_matches('\r');
        match fence {
            None => match fence_marker(text) {
                Some((c, n)) => fence = Some((c, n)),
                None => {
                    if !text.trim().is_empty() {
                        count += 1;
                    }
                }
            },
            Some((fc, fcount)) => {
                if let Some((c, n)) = fence_marker(text)
                    && c == fc
                    && n >= fcount
                    && text.trim_start()[n..].trim().is_empty()
                {
                    fence = None;
                }
            }
        }
    }
    count
}

/// A fence opener or closer: the marker character and its run length, for a
/// line indented no more than three spaces.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = rest.chars().take_while(|c| *c == first).count();
    if count < 3 {
        None
    } else {
        Some((first, count))
    }
}

#[cfg(test)]
mod tests;
