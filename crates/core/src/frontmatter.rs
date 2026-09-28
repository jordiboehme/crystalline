//! Frontmatter as top-level key blocks: the pure pieces the three-way merge
//! of an engram text, verify's `E010` and doctor's repair of a repeated key
//! share.
//!
//! A block is one top-level key: its key line plus every following line that
//! does not start a new key (indented lines, `-` items, blank and comment
//! lines). Blank and comment lines before the first key are the preamble.
//! Each block parses on its own to a one-entry mapping; its parsed key is its
//! identity and its parsed value is what merges compare, so quoting and
//! comments never count as a change of value.
//!
//! No async, no database and no diff engine: a caller that needs a line merge
//! hands it in as a [`LineMerge`].

use serde::Serialize;
use serde_yaml_ng::Value;

use crate::parse::{locate, parse_engram};

/// A caller's line-based three-way merge, `(base, local, upstream)` to the
/// merged text, or `None` when the two sides collide.
pub type LineMerge<'f> = &'f dyn Fn(&str, &str, &str) -> Option<String>;

/// A source cut at its frontmatter delimiters. The four parts concatenate
/// back to the source byte for byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontmatterParts<'a> {
    /// The opening `---` line with its line ending.
    pub open: &'a str,
    /// The YAML between the delimiters.
    pub yaml: &'a str,
    /// The closing `---` line with its line ending (none at the end of file).
    pub close: &'a str,
    /// Everything after the closing line.
    pub body: &'a str,
}

/// Cut `source` at its frontmatter delimiters, or `None` when it has no
/// frontmatter block (including a source that starts with a BOM).
pub fn split_frontmatter(source: &str) -> Option<FrontmatterParts<'_>> {
    let (has_frontmatter, span, body_start) = locate(source);
    if !has_frontmatter {
        return None;
    }
    Some(FrontmatterParts {
        open: &source[..span.start],
        yaml: &source[span.clone()],
        close: &source[span.end..body_start],
        body: &source[body_start..],
    })
}

/// One top-level frontmatter key with its exact text.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyBlock {
    /// The parsed key, rendered as text.
    pub key: String,
    /// The key line and its continuation lines, line endings included.
    pub text: String,
    /// The parsed value.
    pub value: Value,
    /// One-based file line of the key line (the opening `---` is line 1).
    pub line: usize,
}

/// A frontmatter cut into its preamble and key blocks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyBlocks {
    /// Blank and comment lines before the first key.
    pub preamble: String,
    /// The key blocks in file order, repeats included.
    pub blocks: Vec<KeyBlock>,
}

/// Cut frontmatter YAML (the text between the delimiters) into key blocks,
/// or `None` when it cannot be cut: a non-comment line before the first key,
/// a flow mapping or sequence at column 0, or a block that does not parse on
/// its own to exactly one entry (a complex key, an alias into another
/// block).
pub fn key_blocks(yaml: &str) -> Option<KeyBlocks> {
    let mut preamble = String::new();
    let mut raw: Vec<(usize, String)> = Vec::new();
    for (index, line) in yaml.split_inclusive('\n').enumerate() {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.starts_with(['{', '[']) {
            // Flow style at column 0 is not a key line: step aside.
            return None;
        }
        if starts_key(content) {
            raw.push((index + 2, line.to_string()));
        } else if let Some((_, text)) = raw.last_mut() {
            text.push_str(line);
        } else if is_blank_or_comment(content) {
            preamble.push_str(line);
        } else {
            return None;
        }
    }
    let mut blocks = Vec::with_capacity(raw.len());
    for (line, text) in raw {
        let Ok(Value::Mapping(mapping)) = serde_yaml_ng::from_str::<Value>(&text) else {
            return None;
        };
        if mapping.len() != 1 {
            return None;
        }
        let (key, value) = mapping.into_iter().next()?;
        blocks.push(KeyBlock {
            key: key_text(&key),
            text,
            value,
            line,
        });
    }
    Some(KeyBlocks { preamble, blocks })
}

