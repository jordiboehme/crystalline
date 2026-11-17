//! A capture whose `content` opens with a frontmatter block of its own.
//!
//! An agent that writes a whole markdown file sends its frontmatter along in
//! `content`. Written as body text, that block became a second frontmatter
//! block under the one this write generates, and its keys were lost: every
//! reader takes only the first block. So the block is lifted into the write's
//! own parameters before anything else reads them, by the same rule the
//! reader uses to find a block ([`crystalline_core::frontmatter::split_frontmatter`]).
//!
//! The precedence is the one `metadata` already has: an explicit parameter
//! wins over the same key in the block, and the keys the engine owns
//! (`permalink`, `recorded_at`, `timestamp`, `generated`) are ignored. The
//! receipt names what was taken, what lost to a parameter and what was
//! ignored, so no key the agent wrote goes without a word.

use serde_json::{Map, Value};
use serde_yaml_ng::Value as Yaml;

use super::{EngineError, Result};
use crate::params::WriteParams;

/// Keys the write owns outright: a block cannot set them.
const ENGINE_OWNED: [&str; 4] = ["permalink", "recorded_at", "timestamp", "generated"];

/// What lifting a leading block did to the parameters, for the receipt.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Lifted {
    /// Keys taken from the block.
    taken: Vec<String>,
    /// Keys the block held that a parameter already set.
    overridden: Vec<String>,
    /// Engine-owned keys the block held.
    ignored: Vec<String>,
}

impl Lifted {
    /// The receipt notice, or `None` when the block held no keys at all.
    pub(super) fn notice(&self) -> Option<String> {
        if self.taken.is_empty() && self.overridden.is_empty() && self.ignored.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if !self.taken.is_empty() {
            parts.push(format!("taken: {}", self.taken.join(", ")));
        }
        if !self.overridden.is_empty() {
            parts.push(format!(
                "kept your parameter instead: {}",
                self.overridden.join(", ")
            ));
        }
        if !self.ignored.is_empty() {
            parts.push(format!(
                "ignored, the server sets these: {}",
                self.ignored.join(", ")
            ));
        }
        Some(format!(
            "content opened with a frontmatter block, so its keys went into the engram's \
             frontmatter instead of the body ({}). Pass them as type, tags, status and \
             metadata next time.",
            parts.join("; ")
        ))
    }
}

impl WriteParams {
    /// These parameters as the write reads them: a leading frontmatter block
    /// in `content` lifted into the parameters, or these parameters unchanged
    /// when there is none or it cannot be lifted (the write refuses that one
    /// itself). For what a surface does after the write, such as the
    /// similar-engrams probe, so it sees the body without the block.
    pub fn with_content_frontmatter_merged(&self) -> WriteParams {
        lift_content_frontmatter(self)
            .map(|(merged, _)| merged)
            .unwrap_or_else(|_| self.clone())
    }
}

