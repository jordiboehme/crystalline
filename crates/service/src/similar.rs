//! The neighbours advisory: what a write or a content edit is close to.
//!
//! An agent that writes is an agent in the loop, so the cheapest duplicate
//! check there is hands it the three existing engrams nearest in meaning to
//! what it just wrote and lets it decide. No threshold decides anything here:
//! the one measurement that exists says ranking is reliable where a score is
//! not (research/2026-09-07-contradiction-handling-external-survey.md, A and
//! D). This module owns the probe text, the receipt shape and the guidance;
//! the retrieval lives on the engine, which holds the provider and the store.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::params::{EditParams, WriteParams};

/// The probe is capped to mirror the leading indexed chunk, so the receipt
/// compares close to what search compared. Close, not identical:
/// `build_header` joins title and description with a blank line where this
/// joins every part with one newline, a difference no embedder makes anything
/// of.
pub const SIMILAR_PROBE_MAX_CHARS: usize = 900;
/// Below this a probe is a title and a fragment, and the neighbours it finds
/// are noise; the write is left quiet instead.
pub const SIMILAR_PROBE_MIN_CHARS: usize = 80;
/// One more than the receipt carries, so excluding the engram itself still
/// leaves a full list. Two predicates filter the page though, not one: a
/// retired hit inside it costs a slot, and no deeper hit refills that slot.
pub const SIMILAR_PAGE: usize = 4;
/// How many neighbours a receipt names.
pub const SIMILAR_LIMIT: usize = 3;
/// The whole probe, backlog wait included, is bounded by this; past it the
/// receipt goes out without an advisory and the write is unaffected.
pub const SIMILAR_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the probe waits for the embed worker to catch up, so a capture
/// written moments ago can be a neighbour of the next one.
pub const SIMILAR_BACKLOG_WAIT: Duration = Duration::from_secs(1);
/// The backlog poll interval inside that wait.
pub const SIMILAR_BACKLOG_POLL: Duration = Duration::from_millis(50);
/// The `edit_engram` operations that change the body; `set_frontmatter` is
/// not one and never probes.
pub const CONTENT_OPERATIONS: [&str; 6] = [
    "append",
    "prepend",
    "find_replace",
    "replace_section",
    "insert_before_section",
    "insert_after_section",
];
/// The fixed instruction every advisory carries.
pub const SIMILAR_GUIDANCE: &str = "These existing engrams are close to what was just written. Read the one that fits. \
     If it already owns the topic, merge into it and retire or delete this one. \
     If this one makes it false, supersede it: status superseded plus the superseded_by and supersedes pair. \
     If they are related but distinct, add a relation between them. If unrelated, do nothing.";

/// What a surface hands the engine to probe with.
pub enum SimilarProbe<'a> {
    /// A whole engram given as its parts: a `write_engram` call.
    Write {
        /// The engram title.
        title: &'a str,
        /// The `description` frontmatter, when the caller supplied one.
        description: Option<&'a str>,
        /// The markdown body.
        body: &'a str,
    },
    /// A whole engram given as markdown, frontmatter included: a save.
    Markdown {
        /// The full document, frontmatter and body.
        text: &'a str,
    },
    /// A content edit: the text the operation added or substituted. The
    /// engine looks the title up from the receipt's own address.
    Edit {
        /// The text the operation added or substituted.
        new_text: &'a str,
    },
}

