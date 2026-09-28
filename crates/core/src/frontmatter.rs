//! Frontmatter as top-level key blocks: the pure pieces the three-way merge
//! of an engram text ([`merge_text`], used by the pull in crystalline-remote
//! and by the co-editing session's external change), verify's `E010` and
//! doctor's repair of a repeated key share.
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
    /// The parsed key, rendered as text for messages.
    pub key: String,
    /// The parsed key: a block's identity (`1:` and `"1":` are two keys).
    pub parsed_key: Value,
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
            parsed_key: key,
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
        if blocks[..i].iter().any(|b| b.parsed_key == block.parsed_key) {
            continue;
        }
        let copies: Vec<&KeyBlock> = blocks[i..]
            .iter()
            .filter(|b| b.parsed_key == block.parsed_key)
            .collect();
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
            Keep::First => !blocks[..*i]
                .iter()
                .any(|b| b.parsed_key == block.parsed_key),
            Keep::Last => !blocks[i + 1..]
                .iter()
                .any(|b| b.parsed_key == block.parsed_key),
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

/// What [`merge_frontmatter`] made of three sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontmatterMerge<'a> {
    /// The merged frontmatter, delimiters included (upstream's), and the
    /// three bodies the caller still has to merge.
    Clean {
        /// Opening delimiter, merged keys, closing delimiter.
        head: String,
        /// The base body: the whole base when it has no frontmatter, empty
        /// when there is no base.
        base_body: &'a str,
        /// The local body.
        local_body: &'a str,
        /// The upstream body.
        upstream_body: &'a str,
    },
    /// The sides collide: on a key both sides changed differently, on a key
    /// local or upstream holds twice with different values, or on a preamble
    /// both sides changed differently.
    Conflict,
    /// The frontmatter cannot be merged by key; merge the whole text.
    NotApplicable,
}

#[derive(Default)]
struct Prepared {
    preamble: String,
    blocks: Vec<KeyBlock>,
}

enum Refusal {
    StepAside,
    Conflict,
}

/// One side ready to merge: cut, repeats with one value collapsed to the
/// first copy, and checked that the blocks read together say what they say
/// one by one (the guard against a wrong cut).
fn prepare(yaml: &str) -> Result<Prepared, Refusal> {
    let KeyBlocks { preamble, blocks } = key_blocks(yaml).ok_or(Refusal::StepAside)?;
    if repeats(&blocks).iter().any(|d| !d.same_value) {
        return Err(Refusal::Conflict);
    }
    let kept: Vec<KeyBlock> = without_repeats(&blocks, Keep::First)
        .into_iter()
        .cloned()
        .collect();
    if !consistent(&preamble, &kept) {
        return Err(Refusal::StepAside);
    }
    Ok(Prepared {
        preamble,
        blocks: kept,
    })
}

fn consistent(preamble: &str, blocks: &[KeyBlock]) -> bool {
    if blocks.is_empty() {
        return true;
    }
    let mut joined = preamble.to_string();
    for block in blocks {
        joined.push_str(&block.text);
    }
    match serde_yaml_ng::from_str::<Value>(&joined) {
        Ok(Value::Mapping(mapping)) => mapping.into_iter().eq(blocks
            .iter()
            .map(|b| (b.parsed_key.clone(), b.value.clone()))),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Lf,
    CrLf,
}

/// The one line ending a frontmatter section uses, or `None` when it mixes.
fn ending(section: &str) -> Option<Ending> {
    let mut seen = None;
    for line in section.split_inclusive('\n').filter(|l| l.ends_with('\n')) {
        let this = if line.ends_with("\r\n") {
            Ending::CrLf
        } else {
            Ending::Lf
        };
        match seen {
            None => seen = Some(this),
            Some(before) if before != this => return None,
            Some(_) => {}
        }
    }
    Some(seen.unwrap_or(Ending::Lf))
}

fn head_of<'a>(source: &'a str, parts: &FrontmatterParts<'a>) -> &'a str {
    &source[..source.len() - parts.body.len()]
}

/// The three-way rule on plain text: local unchanged or equal to upstream
/// takes upstream, upstream unchanged takes local, anything else collides.
fn settle(base: &str, local: &str, upstream: &str) -> Option<String> {
    if local == upstream || local == base {
        Some(upstream.to_string())
    } else if upstream == base {
        Some(local.to_string())
    } else {
        None
    }
}

fn find<'s>(blocks: &'s [KeyBlock], key: &Value) -> Option<&'s KeyBlock> {
    blocks.iter().find(|b| b.parsed_key == *key)
}

