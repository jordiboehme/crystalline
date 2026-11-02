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
/// `stable` is what Crystalline writes and the OKF v0.2 word for that state
/// (§5.4, where an absent status also means stable), so a foreign OKF bundle
/// reads naturally without a rewrite. `current` is the backward-compatible
/// alias meaning exactly the same thing: everything written before the flip
/// keeps its meaning, and search treats the two words as one class in both
/// directions.
pub const RECOMMENDED_STATUSES: &[&str] = &[
    "stable",
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
    /// The model that produced the words, as the agent reports it, or `None`
    /// when none was reported.
    ///
    /// A sibling key rather than a third slash segment of `by`, because `by` is
    /// `<producer>/<version>` and a reader that splits on the first slash would
    /// otherwise read the version as `2.1.271/claude-opus-5`. Absent means
    /// absent: a write that reports no model emits the two-key form exactly as
    /// it always did, and the key is left out of the serialized form too rather
    /// than sent as a null nobody can read anything out of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    /// The model the verifying agent reported, or `None` when it reported
    /// none. Read, written and serialized exactly like [`Generated::model`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When it was verified, RFC 3339 with offset.
    pub at: Option<DateTime<FixedOffset>>,
}

/// The model reported inside a provenance mapping: a non-empty string, or
/// `None`.
///
/// An empty, all-blank or non-string value reads as absence rather than as a
/// model nobody can name, which is how the write path treats one too and how
/// `generated` reads the same key (`crate::parse`), so one value means one
/// thing wherever it is written.
pub(crate) fn reported_model(value: Option<&YamlValue>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
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
            model: reported_model(map.get("model")),
            at,
        })
    }
}

/// The frontmatter key that carries the acknowledgments an engram was given:
/// findings somebody read, ruled intentional and silenced.
pub const EVOLVE_ACK_KEY: &str = "evolve_ack";

/// One acknowledged finding, the `evolve_ack` entry
/// `{ rule, scope, note, by, at }`.
///
/// Ordinary custom frontmatter rather than a typed field, so it travels with
/// team sharing, survives a resync and stays editable by hand. `scope` is the
/// evidence the acknowledgment was given for, computed by the server: absent
/// means "whatever this rule finds here", which is what a hand-written entry
/// gets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvolveAck {
    /// The rule id it silences, for example `V101`.
    pub rule: String,
    /// The evidence it was given for, or `None` for an entry that matches
    /// whatever the rule finds.
    pub scope: Option<String>,
    /// Why the finding is intentional, in the acknowledger's words.
    pub note: Option<String>,
    /// The actor that acknowledged it, the OKF actor convention.
    pub by: String,
    /// When it was acknowledged, RFC 3339 with offset.
    pub at: Option<DateTime<FixedOffset>>,
}

impl EvolveAck {
    /// Parse an `evolve_ack` frontmatter value into entries, accepting a bare
    /// mapping as a one-element list exactly as `verified` does.
    ///
    /// Lenient on purpose, and the one place that leniency is decided: an entry
    /// that names no rule is skipped and everything else about it is taken as
    /// written, because this key is meant to be hand-editable and a typo in one
    /// entry must never cost the others or fail a sweep. An `at` that does not
    /// parse simply leaves the instant unknown.
    pub fn parse_list(value: &YamlValue) -> Vec<EvolveAck> {
        match value {
            YamlValue::Mapping(_) => EvolveAck::parse_entry(value).into_iter().collect(),
            YamlValue::Sequence(items) => items.iter().filter_map(EvolveAck::parse_entry).collect(),
            _ => Vec::new(),
        }
    }

