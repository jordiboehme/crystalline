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
pub const RECOMMENDED_STATUSES: &[&str] = &[
    "current",
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
    /// Last write timestamp, RFC 3339 with offset.
    pub timestamp: Option<DateTime<FixedOffset>>,
    /// Short description; feeds search snippets.
    pub description: Option<String>,
    /// A resource locator associated with the Engram.
    pub resource: Option<String>,
    /// Date the underlying source material carries.
    pub source_date: Option<NaiveDate>,
    /// Date the knowledge was last verified.
    pub last_verified: Option<NaiveDate>,
    /// Date after which the knowledge should be reviewed.
    pub review_after: Option<NaiveDate>,
    /// Whether temporal metadata was explicit or inferred.
    pub temporal_confidence: Option<String>,
    /// Picoschema definition, present when `type` is `schema`.
    pub schema_def: Option<SchemaDef>,
    /// Unknown keys, preserved verbatim and in original order.
    pub extra: IndexMap<String, YamlValue>,
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
            || f.timestamp.is_some()
            || f.description.is_some()
            || f.resource.is_some()
            || f.source_date.is_some()
            || f.last_verified.is_some()
            || f.review_after.is_some()
            || f.temporal_confidence.is_some()
            || f.schema_def.is_some()
            || !f.extra.is_empty()
    }
}