fn starts_key(content: &str) -> bool {
    !content.is_empty()
        && !content.starts_with([' ', '\t', '#'])
        && content != "-"
        && !content.starts_with("- ")
        && !content.starts_with("-\t")
}

fn is_blank_or_comment(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn key_text(key: &Value) -> String {
    match key {
        Value::String(s) => s.clone(),
        other => serde_yaml_ng::to_string(other)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
    }
}

/// A top-level key a frontmatter holds more than once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuplicateKey {
    /// The key.
    pub key: String,
    /// One-based file lines of every copy, in file order.
    pub lines: Vec<usize>,
    /// Whether every copy has the same parsed value.
    pub same_value: bool,
}

/// Every top-level key `source`'s frontmatter holds more than once, in the
/// order of their first copy. Empty when there is no frontmatter or it
/// cannot be cut into key blocks.
pub fn duplicate_keys(source: &str) -> Vec<DuplicateKey> {
    match split_frontmatter(source).and_then(|parts| key_blocks(parts.yaml)) {
        Some(blocks) => repeats(&blocks.blocks),
        None => Vec::new(),
    }
}

fn repeats(blocks: &[KeyBlock]) -> Vec<DuplicateKey> {
    let mut found = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if blocks[..i].iter().any(|b| b.key == block.key) {
            continue;
        }
        let copies: Vec<&KeyBlock> = blocks[i..].iter().filter(|b| b.key == block.key).collect();
        if copies.len() > 1 {
            found.push(DuplicateKey {
                key: block.key.clone(),
                lines: copies.iter().map(|c| c.line).collect(),
                same_value: copies.iter().all(|c| c.value == block.value),
            });
        }
    }
    found
}

#[derive(Debug, Clone, Copy)]
enum Keep {
    First,
    Last,
}

fn without_repeats(blocks: &[KeyBlock], keep: Keep) -> Vec<&KeyBlock> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, block)| match keep {
            Keep::First => !blocks[..*i].iter().any(|b| b.key == block.key),
            Keep::Last => !blocks[i + 1..].iter().any(|b| b.key == block.key),
        })
        .map(|(_, block)| block)
        .collect()
}

fn rebuild(parts: &FrontmatterParts<'_>, preamble: &str, blocks: &[&KeyBlock]) -> String {
    let mut out = String::with_capacity(
        parts.open.len() + parts.yaml.len() + parts.close.len() + parts.body.len(),
    );
    out.push_str(parts.open);
    out.push_str(preamble);
    for block in blocks {
        out.push_str(&block.text);
    }
    out.push_str(parts.close);
    out.push_str(parts.body);
    out
}

/// `source` with every later copy of a repeated key removed, whatever the
/// copies' values, or `None` when nothing repeats. Verify uses it to tell a
/// file whose only YAML problem is a repeat from one that is broken anyway.
pub fn keep_first_copies(source: &str) -> Option<String> {
    let parts = split_frontmatter(source)?;
    let blocks = key_blocks(parts.yaml)?;
    if repeats(&blocks.blocks).is_empty() {
        return None;
    }
    Some(rebuild(
        &parts,
        &blocks.preamble,
        &without_repeats(&blocks.blocks, Keep::First),
    ))
}

/// What [`collapse_duplicate_keys`] made of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collapse {
    /// Nothing repeats (or there is no frontmatter to cut).
    Unchanged,
    /// Every repeat agreed; this is the source with one copy of each.
    Collapsed(String),
    /// At least one key repeats with different values; these keys. Nothing
    /// is collapsed, not even the keys that agree.
    ValuesDiffer(Vec<DuplicateKey>),
    /// The repeats agree, but the collapsed source still does not parse.
    NotRepairable,
}

