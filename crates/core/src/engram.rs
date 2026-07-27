//! Engram is the unit of knowledge in Crystalline: one markdown file with
//! YAML frontmatter, stored inside a Domain. This module holds the
//! frontmatter and body model plus the small value types extracted from the
//! body (observations, relations, wikilinks, headings). Parsing lives in
//! [`crate::parse`] and deterministic emission in [`crate::emit`].

use chrono::{DateTime, FixedOffset, NaiveDate};
use indexmap::IndexMap;
use serde::Serialize;

use crate::yaml::YamlValue;

/// Recommended values for the `type` frontmatter field. These are guidance
/// surfaced in documentation and tool descriptions only. Any non-empty
/// string is a valid type; this set is never used to reject an Engram.
pub const RECOMMENDED_TYPES: &[&str] = &[
    "manifest",
    "schema",
    "engram",
    "guide",
    "decision",
    "architecture",
    "runbook",
    "reference",
];

/// Recommended values for the `status` frontmatter field. Guidance only; the
/// purpose of status is letting an agent tell an idea or draft apart from
/// current fact, not taxonomy policing. Never used to reject an Engram.
///
/// `current` is what Crystalline writes and `stable` is the OKF v0.2 word for
/// the same state (§5.4, where an absent status also means stable), so both are
/// recommended and a foreign OKF bundle reads naturally without a rewrite.
pub const RECOMMENDED_STATUSES: &[&str] = &[
    "current",
    "stable",
    "implemented",
    "draft",
    "proposed",
    "idea",
    "poc",
    "deprecated",
    "superseded",
    "archived",
    "legacy",
];

/// A parsed Engram: typed frontmatter, the verbatim body and the structured
/// elements scanned out of the body.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Engram {
    /// Typed frontmatter with unknown keys preserved in `extra`.
    pub frontmatter: Frontmatter,
    /// The body text exactly as it appeared after the closing delimiter,
    /// including any leading blank line.
    pub body: String,
    /// Top-level observation bullets.
    pub observations: Vec<Observation>,
    /// Top-level relation bullets.
    pub relations: Vec<Relation>,
    /// Prose wikilinks (excluding relation targets), deduplicated per line.
    pub links: Vec<WikiLink>,
    /// ATX headings found outside code fences.
    pub headings: Vec<Heading>,
}

/// Typed Engram frontmatter.
///
/// Temporal semantics are open ended: an absent `valid_from` means the
/// knowledge has always been valid and an absent `valid_to` means it is valid
/// forever. Sentinel dates are never emitted.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Frontmatter {
    /// The `type` field. Required by OKF; empty string when absent.
    pub engram_type: String,
    /// The `title` field. Empty string when absent.
    pub title: String,
    /// Domain-relative slug path, without a domain prefix.
    pub permalink: Option<String>,
    /// Tags, normalized from a list or a comma-separated string.
    pub tags: Vec<String>,
    /// Free-form lifecycle status.
    pub status: Option<String>,
    /// When the knowledge was recorded.
    pub recorded_at: Option<NaiveDate>,
    /// Start of the validity window; absent means always valid.
    pub valid_from: Option<NaiveDate>,
    /// End of the validity window; absent means valid forever.
    pub valid_to: Option<NaiveDate>,
    /// Write provenance: who wrote the Engram last and when. The OKF v0.2
    /// `generated` family, which supersedes the v0.1 `timestamp` key.
    pub generated: Option<Generated>,
    /// Last write timestamp, RFC 3339 with offset. The legacy OKF v0.1 key,
    /// still read so an engram written before the `generated` migration keeps
    /// its recency; new writes emit [`Frontmatter::generated`] instead.
    pub timestamp: Option<DateTime<FixedOffset>>,
    /// Short description; feeds search snippets.
    pub description: Option<String>,
    /// A resource locator associated with the Engram.
    pub resource: Option<String>,
    /// Date the underlying source material carries.
    pub source_date: Option<NaiveDate>,
    /// The verification trail: who checked this knowledge and when. The OKF
    /// v0.2 `verified` family, which supersedes the actorless `last_verified`
    /// key. Empty when the Engram carries no verification.
    pub verified: Vec<Verified>,
    /// Date the knowledge was last verified. The legacy key, still read so an
    /// Engram written before the `verified` migration keeps its trust record;
    /// new verifications are recorded as [`Frontmatter::verified`] entries.
    pub last_verified: Option<NaiveDate>,
    /// Date at or after which the knowledge counts as stale. The OKF v0.2
    /// `stale_after` family, which supersedes `review_after`.
    pub stale_after: Option<NaiveDate>,
    /// Date after which the knowledge should be reviewed. The legacy spelling
    /// of [`Frontmatter::stale_after`], still read so an Engram written before
    /// the migration keeps its staleness bound.
    pub review_after: Option<NaiveDate>,
    /// Whether temporal metadata was explicit or inferred.
    pub temporal_confidence: Option<String>,
    /// Picoschema definition, present when `type` is `schema`.
    pub schema_def: Option<SchemaDef>,
    /// Unknown keys, preserved verbatim and in original order.
    pub extra: IndexMap<String, YamlValue>,
}