fn value_of(block: Option<&KeyBlock>) -> Option<&Value> {
    block.map(|b| &b.value)
}

fn text_of(block: Option<&KeyBlock>) -> Option<&str> {
    block.map(|b| b.text.as_str())
}

/// Whether `side` holds the keys it shares with `base` in another relative
/// order than `base` does.
fn reordered(base: &[KeyBlock], side: &[KeyBlock]) -> bool {
    let at: Vec<usize> = side
        .iter()
        .filter_map(|block| base.iter().position(|b| b.parsed_key == block.parsed_key))
        .collect();
    at.windows(2).any(|w| w[0] > w[1])
}

/// Local's key order when only local reordered the keys it shares with the
/// base, upstream's otherwise (always upstream's when the base has no keys).
/// A key only the other side has goes right after its nearest preceding key
/// of that side (first when it has none).
fn key_order<'s>(
    base: &[KeyBlock],
    local: &'s [KeyBlock],
    upstream: &'s [KeyBlock],
) -> Vec<&'s KeyBlock> {
    if reordered(base, local) && !reordered(base, upstream) {
        interleave(local, upstream)
    } else {
        interleave(upstream, local)
    }
}

/// `leading`'s order, with each key only `other` has placed right after its
/// nearest preceding `other` key (first when it has none).
fn interleave<'s>(leading: &'s [KeyBlock], other: &'s [KeyBlock]) -> Vec<&'s KeyBlock> {
    let mut order: Vec<&KeyBlock> = leading.iter().collect();
    let mut after: Option<usize> = None;
    for block in other {
        match order.iter().position(|b| b.parsed_key == block.parsed_key) {
            Some(at) => after = Some(at),
            None => {
                let at = after.map_or(0, |a| a + 1);
                order.insert(at, block);
                after = Some(at);
            }
        }
    }
    order
}

/// The three block texts line-merged by `line_merge`, with the merged value,
/// when the result is exactly one block of the same key ending in a line
/// ending.
fn merge_block(
    base: &KeyBlock,
    local: &KeyBlock,
    upstream: &KeyBlock,
    line_merge: LineMerge<'_>,
) -> Option<(String, Value)> {
    let merged = line_merge(&base.text, &local.text, &upstream.text)?;
    let mut cut = key_blocks(&merged)?;
    if !cut.preamble.is_empty()
        || cut.blocks.len() != 1
        || cut.blocks[0].parsed_key != local.parsed_key
        || !merged.ends_with('\n')
    {
        return None;
    }
    let value = cut.blocks.pop()?.value;
    Some((merged, value))
}