/// Remove the extra copies of every repeated frontmatter key whose copies
/// all agree. The first copy stays, unless `reference` (a team domain's base
/// snapshot of the same file) has a frontmatter that keeping the last copy
/// matches byte for byte and keeping the first does not.
pub fn collapse_duplicate_keys(source: &str, reference: Option<&str>) -> Collapse {
    let Some(parts) = split_frontmatter(source) else {
        return Collapse::Unchanged;
    };
    let Some(blocks) = key_blocks(parts.yaml) else {
        return Collapse::Unchanged;
    };
    let found = repeats(&blocks.blocks);
    if found.is_empty() {
        return Collapse::Unchanged;
    }
    if found.iter().any(|d| !d.same_value) {
        return Collapse::ValuesDiffer(found.into_iter().filter(|d| !d.same_value).collect());
    }
    let first = rebuild(
        &parts,
        &blocks.preamble,
        &without_repeats(&blocks.blocks, Keep::First),
    );
    let last = rebuild(
        &parts,
        &blocks.preamble,
        &without_repeats(&blocks.blocks, Keep::Last),
    );
    let reference_yaml = reference.and_then(split_frontmatter).map(|p| p.yaml);
    let yaml_of = |text: &str| split_frontmatter(text).map(|p| p.yaml.to_string());
    let chosen = if reference_yaml.is_some()
        && yaml_of(&last).as_deref() == reference_yaml
        && yaml_of(&first).as_deref() != reference_yaml
    {
        last
    } else {
        first
    };
    if parse_engram(&chosen).is_ok() {
        Collapse::Collapsed(chosen)
    } else {
        Collapse::NotRepairable
    }
}