    fn parse_entry(value: &YamlValue) -> Option<EvolveAck> {
        let map = value.as_mapping()?;
        let text = |key: &str| {
            map.get(key)
                .and_then(YamlValue::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let rule = text("rule")?;
        Some(EvolveAck {
            rule,
            scope: text("scope"),
            note: text("note"),
            by: text("by").unwrap_or_default(),
            at: map
                .get("at")
                .and_then(YamlValue::as_str)
                .and_then(|raw| DateTime::parse_from_rfc3339(raw.trim()).ok()),
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
///
/// A bullet that wraps onto further lines is one observation: its
/// continuation lines are joined into the text with single spaces, so the
/// trailing tags and context are read from the end of the whole bullet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Observation {
    /// One-based line number in the source file: the bullet's own line, also
    /// when the bullet wraps onto further lines.
    pub line: usize,
    /// One-based line number of the last source line the observation's text
    /// was read from. Equal to [`Observation::line`] for a bullet on one line;
    /// for a wrapped bullet it is its last continuation line, so a caller that
    /// moves or rewrites the bullet takes `line..=end_line`. Not serialized:
    /// the wire shape stays the bullet's first line.
    #[serde(skip)]
    pub end_line: usize,
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
    /// One-based line number in the source file: the bullet's own line, also
    /// when the bullet wraps onto further lines.
    pub line: usize,
    /// One-based line number of the last source line the relation was read
    /// from; see [`Observation::end_line`]. Not serialized.
    #[serde(skip)]
    pub end_line: usize,
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

/// A link target, optionally carrying an explicit cross-domain prefix and a
/// `#heading` fragment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct LinkTarget {
    /// The domain named by a `[[domain:Target]]` prefix, if any.
    pub domain: Option<String>,
    /// The target title or permalink: the part before the first `#`, trimmed.
    pub target: String,
    /// The `#heading` of a `[[target#heading]]` wikilink: the text after the
    /// first `#` of the target part, trimmed. `None` when there is no `#`, or
    /// nothing after it. It names a place inside the target and never changes
    /// which engram the link lands on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment: Option<String>,
    /// The target as 0.30.0 read it, the `#` and all, trimmed: `C# Basics`
    /// for `[[C# Basics]]`, whose target reads `C`. Set whenever a `#` was
    /// split off a non-empty target and the reading without the split names
    /// the same domain. The resolver tries it as a title first, so an engram
    /// whose own title holds a `#` stays reachable by it. Not part of the
    /// serialized shape.
    #[serde(skip)]
    pub whole: Option<String>,
    /// The target part of the bracket text as it was written: everything
    /// before the first `|` (the display text is [`LinkTarget::alias`]) and
    /// before the `#` of a fragment ([`LinkTarget::fragment`]), trimmed at the
    /// ends and nothing else.
    ///
    /// The parse below is domain-agnostic: it cannot know whether the segment
    /// before a colon names a real domain, so a title whose own first word ends
    /// in a colon (`Log: Weekly Garden Notes`) splits like a cross-domain
    /// prefix. Only a resolver holding the registry can tell the two apart, and
    /// telling them apart means looking the whole original string up as a
    /// title, which the split has by then thrown away. So it is kept here.
    /// Not part of the serialized shape: it is what the parse consumed, not a
    /// second field a client should read.
    #[serde(skip)]
    pub raw: String,
    /// How the reference was written: a `[[...]]` wikilink or a relative
    /// Markdown link to another engram's file. Left out of the serialized
    /// shape for a wikilink, so a wikilink reads as it always did.
    #[serde(default, skip_serializing_if = "LinkForm::is_wiki")]
    pub form: LinkForm,
    /// The display text of a `[[target|alias]]` wikilink: what follows the
    /// first `|`, trimmed. `None` when there is no `|` or nothing after it.
    /// It only changes what a reader sees, never where the link points, so it
    /// is not part of the serialized shape.
    #[serde(skip)]
    pub alias: Option<String>,
}

/// How a [`LinkTarget`] was written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkForm {
    /// `[[Target]]` or `[[domain:Target]]`: the target is a title or a
    /// permalink.
    #[default]
    Wiki,
    /// `[text](../folder/file.md)`: the target is a path relative to the
    /// folder of the engram that holds the link, percent-decoded and without
    /// its `#fragment`.
    Relative,
}

impl LinkForm {
    /// Whether this is the wikilink form, the one left out of the serialized
    /// shape.
    pub fn is_wiki(&self) -> bool {
        *self == LinkForm::Wiki
    }
}

impl LinkTarget {
    /// A relative Markdown link's target: `path` is the destination as
    /// [`crate::parse`] read it, percent-decoded and without its `#fragment`.
    pub fn relative(path: &str) -> LinkTarget {
        LinkTarget {
            domain: None,
            target: path.to_string(),
            fragment: None,
            whole: None,
            raw: path.to_string(),
            form: LinkForm::Relative,
            alias: None,
        }
    }

    /// The target an index row stores for this reference, written in the
    /// engram at the domain-relative `from_path`. A wikilink stores its target
    /// text as it is. A relative link stores the domain-relative path it lands
    /// on (`pumps/p-12.md`), which is what binds it to the engram at that path;
    /// one that climbs out of the domain keeps the target as written, which no
    /// engram's path equals, so it stays unresolved.
    pub fn stored_target(&self, from_path: &str) -> String {
        match self.form {
            LinkForm::Wiki => self.target.clone(),
            LinkForm::Relative => crate::reflection::links::resolve(from_path, &self.target)
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| self.target.clone()),
        }
    }

    /// Parse the inside of a `[[...]]` into a target. The text after the
    /// first `|` is the display text ([`LinkTarget::alias`]); the text after
    /// the first `#` of the part before it is the fragment
    /// ([`LinkTarget::fragment`]), split off only when something stands
    /// before the `#` (`[[#setup]]` stays the target `#setup`). On the part
    /// before the `#`, a single leading colon group is treated as a
    /// cross-domain prefix; further colons stay in the target text.
    pub fn parse(inner: &str) -> LinkTarget {
        let (part, alias) = split_alias(inner);
        let alias = alias.map(str::to_string);
        let (before, fragment) = split_fragment(part);
        let Some(fragment) = fragment else {
            return read_prefix(part, alias);
        };
        let mut link = read_prefix(before.trim(), alias);
        // The reading 0.30.0 took, with the `#` left in the target. Kept as
        // the whole text only when it names the same domain, so a title
        // holding a `#` is looked up where the link points.
        let unsplit = read_prefix(part, None);
        if unsplit.domain == link.domain {
            link.whole = Some(unsplit.target);
        }
        let fragment = fragment.trim();
        link.fragment = (!fragment.is_empty()).then(|| fragment.to_string());
        link
    }

    /// The target part written back: `[domain:]target[#fragment]`, never the
    /// display text. What a rewrite or a message that rebuilds bracket text
    /// writes.
    pub fn written(&self) -> String {
        let mut out = String::new();
        if let Some(domain) = &self.domain {
            out.push_str(domain);
            out.push(':');
        }
        out.push_str(&self.target);
        if let Some(fragment) = &self.fragment {
            out.push('#');
            out.push_str(fragment);
        }
        out
    }

    /// Whether two targets point at the same place: equal in everything but
    /// the display text, which only changes what a reader sees.
    pub fn same_place(&self, other: &LinkTarget) -> bool {
        self.domain == other.domain
            && self.target == other.target
            && self.fragment == other.fragment
            && self.raw == other.raw
            && self.form == other.form
    }

    /// [`LinkTarget::parse`] for the inside of a `[[...]]` found in a body, or
    /// `None` when it is not a link: a `|` with nothing before it (`[[|b]]`)
    /// names no target, so it stays the text it was written as.
    pub fn wikilink(inner: &str) -> Option<LinkTarget> {
        if inner.contains('|') && split_alias(inner).0.is_empty() {
            return None;
        }
        Some(LinkTarget::parse(inner))
    }
}

/// The domain prefix of a trimmed target part, read the one way every
/// wikilink is read: a single leading colon group is a prefix when both sides
/// are non-empty and the domain segment holds no whitespace.
fn read_prefix(part: &str, alias: Option<String>) -> LinkTarget {
    if let Some((domain, rest)) = part.split_once(':') {
        let domain = domain.trim();
        let rest = rest.trim();
        // Only treat it as a domain prefix when both sides look like a
        // plausible domain and target (no spaces in the domain segment).
        if !domain.is_empty() && !rest.is_empty() && !domain.contains(char::is_whitespace) {
            return LinkTarget {
                domain: Some(domain.to_string()),
                target: rest.to_string(),
                fragment: None,
                whole: None,
                raw: part.to_string(),
                form: LinkForm::Wiki,
                alias,
            };
        }
    }
    LinkTarget {
        domain: None,
        target: part.to_string(),
        fragment: None,
        whole: None,
        raw: part.to_string(),
        form: LinkForm::Wiki,
        alias,
    }
}

/// Split the target part of a wikilink (the text before its `|`) at its
/// first `#` into the part before it and the fragment text after it, both as
/// written, untrimmed. No split, and `None` for the fragment, when there is
/// no `#` or nothing but whitespace stands before it (`[[#setup]]`). The one
/// place the split is spelled, so the parser and every rewrite that keeps the
/// fragment agree on where the target ends.
pub fn split_fragment(part: &str) -> (&str, Option<&str>) {
    match part.split_once('#') {
        Some((before, after)) if !before.trim().is_empty() => (before, Some(after)),
        _ => (part, None),
    }
}

/// Split the inside of a `[[...]]` at its first `|` into the target part and
/// the display text, both trimmed. An empty display text is no display text.
/// The one place the split is spelled, so the parser and every rewrite that
/// keeps the display text agree on where the target ends.
pub fn split_alias(inner: &str) -> (&str, Option<&str>) {
    match inner.split_once('|') {
        Some((target, alias)) => {
            let alias = alias.trim();
            (target.trim(), (!alias.is_empty()).then_some(alias))
        }
        None => (inner.trim(), None),
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

    /// The filterable metadata of this Engram: every frontmatter key that is
    /// not promoted to a column of its own, as a JSON object. That is every
    /// preserved unknown key (a known key whose value did not parse lands
    /// there too), plus the optional known fields that have no column. The
    /// promoted keys (type, status, title, permalink, the temporal window,
    /// description) and tags are stored in their own columns and join tables
    /// and never appear here, and neither do the provenance and schema keys.
    ///
    /// Two users read this one map. The index stores it as the `metadata`
    /// column and derives its `engram_meta_value` rows from it, one per list
    /// element and per scalar. Verify reads the same map to warn about a key
    /// or value too long for those rows. So the keys of this map are the only
    /// keys that can get rows: a key gets none when it is longer than
    /// [`META_KEY_MAX_BYTES`], or when none of its values has a text (see
    /// [`meta_value_text`]).
    pub fn index_metadata(&self) -> serde_json::Map<String, serde_json::Value> {
        let date_str = |d: Option<NaiveDate>| d.map(|d| d.format("%Y-%m-%d").to_string());
        let mut meta = serde_json::Map::new();
        for (k, v) in &self.extra {
            if let Ok(jv) = serde_json::to_value(v) {
                meta.insert(k.clone(), jv);
            }
        }
        let mut add_str = |key: &str, value: Option<String>| {
            if let Some(v) = value {
                meta.insert(key.to_string(), serde_json::Value::String(v));
            }
        };
        add_str("source_date", date_str(self.source_date));
        // `last_verified` and `stale_after` carry the effective value whichever
        // spelling recorded it, so a filter keeps working across an engram that
        // has migrated to the OKF keys and one that has not.
        add_str(
            "last_verified",
            self.latest_verified()
                .map(|v| v.at.date_naive().format("%Y-%m-%d").to_string()),
        );
        add_str("stale_after", date_str(self.stale_on()));
        add_str("temporal_confidence", self.temporal_confidence.clone());
        add_str("resource", self.resource.clone());
        // The verification trail itself stays filterable in its OKF shape.
        if !self.verified.is_empty()
            && let Ok(jv) = serde_json::to_value(&self.verified)
        {
            meta.insert("verified".to_string(), jv);
        }
        meta
    }
}

/// The longest frontmatter key, in bytes, that gets `engram_meta_value` rows
/// in the index. A longer key gets no rows, and a `$contains` on it matches
/// nothing.
///
/// Two users measure against this one value: the index, when it writes the
/// rows, and verify, which warns about a key that is too long.
pub const META_KEY_MAX_BYTES: usize = 256;

/// The longest value text, in bytes, that an `engram_meta_value` row holds
/// (see [`meta_value_text`]). Postgres refuses a btree entry above about
/// 2.7 KB, and every scalar of a non-promoted key gets a row, so one long
/// custom text field would otherwise fail its engram's write there. Both
/// index backends apply the same cap, so they keep answering the same.
///
/// Two users measure against this one value: the index, when it writes the
/// rows, and verify, which warns about a value that is too long.
pub const META_VALUE_MAX_BYTES: usize = 1024;

/// The text a frontmatter value is stored and compared as in the index's
/// `engram_meta_value` table: its compact JSON. So the string `"1"`, the
/// number `1` and the boolean `true` are three values, and a string needs no
/// escaping rule of its own.
///
/// `None` in two different cases: the value is a list or an object (those
/// never get a row of their own), or its text is longer than
/// [`META_VALUE_MAX_BYTES`]. A caller that must tell the two apart checks the
/// value's type first.
///
/// Two users go through this one function: the index (its writer, its
/// migration and both query builders), and verify, so verify measures a
/// value byte for byte the way the index does.
pub fn meta_value_text(value: &serde_json::Value) -> Option<String> {
    if value.is_array() || value.is_object() {
        return None;
    }
    let text = value.to_string();
    (text.len() <= META_VALUE_MAX_BYTES).then_some(text)
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

#[cfg(test)]
mod meta_value_tests {
    use super::{META_VALUE_MAX_BYTES, meta_value_text};
    use serde_json::json;

    /// A value is stored as its compact JSON, so the string "1", the number
    /// 1 and the boolean true are three different values, and the escapes
    /// are serde_json's wherever a value is written or compared.
    #[test]
    fn a_value_keeps_its_json_type() {
        assert_eq!(meta_value_text(&json!("1")).as_deref(), Some("\"1\""));
        assert_eq!(meta_value_text(&json!(1)).as_deref(), Some("1"));
        assert_eq!(meta_value_text(&json!(true)).as_deref(), Some("true"));
        assert_eq!(meta_value_text(&json!(1.5)).as_deref(), Some("1.5"));
        assert_eq!(meta_value_text(&json!(-3)).as_deref(), Some("-3"));
        assert_eq!(
            meta_value_text(&json!(i64::MAX)).as_deref(),
            Some("9223372036854775807")
        );
        assert_eq!(meta_value_text(&json!(null)).as_deref(), Some("null"));
        assert_eq!(
            meta_value_text(&json!("a \"q\" \\ \n ü")).as_deref(),
            Some("\"a \\\"q\\\" \\\\ \\n ü\"")
        );
        assert_eq!(meta_value_text(&json!(["a"])), None);
        assert_eq!(meta_value_text(&json!({"a": 1})), None);
    }

    /// Postgres refuses a btree entry above about 2.7 KB, so a long value
    /// gets no row on either backend instead of failing the engram's write.
    #[test]
    fn a_value_longer_than_the_cap_has_no_text() {
        let fits = "x".repeat(META_VALUE_MAX_BYTES - 2);
        assert_eq!(
            meta_value_text(&json!(fits.as_str())).map(|t| t.len()),
            Some(META_VALUE_MAX_BYTES)
        );
        assert_eq!(meta_value_text(&json!(format!("{fits}x"))), None);
    }
}

#[cfg(test)]
mod link_target_tests {
    use super::LinkTarget;

    #[test]
    fn an_alias_is_the_text_after_the_first_pipe() {
        let link = LinkTarget::parse("pumps/p-12|the pump");
        assert_eq!(link.domain, None);
        assert_eq!(link.target, "pumps/p-12");
        assert_eq!(link.alias.as_deref(), Some("the pump"));
        assert_eq!(link.raw, "pumps/p-12", "raw is the target part only");

        let link = LinkTarget::parse("ops:pumps/p-12|x");
        assert_eq!(link.domain.as_deref(), Some("ops"));
        assert_eq!(link.target, "pumps/p-12");
        assert_eq!(link.alias.as_deref(), Some("x"));

        // Split at the first pipe; the rest is display text, colons included.
        let link = LinkTarget::parse(" a | b|c: d ");
        assert_eq!(link.target, "a");
        assert_eq!(link.alias.as_deref(), Some("b|c: d"));

        // An empty alias is no alias.
        let link = LinkTarget::parse("a| ");
        assert_eq!(link.target, "a");
        assert_eq!(link.alias, None);

        // An empty target is not a link at all.
        assert!(LinkTarget::wikilink("|b").is_none());
        assert!(LinkTarget::wikilink(" | b").is_none());
        assert!(LinkTarget::wikilink("a|b").is_some());
        assert_eq!(LinkTarget::parse("plain").alias, None);
    }

    #[test]
    fn a_heading_after_the_target_is_a_fragment() {
        let link = LinkTarget::parse("b#setup");
        assert_eq!(link.domain, None);
        assert_eq!(link.target, "b");
        assert_eq!(link.fragment.as_deref(), Some("setup"));
        assert_eq!(link.whole.as_deref(), Some("b#setup"));
        assert_eq!(link.raw, "b");
        assert_eq!(link.written(), "b#setup");

        let link = LinkTarget::parse("b#Setup Guide|see");
        assert_eq!(link.target, "b");
        assert_eq!(link.fragment.as_deref(), Some("Setup Guide"));
        assert_eq!(link.whole.as_deref(), Some("b#Setup Guide"));
        assert_eq!(link.alias.as_deref(), Some("see"));
        assert_eq!(link.raw, "b");
        assert_eq!(link.written(), "b#Setup Guide", "never the alias");
    }

    #[test]
    fn the_prefix_is_read_on_the_part_before_the_fragment() {
        let link = LinkTarget::parse("ops:b#x:y");
        assert_eq!(link.domain.as_deref(), Some("ops"));
        assert_eq!(link.target, "b");
        assert_eq!(link.fragment.as_deref(), Some("x:y"));
        assert_eq!(link.whole.as_deref(), Some("b#x:y"));
        assert_eq!(link.raw, "ops:b");
        assert_eq!(link.written(), "ops:b#x:y");

        // The unregistered-prefix fallback reads `raw`, which holds no
        // fragment.
        let link = LinkTarget::parse("Log: Weekly#x");
        assert_eq!(link.domain.as_deref(), Some("Log"));
        assert_eq!(link.target, "Weekly");
        assert_eq!(link.fragment.as_deref(), Some("x"));
        assert_eq!(link.whole.as_deref(), Some("Weekly#x"));
        assert_eq!(link.raw, "Log: Weekly");
    }

    #[test]
    fn an_empty_fragment_or_an_empty_target_is_no_split() {
        let link = LinkTarget::parse("b#");
        assert_eq!(link.target, "b");
        assert_eq!(link.fragment, None);
        assert_eq!(link.whole.as_deref(), Some("b#"));
        assert_eq!(link.raw, "b");

        let link = LinkTarget::parse("#setup");
        assert_eq!(link.target, "#setup");
        assert_eq!(link.fragment, None);
        assert_eq!(link.whole, None);
        assert_eq!(link.raw, "#setup");

        let link = LinkTarget::parse("plain");
        assert_eq!(link.fragment, None);
        assert_eq!(link.whole, None);
        assert_eq!(link.written(), "plain");
    }

    #[test]
    fn a_split_that_moves_the_prefix_keeps_no_whole_text() {
        // Read whole, `a#b` would be the prefix; split, there is none.
        let link = LinkTarget::parse("a#b:c");
        assert_eq!(link.domain, None);
        assert_eq!(link.target, "a");
        assert_eq!(link.fragment.as_deref(), Some("b:c"));
        assert_eq!(link.whole, None);
    }

    #[test]
    fn the_fragment_is_part_of_the_place_and_is_serialized() {
        let a = LinkTarget::parse("b#setup");
        let b = LinkTarget::parse("b#setup|text");
        let c = LinkTarget::parse("b");
        assert!(a.same_place(&b));
        assert!(!a.same_place(&c));
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["fragment"], "setup");
        assert!(json.get("whole").is_none());
        assert!(serde_json::to_value(&c).unwrap().get("fragment").is_none());
    }
}