/// One key's result by the table on [`merge_frontmatter`]: `Ok(Some(text))`
/// writes that block, `Ok(None)` drops the key, `Err(())` is a conflict.
fn pick(
    base: Option<&KeyBlock>,
    local: Option<&KeyBlock>,
    upstream: Option<&KeyBlock>,
    line_merge: LineMerge<'_>,
) -> Result<Option<String>, ()> {
    let owned = |text: Option<&str>| text.map(str::to_string);
    if value_of(local) == value_of(upstream) {
        // Same value: the spelling of the side that re-spelled it; when both
        // did, their line merge if it keeps the value, else local's. Both
        // sides added it: upstream's, so the pull converges on the team's copy.
        if text_of(local) == text_of(upstream) || text_of(local) == text_of(base) {
            return Ok(owned(text_of(upstream)));
        }
        if text_of(upstream) == text_of(base) {
            return Ok(owned(text_of(local)));
        }
        let Some(b) = base else {
            return Ok(owned(text_of(upstream)));
        };
        if let (Some(l), Some(u)) = (local, upstream)
            && let Some((merged, value)) = merge_block(b, l, u, line_merge)
            && value == l.value
        {
            return Ok(Some(merged));
        }
        return Ok(owned(text_of(local)));
    }
    if value_of(local) == value_of(base) {
        return Ok(owned(text_of(upstream)));
    }
    if value_of(upstream) == value_of(base) {
        return Ok(owned(text_of(local)));
    }
    if let (Some(b), Some(l), Some(u)) = (base, local, upstream)
        && let Some((merged, _)) = merge_block(b, l, u, line_merge)
    {
        return Ok(Some(merged));
    }
    Err(())
}

/// Merge three frontmatters key by key. `base` is `None` when both sides
/// added the file.
///
/// Steps aside ([`FrontmatterMerge::NotApplicable`]) when local or upstream
/// has no frontmatter, when any side cannot be cut into key blocks, when a
/// side's blocks read together do not say what they say one by one, or when
/// the sides do not use one line ending. A base without frontmatter counts
/// as an empty one with its whole text as the body; so does a missing base,
/// with an empty body. A key local or upstream holds twice with one value
/// keeps its first copy; with different values it is a conflict. When only
/// the base holds such a repeat, the merge steps aside.
///
/// For each key, with B, L and U its base, local and upstream block (each
/// possibly absent, "equal" comparing parsed values, absent equal to
/// absent):
///
/// | case | result |
/// |---|---|
/// | L equals U | U's text when L's text is byte-equal to U's or B's; L's text when U's text is byte-equal to B's (only local re-spelled it); when both re-spelled it, the three block texts line-merged by `line_merge`, taken when the result is exactly one block of the same key with L's value, else L's text; no B (both added it): U's text; absent on both: the key is gone |
/// | L equals B | U's text (only upstream changed or removed it) |
/// | U equals B | L's text (only local changed, added or removed it) |
/// | otherwise, all three present | the three block texts line-merged by `line_merge`, taken when the result is exactly one block of the same key |
/// | otherwise | conflict |
///
/// The preamble follows the first three rules, compared as text; changed
/// differently on both sides it is a conflict. Keys come in local's order
/// when local reordered the keys it shares with the base and upstream did
/// not reorder the keys it shares with the base; a key only upstream has
/// then goes right after its nearest preceding upstream key (first when it
/// has none). Otherwise, and always when the base has no keys (no base, or
/// one without frontmatter), keys come in upstream's order, a key only local
/// has right after its nearest preceding local key (first when it has none).
/// So a local reordering survives a pull that changes only the body or only
/// values, and an upstream reordering wins over a local one. The delimiter
/// lines are upstream's and every block is written as the chosen side's
/// exact text.
pub fn merge_frontmatter<'a>(
    base: Option<&'a str>,
    local: &'a str,
    upstream: &'a str,
    line_merge: LineMerge<'_>,
) -> FrontmatterMerge<'a> {
    let (Some(l), Some(u)) = (split_frontmatter(local), split_frontmatter(upstream)) else {
        return FrontmatterMerge::NotApplicable;
    };
    let b = base.and_then(split_frontmatter);
    let base_body = match (base, b) {
        (_, Some(parts)) => parts.body,
        (Some(text), None) => text,
        (None, None) => "",
    };
    let mut endings = vec![ending(head_of(local, &l)), ending(head_of(upstream, &u))];
    if let (Some(text), Some(parts)) = (base, b) {
        endings.push(ending(head_of(text, &parts)));
    }
    if endings.iter().any(Option::is_none) || endings.windows(2).any(|w| w[0] != w[1]) {
        return FrontmatterMerge::NotApplicable;
    }
    let sides = (
        b.map_or_else(|| Ok(Prepared::default()), |parts| prepare(parts.yaml)),
        prepare(l.yaml),
        prepare(u.yaml),
    );
    // Refusals in order: a side that cannot be cut steps aside before any
    // repeat is looked at; a differing repeat in local or upstream is a
    // conflict; one only the base holds steps aside, so the whole-file
    // merge (guarded by the parse check) decides.
    let (bs, ls, us) = match sides {
        (Ok(bs), Ok(ls), Ok(us)) => (bs, ls, us),
        (Err(Refusal::StepAside), _, _)
        | (_, Err(Refusal::StepAside), _)
        | (_, _, Err(Refusal::StepAside)) => return FrontmatterMerge::NotApplicable,
        (_, Err(Refusal::Conflict), _) | (_, _, Err(Refusal::Conflict)) => {
            return FrontmatterMerge::Conflict;
        }
        (Err(Refusal::Conflict), Ok(_), Ok(_)) => return FrontmatterMerge::NotApplicable,
    };
    let Some(preamble) = settle(&bs.preamble, &ls.preamble, &us.preamble) else {
        return FrontmatterMerge::Conflict;
    };
    let mut head = String::from(u.open);
    head.push_str(&preamble);
    for block in key_order(&bs.blocks, &ls.blocks, &us.blocks) {
        let key = &block.parsed_key;
        match pick(
            find(&bs.blocks, key),
            find(&ls.blocks, key),
            find(&us.blocks, key),
            line_merge,
        ) {
            Ok(Some(text)) => head.push_str(&text),
            Ok(None) => {}
            Err(()) => return FrontmatterMerge::Conflict,
        }
    }
    head.push_str(u.close);
    FrontmatterMerge::Clean {
        head,
        base_body,
        local_body: l.body,
        upstream_body: u.body,
    }
}