/// `p` with a leading frontmatter block in its `content` lifted into its
/// parameters, and what that did. A content without a block, or with one
/// that holds no mapping or is not YAML at all (a thematic break, some text,
/// a thematic break), comes back unchanged: that is markdown, and no reader
/// takes it for frontmatter. A mapping whose title, type, status or tags is
/// not plain text is refused by key.
pub(super) fn lift_content_frontmatter(p: &WriteParams) -> Result<(WriteParams, Lifted)> {
    let mut out = p.clone();
    let mut lifted = Lifted::default();
    let Some(content) = p.content.as_deref() else {
        return Ok((out, lifted));
    };
    // The emitter trims leading newlines off the body, so a block after a
    // blank line would still land at the top of the body.
    let content = content.trim_start_matches('\n');
    let Some(parts) = crystalline_core::frontmatter::split_frontmatter(content) else {
        return Ok((out, lifted));
    };
    let mapping = match serde_yaml_ng::from_str::<Yaml>(parts.yaml) {
        Ok(Yaml::Mapping(mapping)) => mapping,
        Ok(Yaml::Null) => {
            out.content = Some(parts.body.to_string());
            return Ok((out, lifted));
        }
        // Not a mapping, or not YAML at all: a thematic break, some text and
        // a thematic break is ordinary markdown, and no reader ever took it
        // for frontmatter, so it stays body text as it always did.
        Ok(_) | Err(_) => return Ok((out, lifted)),
    };

    // The caller's own metadata, decoded the way `build_markdown` decodes it.
    // Anything but an object is left alone for `build_markdown` to refuse.
    let mut metadata: Option<Map<String, Value>> = match &p.metadata {
        None | Some(Value::Null) => Some(Map::new()),
        Some(Value::Object(map)) => Some(map.clone()),
        Some(Value::String(raw)) => match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        },
        Some(_) => None,
    };

    for (key, value) in mapping {
        let key = key_text(&key);
        let set_by_param = match key.as_str() {
            k if ENGINE_OWNED.contains(&k) => {
                lifted.ignored.push(key);
                continue;
            }
            "title" => !p.title.trim().is_empty(),
            "type" => p.engram_type.is_some(),
            "status" => p.status.is_some(),
            "tags" => !p.tags.is_empty(),
            _ => match &metadata {
                Some(map) => map.contains_key(&key),
                // Metadata the write will refuse: the block's keys wait for
                // the agent's corrected call rather than going in around it.
                None => true,
            },
        };
        if set_by_param {
            lifted.overridden.push(key);
            continue;
        }
        match key.as_str() {
            "title" => out.title = scalar(&key, &value)?,
            "type" => out.engram_type = Some(scalar(&key, &value)?),
            "status" => out.status = Some(scalar(&key, &value)?),
            "tags" => out.tags = tag_list(&value)?,
            _ => {
                if let Some(map) = metadata.as_mut() {
                    map.insert(key.clone(), to_json(&value));
                }
            }
        }
        lifted.taken.push(key);
    }
    if let Some(map) = metadata
        && !map.is_empty()
    {
        out.metadata = Some(Value::Object(map));
    }
    out.content = Some(parts.body.to_string());
    Ok((out, lifted))
}

/// Whether `content` opens with what reads as a frontmatter block: a block
/// whose YAML is a mapping. A block that is not YAML is text. A prepend puts
/// its content right under the engram's frontmatter, where such a block is a
/// second one that every reader takes for body text, so a prepend refuses it.
pub(super) fn opens_with_frontmatter(content: &str) -> bool {
    crystalline_core::frontmatter::split_frontmatter(content.trim_start_matches('\n')).is_some_and(
        |parts| {
            matches!(
                serde_yaml_ng::from_str::<Yaml>(parts.yaml),
                Ok(Yaml::Mapping(_))
            )
        },
    )
}

/// The refusal a prepend that opens with a frontmatter block gets.
pub(super) const PREPEND_FRONTMATTER_REFUSAL: &str = "content opens with a frontmatter block, \
    and a prepend would leave it as a second block under the engram's own. Set each key with \
    set_frontmatter, or rewrite the whole engram with write_engram and overwrite: true, where \
    a leading block is merged";

/// A key as text: `1:` reads `1`.
fn key_text(key: &Yaml) -> String {
    match key {
        Yaml::String(s) => s.clone(),
        other => serde_yaml_ng::to_string(other)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
    }
}

/// A one-value key (title, type, status) as text.
fn scalar(key: &str, value: &Yaml) -> Result<String> {
    match value {
        Yaml::String(s) => Ok(s.clone()),
        Yaml::Bool(b) => Ok(b.to_string()),
        Yaml::Number(n) => Ok(n.to_string()),
        _ => Err(EngineError::Invalid(format!(
            "the frontmatter block in content gives {key} a value that is not plain text; \
             pass {key} as its own parameter instead"
        ))),
    }
}

/// `tags` as a list, from a YAML list or a comma-separated line.
fn tag_list(value: &Yaml) -> Result<Vec<String>> {
    match value {
        Yaml::Null => Ok(Vec::new()),
        Yaml::String(s) => Ok(s
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()),
        Yaml::Sequence(items) => items.iter().map(|item| scalar("tags", item)).collect(),
        _ => Err(EngineError::Invalid(
            "the frontmatter block in content gives tags a value that is not a list; pass \
             tags as its own parameter instead"
                .into(),
        )),
    }
}