/// Write provenance, the OKF v0.2 `generated: { by, at }` mapping: the actor
/// that produced this revision and when it did.
///
/// `by` follows the OKF actor convention: an agent is `name/version`, a person
/// is `human:name` and an automated job is `process:name`. `at` is optional in
/// the model so a hand-written `generated` block with only an actor still
/// parses, but everything Crystalline writes carries both.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Generated {
    /// The actor that wrote this revision.
    pub by: String,
    /// When it was written, RFC 3339 with offset.
    pub at: Option<DateTime<FixedOffset>>,
}

/// One verification, the OKF v0.2 `verified` entry `{ by, at }`: the actor that
/// checked this knowledge and when it did.
///
/// `by` follows the OKF actor convention, exactly like [`Generated::by`]. `at`
/// is optional in the model so a hand-written entry naming only an actor still
/// parses, but everything Crystalline writes carries both. A bare mapping in
/// the frontmatter parses as a one-element list, as the spec requires.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verified {
    /// The actor that verified the knowledge.
    pub by: String,
    /// When it was verified, RFC 3339 with offset.
    pub at: Option<DateTime<FixedOffset>>,
}

impl Verified {
    /// Parse a `verified` frontmatter value into entries, accepting a bare
    /// mapping as a one-element list (OKF v0.2 §11). Returns `None` when the
    /// value is not a well-formed entry or list of entries: an entry needs a
    /// non-empty `by`, and an `at` that is present must be a parseable RFC 3339
    /// instant. A malformed value is kept verbatim instead, so nothing is lost
    /// and verify can flag it.
    pub fn parse_list(value: &YamlValue) -> Option<Vec<Verified>> {
        match value {
            YamlValue::Mapping(_) => Verified::parse_entry(value).map(|e| vec![e]),
            YamlValue::Sequence(items) if !items.is_empty() => {
                items.iter().map(Verified::parse_entry).collect()
            }
            _ => None,
        }
    }

    fn parse_entry(value: &YamlValue) -> Option<Verified> {
        let map = value.as_mapping()?;
        let by = map.get("by")?.as_str()?.trim();
        if by.is_empty() {
            return None;
        }
        let at = match map.get("at") {
            Some(raw) => Some(DateTime::parse_from_rfc3339(raw.as_str()?).ok()?),
            None => None,
        };
        Some(Verified {
            by: by.to_string(),
            at,
        })
    }
}

/// The most recent verification recorded on an Engram: the actor when one is
/// known and the instant it happened.
#[derive(Debug, Clone, PartialEq)]
pub struct Verification<'a> {
    /// The actor that verified the knowledge, absent for a legacy
    /// `last_verified` date, which records no actor.
    pub by: Option<&'a str>,
    /// When the verification happened.
    pub at: DateTime<FixedOffset>,
}

/// The schema-defining frontmatter block of a `type: schema` Engram. The raw
/// declaration strings and values are kept so the block round-trips exactly;
/// [`crate::schema::Schema`] parses them into structured field declarations.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SchemaDef {
    /// The entity type this schema governs.
    pub entity: Option<String>,
    /// Schema version.
    pub version: Option<i64>,
    /// Body declarations: declaration string to type or nested value.
    pub schema: IndexMap<String, YamlValue>,
    /// Settings such as `validation` and `frontmatter`.
    pub settings: IndexMap<String, YamlValue>,
}

/// A top-level observation bullet: `- [category] content #tag (context)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Observation {
    /// One-based line number in the source file.
    pub line: usize,
    /// The single bracket token category.
    pub category: String,
    /// The observation text with trailing tags and context removed.
    pub content: String,
    /// Trailing hashtags, in order, without the leading `#`.
    pub tags: Vec<String>,
    /// A trailing parenthesized group, if present.
    pub context: Option<String>,
}