/// What [`merge_text`] made of three texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergedText {
    /// Write this.
    Clean(String),
    /// Leave the local text alone and record a conflict.
    Conflict,
}

/// Three-way merge of an engram text: the frontmatter key by key, the body
/// with `line_merge`, and the whole text with `line_merge` when the
/// frontmatter cannot be merged by key. A clean result that no longer parses
/// while local and upstream both did is a conflict: a merge never makes a
/// parseable file unparseable.
pub fn merge_text(
    base: Option<&str>,
    local: &str,
    upstream: &str,
    line_merge: LineMerge<'_>,
) -> MergedText {
    let merged = match merge_frontmatter(base, local, upstream, line_merge) {
        FrontmatterMerge::Clean {
            head,
            base_body,
            local_body,
            upstream_body,
        } => settle(base_body, local_body, upstream_body)
            .or_else(|| line_merge(base_body, local_body, upstream_body))
            .map(|body| head + &body),
        FrontmatterMerge::Conflict => None,
        FrontmatterMerge::NotApplicable => line_merge(base.unwrap_or(""), local, upstream),
    };
    match merged {
        Some(text) if !breaks_parse(local, upstream, &text) => MergedText::Clean(text),
        _ => MergedText::Conflict,
    }
}

fn breaks_parse(local: &str, upstream: &str, merged: &str) -> bool {
    parse_engram(local).is_ok() && parse_engram(upstream).is_ok() && parse_engram(merged).is_err()
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

    #[test]
    fn collapse_refuses_when_the_collapsed_source_still_does_not_parse() {
        let source =
            "---\ntitle: T\ndomain_name: eng\ndomain_name: eng\n---\n\nBody \0 with a null byte.\n";
        assert_eq!(
            collapse_duplicate_keys(source, None),
            Collapse::NotRepairable,
            "the repeats agree, but the null byte in the body still breaks the parse"
        );
    }

    #[test]
    fn a_tab_after_a_sequence_dash_is_not_a_new_key() {
        // Tested on the line classifier itself: the YAML parser rejects a tab
        // after `-`, so a whole cut refuses such a block either way.
        assert!(!starts_key("-\titem"));
        assert!(starts_key("title: T"));
    }

    /// A line merge that only settles the trivial cases, so every test below
    /// that expects a clean result proves the key rules did the work.
    fn trivial(base: &str, local: &str, upstream: &str) -> Option<String> {
        if local == upstream || local == base {
            Some(upstream.to_string())
        } else if upstream == base {
            Some(local.to_string())
        } else {
            None
        }
    }

    const SCOTTY: &str = "---\ntype: manifest\ntitle: Scotty\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\n---\n\n# Scotty\n\n## Scope\n\n- Engineering\n";

    fn with(text: &str, after: &str, line: &str) -> String {
        text.replacen(after, &format!("{after}{line}"), 1)
    }

    #[test]
    fn a_key_both_sides_added_with_one_value_lands_once_in_upstreams_place() {
        let local = with(SCOTTY, "status: stable\n", "domain_name: scotty\n");
        let upstream = with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn a_key_both_sides_added_with_different_values_is_a_conflict() {
        let local = with(SCOTTY, "status: stable\n", "domain_name: scotty\n");
        let upstream = with(SCOTTY, "title: Scotty\n", "domain_name: scotty-eng\n");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_key_deleted_on_one_side_and_changed_on_the_other_is_a_conflict() {
        let local = SCOTTY.replace("status: stable\n", "");
        let upstream = SCOTTY.replace("status: stable", "status: archived");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_key_deleted_on_one_side_and_unchanged_on_the_other_is_gone() {
        let local = SCOTTY.replace("status: stable\n", "");
        let upstream = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let expected = with(&local, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_key_only_local_added_follows_its_local_predecessor_and_upstream_spelling_stays() {
        let local = with(SCOTTY, "permalink: manifest\n", "owner: kim\n");
        let upstream = SCOTTY.replace("status: stable", "status: 'archived'");
        let expected = with(&upstream, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn an_upstream_respelling_of_an_equal_value_wins() {
        let local = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let upstream = SCOTTY.replace("status: stable", "status: 'stable'");
        let expected = with(&upstream, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_comment_only_local_added_survives_an_upstream_change() {
        let local = with(
            SCOTTY,
            "permalink: manifest\n",
            "# ask kim before renaming\n",
        );
        let upstream = SCOTTY.replace("status: stable", "status: archived");
        let expected = with(
            &upstream,
            "permalink: manifest\n",
            "# ask kim before renaming\n",
        );
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_repeat_already_in_local_is_repaired_when_upstream_moves() {
        let base = with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n");
        let local = with(&base, "status: stable\n", "domain_name: scotty\n");
        let upstream = with(&base, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(&base), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn a_repeat_with_different_values_in_one_side_is_a_conflict() {
        let local = with(SCOTTY, "status: stable\n", "status: archived\n");
        let upstream = with(SCOTTY, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_side_that_cannot_be_cut_steps_aside_before_a_repeat_conflicts() {
        let local = SCOTTY.replace("title: Scotty", "title: \"Scotty");
        let upstream = with(SCOTTY, "status: stable\n", "status: archived\n");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn a_list_changed_on_both_sides_merges_through_the_line_merge() {
        let local = SCOTTY.replace("  - manifest\n", "  - manifest\n  - c\n");
        let upstream = SCOTTY.replace("tags:\n  - manifest\n", "tags:\n  - z\n  - manifest\n");
        let block_merge = |b: &str, l: &str, u: &str| {
            if b == "tags:\n  - manifest\n" {
                Some("tags:\n  - z\n  - manifest\n  - c\n".to_string())
            } else {
                trivial(b, l, u)
            }
        };
        let expected = SCOTTY.replace(
            "tags:\n  - manifest\n",
            "tags:\n  - z\n  - manifest\n  - c\n",
        );
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &block_merge),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_block_line_merge_that_spills_into_another_key_is_a_conflict() {
        let local = SCOTTY.replace("  - manifest\n", "  - manifest\n  - c\n");
        let upstream = SCOTTY.replace("tags:\n  - manifest\n", "tags:\n  - z\n  - manifest\n");
        let spilling = |b: &str, l: &str, u: &str| {
            if b == "tags:\n  - manifest\n" {
                Some("tags:\n  - z\nowner: kim\n".to_string())
            } else {
                trivial(b, l, u)
            }
        };
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &spilling),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn mixed_line_endings_step_aside() {
        let local = with(SCOTTY, "status: stable\n", "domain_name: scotty\n");
        let upstream =
            with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n").replace('\n', "\r\n");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn crlf_on_every_side_merges_and_keeps_crlf() {
        let base = SCOTTY.replace('\n', "\r\n");
        let local = with(SCOTTY, "status: stable\n", "domain_name: scotty\n").replace('\n', "\r\n");
        let upstream =
            with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n").replace('\n', "\r\n");
        assert_eq!(
            merge_text(Some(&base), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn a_side_without_frontmatter_steps_aside() {
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), "# plain\n", SCOTTY, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn a_base_without_frontmatter_counts_as_an_empty_one() {
        let base = "\n# Scotty\n\n## Scope\n\n- Engineering\n";
        let local = with(SCOTTY, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(Some(base), &local, SCOTTY, &trivial),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn both_sides_added_the_file_keys_on_one_side_merge() {
        let local = with(SCOTTY, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(None, &local, SCOTTY, &trivial),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn a_clean_line_merge_that_breaks_the_parse_is_a_conflict() {
        let breaking = |_: &str, _: &str, _: &str| Some("---\na: 1\na: 2\n---\n".to_string());
        assert_eq!(
            merge_text(Some("base\n"), "local\n", "upstream\n", &breaking),
            MergedText::Conflict
        );
    }

    #[test]
    fn a_numeric_key_and_its_quoted_spelling_are_two_keys() {
        let base = "---\ntitle: T\n1: a\n\"1\": a\n---\n\nB\n";
        let local = "---\ntitle: T\n1: a\n\"1\": a\n---\n\nL\n";
        assert!(duplicate_keys(base).is_empty());
        assert_eq!(
            merge_text(Some(base), local, base, &trivial),
            MergedText::Clean(local.to_string())
        );
        let base = "---\ntitle: T\n1: a\n---\n\nB\n";
        let local = "---\ntitle: T\n1: a\n\"1\": a\n---\n\nB\n";
        let upstream = "---\ntitle: T\n1: a\n---\n\nU\n";
        assert_eq!(
            merge_text(Some(base), local, upstream, &trivial),
            MergedText::Clean("---\ntitle: T\n1: a\n\"1\": a\n---\n\nU\n".to_string()),
            "a quoted key only local added is not taken for the plain one"
        );
    }

    #[test]
    fn a_repeat_only_the_base_holds_steps_aside() {
        let base = with(SCOTTY, "status: stable\n", "status: archived\n");
        let local = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let upstream = SCOTTY.replace("status: stable", "status: archived");
        assert_eq!(
            merge_frontmatter(Some(&base), &local, &upstream, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn a_repeat_in_the_base_and_in_local_is_still_a_conflict() {
        let base = with(SCOTTY, "status: stable\n", "status: archived\n");
        let local = with(&base, "- Engineering\n", "- Warp\n");
        let upstream = SCOTTY.replace("status: stable", "status: archived");
        assert_eq!(
            merge_frontmatter(Some(&base), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    /// Both sides extend `tags`; `block` answers the block line merge.
    fn tags_merged_as(block: &'static str) -> impl Fn(&str, &str, &str) -> Option<String> {
        move |b: &str, l: &str, u: &str| {
            if b == "tags:\n  - manifest\n" {
                Some(block.to_string())
            } else {
                trivial(b, l, u)
            }
        }
    }

    fn tags_on_both_sides() -> (String, String) {
        (
            SCOTTY.replace("  - manifest\n", "  - manifest\n  - c\n"),
            SCOTTY.replace("tags:\n  - manifest\n", "tags:\n  - z\n  - manifest\n"),
        )
    }

    #[test]
    fn a_block_line_merge_without_a_final_line_ending_is_a_conflict() {
        let (local, upstream) = tags_on_both_sides();
        let merge = tags_merged_as("tags:\n  - z\n  - manifest\n  - c");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &merge),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_block_line_merge_that_adds_a_preamble_is_a_conflict() {
        let (local, upstream) = tags_on_both_sides();
        let merge = tags_merged_as("# c\ntags:\n  - z\n  - manifest\n  - c\n");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &merge),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_block_line_merge_that_renames_the_key_is_a_conflict() {
        let (local, upstream) = tags_on_both_sides();
        let merge = tags_merged_as("labels:\n  - z\n  - manifest\n  - c\n");
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &merge),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_preamble_changed_on_one_side_is_kept() {
        let local = SCOTTY.replacen("---\n", "---\n# p local\n", 1);
        let upstream = SCOTTY.replace("status: stable", "status: archived");
        let expected = upstream.replacen("---\n", "---\n# p local\n", 1);
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_preamble_changed_differently_on_both_sides_is_a_conflict() {
        let local = SCOTTY.replacen("---\n", "---\n# p local\n", 1);
        let upstream = SCOTTY.replacen("---\n", "---\n# p upstream\n", 1);
        assert_eq!(
            merge_frontmatter(Some(SCOTTY), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn two_keys_only_local_added_keep_their_order_after_their_predecessor() {
        let local = with(SCOTTY, "title: Scotty\n", "x: 1\ny: 2\n");
        let upstream = SCOTTY.replace("status: stable", "status: archived");
        let expected = with(&upstream, "title: Scotty\n", "x: 1\ny: 2\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_key_only_local_added_with_no_predecessor_goes_first() {
        let local = SCOTTY.replacen("---\n", "---\nfirst: 0\n", 1);
        let upstream = SCOTTY.replacen("---\n", "---\nzero: z\n", 1);
        let expected = SCOTTY.replacen("---\n", "---\nfirst: 0\nzero: z\n", 1);
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn blocks_that_read_differently_together_are_not_consistent() {
        // `key_blocks` never makes this cut; the guard is tested on its own,
        // as the classifier above is.
        let block = |key: &str, text: &str, value: Value| KeyBlock {
            key: key.to_string(),
            parsed_key: Value::String(key.to_string()),
            text: text.to_string(),
            value,
            line: 2,
        };
        let wrong = [
            block("a", "a:\n", Value::Null),
            block("b", "  b: 1\n", Value::Number(1.into())),
        ];
        assert!(!consistent("", &wrong));
        let right =
            key_blocks(split_frontmatter(SCOTTY).expect("frontmatter").yaml).expect("splits");
        assert!(consistent(&right.preamble, &right.blocks));
    }

    #[test]
    fn a_repeat_with_one_value_keeps_its_first_copy() {
        let local = with(
            &with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n"),
            "status: stable\n",
            "domain_name: 'scotty'\n",
        );
        let upstream = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let expected = with(&upstream, "title: Scotty\n", "domain_name: scotty\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_base_with_another_line_ending_steps_aside() {
        let base = SCOTTY.replace('\n', "\r\n");
        let local = with(SCOTTY, "status: stable\n", "domain_name: scotty\n");
        let upstream = with(SCOTTY, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_frontmatter(Some(&base), &local, &upstream, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn bodies_that_settle_never_reach_the_line_merge() {
        let local = with(SCOTTY, "permalink: manifest\n", "owner: kim\n");
        let upstream = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let expected = with(&upstream, "permalink: manifest\n", "owner: kim\n");
        let refusing = |_: &str, _: &str, _: &str| None;
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &refusing),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn the_safety_net_stays_quiet_when_upstream_already_did_not_parse() {
        let broken_upstream = "---\ntitle: \"T\n---\n\nupstream\n";
        let merged = "---\ntitle: \"T\n---\n\nmerged\n";
        let fixed = |_: &str, _: &str, _: &str| Some(merged.to_string());
        assert_eq!(
            merge_text(Some("base\n"), "local\n", broken_upstream, &fixed),
            MergedText::Clean(merged.to_string())
        );
    }

    #[test]
    fn the_safety_net_stays_quiet_when_an_input_already_did_not_parse() {
        let broken_local = "---\ntitle: \"T\n---\n\nlocal\n";
        let merged = "---\ntitle: \"T\n---\n\nmerged\n";
        let fixed = |_: &str, _: &str, _: &str| Some(merged.to_string());
        assert_eq!(
            merge_text(Some("base\n"), broken_local, "upstream\n", &fixed),
            MergedText::Clean(merged.to_string())
        );
    }

    /// SCOTTY with `title` and `permalink` swapped.
    fn scotty_title_after_permalink(text: &str) -> String {
        text.replace(
            "title: Scotty\npermalink: manifest\n",
            "permalink: manifest\ntitle: Scotty\n",
        )
    }

    #[test]
    fn a_local_reorder_survives_a_body_only_upstream_change() {
        let local = scotty_title_after_permalink(SCOTTY);
        let upstream = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let expected = with(&local, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_key_only_upstream_added_lands_after_its_upstream_predecessor_in_a_local_order() {
        let local = scotty_title_after_permalink(SCOTTY);
        let upstream = with(SCOTTY, "permalink: manifest\n", "owner: kim\n");
        let expected = with(&local, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn an_upstream_reorder_wins_when_local_kept_the_base_order() {
        let local = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let upstream = scotty_title_after_permalink(SCOTTY);
        let expected = with(&upstream, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn when_both_sides_reordered_upstreams_order_wins() {
        let local = scotty_title_after_permalink(SCOTTY);
        let upstream = SCOTTY.replace(
            "tags:\n  - manifest\nstatus: stable\n",
            "status: stable\ntags:\n  - manifest\n",
        );
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn without_a_base_upstreams_order_wins() {
        let local = scotty_title_after_permalink(SCOTTY);
        assert_eq!(
            merge_text(None, &local, SCOTTY, &trivial),
            MergedText::Clean(SCOTTY.to_string())
        );
    }

    #[test]
    fn a_local_comment_survives_an_upstream_requoting_of_the_same_value() {
        let local = SCOTTY.replace("status: stable", "status: stable # keep");
        let upstream = SCOTTY.replace("status: stable", "status: \"stable\"");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn both_respellings_of_an_equal_list_are_line_merged() {
        let local = SCOTTY.replace("tags:\n", "tags:\n  # keep\n");
        let upstream = SCOTTY.replace("  - manifest\n", "  - \"manifest\"\n");
        let merge = tags_merged_as("tags:\n  # keep\n  - \"manifest\"\n");
        let expected = SCOTTY.replace(
            "tags:\n  - manifest\n",
            "tags:\n  # keep\n  - \"manifest\"\n",
        );
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &merge),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_line_merge_of_two_respellings_that_changes_the_value_keeps_local() {
        let local = SCOTTY.replace("tags:\n", "tags:\n  # keep\n");
        let upstream = SCOTTY.replace("  - manifest\n", "  - \"manifest\"\n");
        let merge = tags_merged_as("tags:\n  - other\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &merge),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn a_key_both_added_with_one_value_in_two_spellings_takes_upstreams_spelling() {
        let local = with(SCOTTY, "title: Scotty\n", "domain_name: 'scotty'\n");
        let upstream = with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n");
        assert_eq!(
            merge_text(Some(SCOTTY), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }
}