/// A YAML value as the JSON `metadata` carries, so a block key goes through
/// the same checks a metadata key does.
fn to_json(value: &Yaml) -> Value {
    match value {
        Yaml::Null => Value::Null,
        Yaml::Bool(b) => Value::Bool(*b),
        Yaml::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                n.as_f64()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Sequence(items) => Value::Array(items.iter().map(to_json).collect()),
        Yaml::Mapping(map) => {
            Value::Object(map.iter().map(|(k, v)| (key_text(k), to_json(v))).collect())
        }
        Yaml::Tagged(tagged) => to_json(&tagged.value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(content: &str) -> WriteParams {
        WriteParams {
            path: None,
            base64: None,
            domain: "eng".into(),
            title: "T".into(),
            content: Some(content.into()),
            folder: None,
            engram_type: None,
            tags: Vec::new(),
            status: None,
            metadata: None,
            overwrite: false,
            expected_checksum: None,
            model: None,
            share_link: None,
        }
    }

    #[test]
    fn content_without_a_block_is_left_alone() {
        let p = params("# Body\n\n---\n\nmore\n");
        let (out, lifted) = lift_content_frontmatter(&p).unwrap();
        assert_eq!(out.content, p.content);
        assert_eq!(lifted, Lifted::default());
        assert!(lifted.notice().is_none());
    }

    #[test]
    fn a_block_after_blank_lines_is_still_lifted() {
        let (out, lifted) =
            lift_content_frontmatter(&params("\n\n---\nstatus: draft\n---\nBody\n")).unwrap();
        assert_eq!(out.status.as_deref(), Some("draft"));
        assert_eq!(out.content.as_deref(), Some("Body\n"));
        assert_eq!(lifted.taken, vec!["status".to_string()]);
    }

    #[test]
    fn tags_read_from_a_comma_separated_line() {
        let (out, _) = lift_content_frontmatter(&params("---\ntags: a, b\n---\nBody\n")).unwrap();
        assert_eq!(out.tags, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_title_fills_an_empty_title_parameter() {
        let mut p = params("---\ntitle: From Block\n---\nBody\n");
        p.title = String::new();
        let (out, _) = lift_content_frontmatter(&p).unwrap();
        assert_eq!(out.title, "From Block");
    }

    #[test]
    fn a_double_encoded_metadata_still_wins() {
        let mut p = params("---\nowner: block\nteam: core\n---\nBody\n");
        p.metadata = Some(Value::String("{\"owner\":\"param\"}".into()));
        let (out, lifted) = lift_content_frontmatter(&p).unwrap();
        let meta = out.metadata.unwrap();
        assert_eq!(meta["owner"], "param");
        assert_eq!(meta["team"], "core");
        assert_eq!(lifted.overridden, vec!["owner".to_string()]);
    }

    #[test]
    fn only_a_mapping_reads_as_a_block_for_a_prepend() {
        assert!(opens_with_frontmatter("---\nstatus: draft\n---\nIntro"));
        assert!(opens_with_frontmatter("\n---\nstatus: draft\n---\nIntro"));
        assert!(!opens_with_frontmatter("---\ntags: [x\n---\nIntro"));
        assert!(!opens_with_frontmatter("---\nJust a line\n---\nIntro"));
        assert!(!opens_with_frontmatter(
            "Intro\n\n---\nstatus: draft\n---\n"
        ));
    }

    #[test]
    fn a_block_that_is_not_yaml_is_left_as_body() {
        let p = params("---\nNot: [yaml\n---\nBody\n");
        let (out, lifted) = lift_content_frontmatter(&p).unwrap();
        assert_eq!(out.content, p.content);
        assert_eq!(lifted, Lifted::default());
    }

    #[test]
    fn the_merged_params_carry_the_body_without_the_block() {
        let merged = params("---\nstatus: draft\n---\nBody\n").with_content_frontmatter_merged();
        assert_eq!(merged.body(), "Body\n");
        assert_eq!(merged.status.as_deref(), Some("draft"));
    }

    #[test]
    fn a_list_valued_type_is_refused_by_name() {
        let err = lift_content_frontmatter(&params("---\ntype: [a, b]\n---\nBody\n")).unwrap_err();
        assert!(err.to_string().contains("type"), "{err}");
    }
}