/// A top-level relation bullet: `- rel_type [[Target]]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Relation {
    /// One-based line number in the source file.
    pub line: usize,
    /// The relation type; a single token or a quoted phrase.
    pub rel_type: String,
    /// The link target.
    pub target: LinkTarget,
}

/// A prose wikilink `[[Target]]` or `[[domain:Target]]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WikiLink {
    /// One-based line number in the source file.
    pub line: usize,
    /// The link target.
    pub target: LinkTarget,
}

/// A link target, optionally carrying an explicit cross-domain prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct LinkTarget {
    /// The domain named by a `[[domain:Target]]` prefix, if any.
    pub domain: Option<String>,
    /// The target title or permalink.
    pub target: String,
}

impl LinkTarget {
    /// Parse the inside of a `[[...]]` into a target. A single leading colon
    /// group is treated as a cross-domain prefix; further colons stay in the
    /// target text.
    pub fn parse(inner: &str) -> LinkTarget {
        let inner = inner.trim();
        if let Some((domain, rest)) = inner.split_once(':') {
            let domain = domain.trim();
            let rest = rest.trim();
            // Only treat it as a domain prefix when both sides look like a
            // plausible domain and target (no spaces in the domain segment).
            if !domain.is_empty() && !rest.is_empty() && !domain.contains(char::is_whitespace) {
                return LinkTarget {
                    domain: Some(domain.to_string()),
                    target: rest.to_string(),
                };
            }
        }
        LinkTarget {
            domain: None,
            target: inner.to_string(),
        }
    }
}

/// An ATX heading (`#` through `######`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Heading {
    /// One-based line number in the source file.
    pub line: usize,
    /// Heading level, 1 through 6.
    pub level: u8,
    /// Heading text with leading and trailing hashes and spaces removed.
    pub text: String,
}

impl Frontmatter {
    /// When this Engram was last written, for recency: `generated.at` when the
    /// OKF v0.2 provenance block is present, falling back to the legacy
    /// `timestamp` key otherwise (OKF v0.2 §13.1). Every recency consumer reads
    /// this rather than either field directly, so a file that has not been
    /// migrated yet ranks exactly as it did before.
    pub fn written_at(&self) -> Option<DateTime<FixedOffset>> {
        self.generated
            .as_ref()
            .and_then(|g| g.at)
            .or(self.timestamp)
    }

    /// The date at or after which this Engram counts as stale: `stale_after`,
    /// falling back to the legacy `review_after` spelling. Every staleness
    /// consumer reads this rather than either field directly, so a file that
    /// has not been migrated yet behaves exactly as it did before.
    pub fn stale_on(&self) -> Option<NaiveDate> {
        self.stale_after.or(self.review_after)
    }

    /// The newest verification recorded on this Engram: the latest `verified`
    /// entry that carries an instant, falling back to the legacy
    /// `last_verified` date read as a verification at midnight UTC by an
    /// unnamed actor. `None` when nothing has been verified.
    pub fn latest_verified(&self) -> Option<Verification<'_>> {
        let newest = self
            .verified
            .iter()
            .filter_map(|v| v.at.map(|at| (at, v.by.as_str())))
            .max_by_key(|(at, _)| *at);
        if let Some((at, by)) = newest {
            return Some(Verification { by: Some(by), at });
        }
        let at = self.last_verified?.and_hms_opt(0, 0, 0)?.and_utc();
        Some(Verification {
            by: None,
            at: at.fixed_offset(),
        })
    }
}

impl Engram {
    /// True when the frontmatter carries no representable field. Used by the
    /// emitter to decide whether to write a frontmatter block at all.
    pub fn has_frontmatter_fields(&self) -> bool {
        let f = &self.frontmatter;
        !f.engram_type.is_empty()
            || !f.title.is_empty()
            || f.permalink.is_some()
            || !f.tags.is_empty()
            || f.status.is_some()
            || f.recorded_at.is_some()
            || f.valid_from.is_some()
            || f.valid_to.is_some()
            || f.generated.is_some()
            || f.timestamp.is_some()
            || f.description.is_some()
            || f.resource.is_some()
            || f.source_date.is_some()
            || !f.verified.is_empty()
            || f.last_verified.is_some()
            || f.stale_after.is_some()
            || f.review_after.is_some()
            || f.temporal_confidence.is_some()
            || f.schema_def.is_some()
            || !f.extra.is_empty()
    }
}