/// One-based line numbers as a reader says them: "5", "3 and 12",
/// "3, 7 and 12".
pub fn line_list(lines: &[usize]) -> String {
    match lines {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!(
            "{} and {last}",
            rest.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntype: engram\ntitle: T\npermalink: t\n---\n\n# T\n\nBody.\n";

    #[test]
    fn split_frontmatter_separates_delimiters_yaml_and_body() {
        let parts = split_frontmatter(DOC).expect("a frontmatter block");
        assert_eq!(parts.open, "---\n");
        assert_eq!(parts.yaml, "type: engram\ntitle: T\npermalink: t\n");
        assert_eq!(parts.close, "---\n");
        assert_eq!(parts.body, "\n# T\n\nBody.\n");
    }

    #[test]
    fn a_bom_or_an_unclosed_block_has_no_frontmatter_parts() {
        assert!(split_frontmatter(&format!("\u{feff}{DOC}")).is_none());
        assert!(split_frontmatter("---\ntitle: T\n").is_none());
        assert!(split_frontmatter("# no frontmatter\n").is_none());
    }

    #[test]
    fn key_blocks_keep_comments_quoted_keys_and_block_scalars() {
        let yaml = "# preamble\ntitle: T\ntags:\n  - a\n  - b\n# note\n\"status\": x\ndescription: |\n  one\n\n  two\n";
        let blocks = key_blocks(yaml).expect("splits");
        assert_eq!(blocks.preamble, "# preamble\n");
        let keys: Vec<&str> = blocks.blocks.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(keys, ["title", "tags", "status", "description"]);
        assert_eq!(blocks.blocks[1].text, "tags:\n  - a\n  - b\n# note\n");
        assert_eq!(blocks.blocks[3].text, "description: |\n  one\n\n  two\n");
        let lines: Vec<usize> = blocks.blocks.iter().map(|b| b.line).collect();
        assert_eq!(lines, [3, 4, 8, 9], "file lines, the opening --- is line 1");
        let joined: String = std::iter::once(blocks.preamble.as_str())
            .chain(blocks.blocks.iter().map(|b| b.text.as_str()))
            .collect();
        assert_eq!(joined, yaml, "the cut is lossless");
    }

    #[test]
    fn key_blocks_refuse_what_they_cannot_cut() {
        assert!(
            key_blocks("  indented: 1\ntitle: T\n").is_none(),
            "indented line before the first key"
        );
        assert!(
            key_blocks("- a\ntitle: T\n").is_none(),
            "sequence item before the first key"
        );
        assert!(
            key_blocks("{a: 1, b: 2}\n").is_none(),
            "a flow mapping is two keys on one line"
        );
        assert!(
            key_blocks("{a: 1}\n").is_none(),
            "a flow mapping steps aside even with one key"
        );
        assert!(
            key_blocks("title: T\n[a, b]\n").is_none(),
            "a flow sequence at column 0"
        );
        assert!(
            key_blocks("a: &x 1\nb: *x\n").is_none(),
            "an alias into another block"
        );
    }

    #[test]
    fn duplicate_keys_name_every_copy_and_whether_they_agree() {
        let source = "---\ntitle: T\ndomain_name: eng\nstatus: stable\ndomain_name: 'eng'\ntags: [a]\ntags: [b]\ntags: [a]\n---\n";
        let found = duplicate_keys(source);
        assert_eq!(
            found,
            vec![
                DuplicateKey {
                    key: "domain_name".into(),
                    lines: vec![3, 5],
                    same_value: true
                },
                DuplicateKey {
                    key: "tags".into(),
                    lines: vec![6, 7, 8],
                    same_value: false
                },
            ]
        );
        assert!(duplicate_keys(DOC).is_empty());
    }

    #[test]
    fn a_quoted_key_repeats_its_plain_spelling() {
        let found = duplicate_keys("---\nstatus: a\n\"status\": a\n---\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].same_value);
    }

    #[test]
    fn keep_first_copies_drops_the_later_ones_and_nothing_else() {
        let source =
            "---\ntitle: T\ndomain_name: eng\nstatus: stable\ndomain_name: eng\n---\n\nBody.\n";
        assert_eq!(
            keep_first_copies(source).as_deref(),
            Some("---\ntitle: T\ndomain_name: eng\nstatus: stable\n---\n\nBody.\n")
        );
        assert_eq!(keep_first_copies(DOC), None, "nothing repeats");
    }

    #[test]
    fn collapse_keeps_the_first_copy_without_a_reference() {
        let source =
            "---\ntitle: T\ndomain_name: eng\nstatus: stable\ndomain_name: eng\n---\n\nBody.\n";
        assert_eq!(
            collapse_duplicate_keys(source, None),
            Collapse::Collapsed(
                "---\ntitle: T\ndomain_name: eng\nstatus: stable\n---\n\nBody.\n".into()
            )
        );
        assert_eq!(collapse_duplicate_keys(DOC, None), Collapse::Unchanged);
    }

    #[test]
    fn collapse_keeps_the_copy_the_reference_has() {
        let source = "---\ntitle: T\ndomain_name: eng\nstatus: stable\ndomain_name: eng\n---\n\nLocal body.\n";
        let reference = "---\ntitle: T\nstatus: stable\ndomain_name: eng\n---\n\nTeam body.\n";
        assert_eq!(
            collapse_duplicate_keys(source, Some(reference)),
            Collapse::Collapsed(
                "---\ntitle: T\nstatus: stable\ndomain_name: eng\n---\n\nLocal body.\n".into()
            ),
            "keeping the last copy matches the reference frontmatter, the body is not compared"
        );
    }

    #[test]
    fn collapse_refuses_when_any_repeat_disagrees() {
        let source = "---\nstatus: a\nstatus: a\ntags: [a]\ntags: [b]\n---\n";
        assert_eq!(
            collapse_duplicate_keys(source, None),
            Collapse::ValuesDiffer(vec![DuplicateKey {
                key: "tags".into(),
                lines: vec![4, 5],
                same_value: false
            }])
        );
    }

    #[test]
    fn line_list_joins_without_an_oxford_comma() {
        assert_eq!(line_list(&[5]), "5");
        assert_eq!(line_list(&[3, 12]), "3 and 12");
        assert_eq!(line_list(&[3, 7, 12]), "3, 7 and 12");
    }
}