impl<'a> SimilarProbe<'a> {
    /// The probe for a capture, description read off the metadata when the
    /// caller put one there.
    pub fn for_write(p: &'a WriteParams) -> SimilarProbe<'a> {
        SimilarProbe::Write {
            title: &p.title,
            description: metadata_description(p.metadata.as_ref()),
            body: &p.content,
        }
    }

    /// The probe for a content edit, or `None` for `set_frontmatter` and for
    /// an operation that carried no text.
    pub fn for_edit(p: &'a EditParams) -> Option<SimilarProbe<'a>> {
        if !CONTENT_OPERATIONS.contains(&p.operation.as_str()) {
            return None;
        }
        p.content
            .as_deref()
            .map(|new_text| SimilarProbe::Edit { new_text })
    }
}

/// One neighbour on a receipt. No score: ranking is the signal, the number is
/// not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SimilarEngram {
    /// The neighbour's domain.
    pub domain: String,
    /// The neighbour's permalink.
    pub permalink: String,
    /// The neighbour's title.
    pub title: String,
    /// The neighbour's lifecycle status.
    pub status: String,
    /// The neighbour's `type`.
    #[serde(rename = "type")]
    pub engram_type: String,
}

/// Title, description and the opening body, capped; `None` under the floor.
pub fn write_probe_text(title: &str, description: Option<&str>, body: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    let title = title.trim();
    if !title.is_empty() {
        parts.push(title);
    }
    if let Some(d) = description.map(str::trim).filter(|d| !d.is_empty()) {
        parts.push(d);
    }
    let body = body.trim();
    if !body.is_empty() {
        parts.push(body);
    }
    floor(cap_chars(&parts.join("\n"), SIMILAR_PROBE_MAX_CHARS))
}

/// A whole document: the frontmatter is parsed off and the probe is built
/// from its title, description and body. A document that does not parse as an
/// engram probes nothing.
pub fn markdown_probe_text(text: &str) -> Option<String> {
    let engram = crystalline_core::parse_engram(text).ok()?;
    write_probe_text(
        &engram.frontmatter.title,
        engram.frontmatter.description.as_deref(),
        &engram.body,
    )
}

/// Title plus the new text of an edit, capped; `None` under the floor.
pub fn edit_probe_text(title: &str, new_text: &str) -> Option<String> {
    write_probe_text(title, None, new_text)
}

/// The `description` a caller put in `metadata`, when it is a string.
pub fn metadata_description(metadata: Option<&Value>) -> Option<&str> {
    metadata
        .and_then(|m| m.get("description"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
}

/// Put the advisory on a receipt. Nothing is added for an empty list, so a
/// quiet receipt is byte-identical to today's.
pub fn attach(receipt: &mut Value, similar: &[SimilarEngram]) {
    if similar.is_empty() {
        return;
    }
    let Value::Object(map) = receipt else {
        return;
    };
    map.insert("similar".to_string(), json!(similar));
    map.insert(
        "guidance".to_string(),
        Value::String(SIMILAR_GUIDANCE.to_string()),
    );
}

/// The first `max` characters, on a character boundary.
fn cap_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn floor(text: String) -> Option<String> {
    (text.chars().count() >= SIMILAR_PROBE_MIN_CHARS).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(word: &str, n: usize) -> String {
        std::iter::repeat_n(word, n).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn a_write_probe_is_title_description_body_capped_at_900() {
        let body = long("retry", 400);
        let text = write_probe_text("Retry queue", Some("backoff notes"), &body).unwrap();
        assert!(text.starts_with("Retry queue\nbackoff notes\nretry retry"));
        assert_eq!(text.chars().count(), SIMILAR_PROBE_MAX_CHARS);
    }

    #[test]
    fn a_short_probe_is_skipped() {
        assert!(write_probe_text("Note", None, "one line").is_none());
        assert!(edit_probe_text("Note", "- [fact] short").is_none());
    }

    #[test]
    fn a_markdown_probe_strips_the_frontmatter() {
        let doc = format!(
            "---\ntype: engram\ntitle: Retry queue\npermalink: retry-queue\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\ndescription: backoff notes\n---\n\n{}\n",
            long("retry", 40)
        );
        let text = markdown_probe_text(&doc).unwrap();
        assert!(text.starts_with("Retry queue\nbackoff notes\nretry"));
        assert!(!text.contains("permalink:"));
        assert!(markdown_probe_text("no frontmatter at all").is_none());
    }

    #[test]
    fn for_edit_probes_content_operations_only() {
        let mut p = EditParams {
            identifier: "x".into(),
            domain: "d".into(),
            operation: "set_frontmatter".into(),
            content: Some(long("retry", 40)),
            key: Some("status".into()),
            value: Some("stable".into()),
            section: None,
            find_text: None,
            expected_checksum: None,
            expected_replacements: None,
            include_subsections: false,
        };
        assert!(SimilarProbe::for_edit(&p).is_none());
        p.operation = "append".into();
        assert!(SimilarProbe::for_edit(&p).is_some());
        p.content = None;
        assert!(SimilarProbe::for_edit(&p).is_none());
    }

    #[test]
    fn attach_adds_nothing_for_an_empty_list_and_both_keys_otherwise() {
        let mut receipt = json!({ "domain": "d", "permalink": "p" });
        attach(&mut receipt, &[]);
        assert_eq!(receipt, json!({ "domain": "d", "permalink": "p" }));
        attach(
            &mut receipt,
            &[SimilarEngram {
                domain: "d".into(),
                permalink: "q".into(),
                title: "Q".into(),
                status: "stable".into(),
                engram_type: "engram".into(),
            }],
        );
        assert_eq!(receipt["similar"][0]["type"], "engram");
        assert_eq!(receipt["guidance"], SIMILAR_GUIDANCE);
    }

    /// The advisory renders as a TOON table, which is the property spec 3.4
    /// asks a unit test to pin: receipts go through `ok` today, so a later move
    /// to `ok_list` costs nothing. Asserted through `toon::render`, the crate's
    /// own encoder, rather than by re-deriving its tabular criteria here - a
    /// hand-rolled copy would not notice `is_tabular` tightening.
    #[test]
    fn similar_rows_render_as_a_toon_table() {
        let mut receipt = json!({ "domain": "d", "permalink": "p" });
        attach(
            &mut receipt,
            &[
                SimilarEngram {
                    domain: "d".into(),
                    permalink: "q".into(),
                    title: "Q".into(),
                    status: "stable".into(),
                    engram_type: "engram".into(),
                },
                SimilarEngram {
                    domain: "d".into(),
                    permalink: "r".into(),
                    title: "R".into(),
                    status: "draft".into(),
                    engram_type: "guide".into(),
                },
            ],
        );
        let rendered = crate::toon::render(&receipt);
        assert!(
            rendered.contains("similar[2]{domain,permalink,status,title,type}:"),
            "{rendered}"
        );
        assert!(rendered.contains("d,q,stable,Q,engram"), "{rendered}");
        assert!(rendered.contains("d,r,draft,R,guide"), "{rendered}");
    }
}
