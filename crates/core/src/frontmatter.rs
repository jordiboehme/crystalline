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

use crate::line_endings::to_lf;
use crate::parse::{locate, parse_engram};

/// A caller's line-based three-way merge, `(base, local, upstream)` to the
/// merged text, or `None` when the two sides collide.
pub type LineMerge<'f> = &'f dyn Fn(&str, &str, &str) -> Option<String>;

/// The frontmatter keys an engram's schema knows: the ones the parser reads
/// into typed fields, plus the lifecycle keys the tools write as plain
/// frontmatter (`salience`, `sources`, `source_version`, `evolve_ack`). A
/// key outside this list is still kept as a custom key; the list is what
/// tells a block of engram frontmatter apart from markdown that only looks
/// like YAML, such as `Note: remember this` between two rules.
pub const RECOGNIZED_FRONTMATTER_KEYS: &[&str] = &[
    "type",
    "title",
    "permalink",
    "tags",
    "status",
    "description",
    "resource",
    "temporal_confidence",
    "recorded_at",
    "valid_from",
    "valid_to",
    "source_date",
    "verified",
    "last_verified",
    "stale_after",
    "review_after",
    "timestamp",
    "generated",
    "salience",
    "sources",
    "source_version",
    "evolve_ack",
];

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

/// Whether two sources say the same thing: their frontmatter parses to the
/// same YAML value and their bodies are byte for byte equal. Quoting, indent,
/// comments and key spelling style never count as a difference, the order of
/// a list's items does. A source whose frontmatter does not parse is never the
/// same as anything, so a caller that skips a write on `true` never skips a
/// repair. Two sources without frontmatter are compared as text.
pub fn same_content(a: &str, b: &str) -> bool {
    match (split_frontmatter(a), split_frontmatter(b)) {
        (None, None) => a == b,
        (Some(x), Some(y)) => {
            if x.body != y.body {
                return false;
            }
            let parse = |yaml: &str| serde_yaml_ng::from_str::<Value>(yaml).ok();
            match (parse(x.yaml), parse(y.yaml)) {
                (Some(left), Some(right)) => left == right,
                _ => false,
            }
        }
        _ => false,
    }
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
        // Same value: the spelling of the side that re-spelled it. Both
        // sides added it: upstream's, so the pull converges on the team's
        // copy. Both re-spelled it: their line merge if it keeps the value,
        // else local's.
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
        if text_of(local) == text_of(base) {
            return Ok(owned(text_of(upstream)));
        }
        // Local only re-spelled it (a comment, say) and upstream changed
        // the value: their line merge when it carries upstream's value,
        // else a conflict, so the local text is never dropped silently.
        if let (Some(b), Some(l), Some(u)) = (base, local, upstream)
            && let Some((merged, value)) = merge_block(b, l, u, line_merge)
            && value == u.value
        {
            return Ok(Some(merged));
        }
        return Err(());
    }
    if value_of(upstream) == value_of(base) {
        if text_of(upstream) == text_of(base) {
            return Ok(owned(text_of(local)));
        }
        // The mirror: upstream only re-spelled it and local changed the
        // value. Their line merge when it carries local's value, else a
        // conflict, so the team's text is never dropped silently.
        if let (Some(b), Some(l), Some(u)) = (base, local, upstream)
            && let Some((merged, value)) = merge_block(b, l, u, line_merge)
            && value == l.value
        {
            return Ok(Some(merged));
        }
        return Err(());
    }
    // `generated` is the provenance of the last write, not a person's words:
    // both sides touching it is the normal case for any two writes, so the
    // line with the later `at` wins whole (a readable time beats an unreadable
    // one, then the greater text, so both merge orders agree) instead of colliding as one line.
    if let (Some(l), Some(u)) = (local, upstream)
        && is_generated(l)
    {
        let rank = |b: &KeyBlock| (generated_at(&b.value), b.text.clone());
        let newer = if rank(l) > rank(u) { l } else { u };
        return Ok(Some(newer.text.clone()));
    }
    if let (Some(b), Some(l), Some(u)) = (base, local, upstream)
        && let Some((merged, _)) = merge_block(b, l, u, line_merge)
    {
        return Ok(Some(merged));
    }
    Err(())
}

fn is_generated(block: &KeyBlock) -> bool {
    matches!(&block.parsed_key, Value::String(k) if k == "generated")
}

/// The instant a `generated` mapping's `at` names, when it reads as RFC 3339.
fn generated_at(value: &Value) -> Option<crate::time::OffsetStamp> {
    let Value::Mapping(map) = value else {
        return None;
    };
    let Value::String(at) = map.get("at")? else {
        return None;
    };
    crate::time::parse_rfc3339(at)
}

/// Merge three frontmatters key by key. `base` is `None` when both sides
/// added the file.
///
/// Steps aside ([`FrontmatterMerge::NotApplicable`]) when local or upstream
/// has no frontmatter, when any side cannot be cut into key blocks, or when a
/// side's blocks read together do not say what they say one by one. The
/// texts are compared as given, so they are expected as LF; [`merge_text`]
/// converts them before it gets here. A base without frontmatter counts
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
/// | L equals U | U's text when L's text is byte-equal to U's or B's (absent on both: the key is gone); L's text when U's text is byte-equal to B's (only local re-spelled it); no B (both added it): U's text; when both re-spelled it, the three block texts line-merged by `line_merge`, taken when the result is exactly one block of the same key with L's value, else L's text |
/// | L equals B | U's text when L's text is byte-equal to B's (only upstream changed or removed it); when local re-spelled it (a comment, say) and U is present, the three block texts line-merged by `line_merge`, taken when the result is exactly one block of the same key with U's value; else conflict, so a local comment is never dropped silently |
/// | U equals B | L's text when U's text is byte-equal to B's (only local changed, added or removed it); when upstream re-spelled it and L is present, the three block texts line-merged by `line_merge`, taken when the result is exactly one block of the same key with L's value; else conflict, so an upstream comment is never dropped silently |
/// | otherwise, key `generated` with L and U both present | the block whose `at` reads as the later instant; a readable `at` beats an unreadable one, then the greater block text, so both merge orders give one line |
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
///
/// Every side is merged as LF, whatever line endings it came with, and the
/// result is LF: a CRLF base against LF sides is the same document and never
/// makes every line look changed.
pub fn merge_text(
    base: Option<&str>,
    local: &str,
    upstream: &str,
    line_merge: LineMerge<'_>,
) -> MergedText {
    merge_text_with(base, local, upstream, line_merge, line_merge)
}

/// [`merge_text`] with a merge of its own for the body: `line_merge` merges
/// the frontmatter key blocks (and the whole text when one side's
/// frontmatter cannot be merged by key), `body_merge` the body after the
/// frontmatter, and the whole text when no side has a frontmatter at all.
///
/// A body merge may be finer than a line merge (per character, say); a
/// frontmatter value is never merged finer than its lines, so two changes to
/// one date never make a date nobody wrote. When the frontmatter merge steps
/// aside and any side has a frontmatter, the body gets the plain line merge
/// too, since the whole text is merged as one.
pub fn merge_text_with(
    base: Option<&str>,
    local: &str,
    upstream: &str,
    line_merge: LineMerge<'_>,
    body_merge: LineMerge<'_>,
) -> MergedText {
    let base = base.map(crate::to_lf);
    let local = crate::to_lf(local);
    let upstream = crate::to_lf(upstream);
    let (base, local, upstream) = (base.as_deref(), local.as_ref(), upstream.as_ref());
    let merged = match merge_frontmatter(base, local, upstream, line_merge) {
        FrontmatterMerge::Clean {
            head,
            base_body,
            local_body,
            upstream_body,
        } => settle(base_body, local_body, upstream_body)
            .or_else(|| body_merge(base_body, local_body, upstream_body))
            .map(|body| head + &body),
        FrontmatterMerge::Conflict => None,
        FrontmatterMerge::NotApplicable => {
            let all_body = [base.unwrap_or(""), local, upstream]
                .iter()
                .all(|text| split_frontmatter(text).is_none());
            let merge = if all_body { body_merge } else { line_merge };
            merge(base.unwrap_or(""), local, upstream)
        }
    };
    match merged {
        Some(text) if !breaks_parse(local, upstream, &text) => MergedText::Clean(text),
        _ => MergedText::Conflict,
    }
}

fn breaks_parse(local: &str, upstream: &str, merged: &str) -> bool {
    parse_engram(local).is_ok() && parse_engram(upstream).is_ok() && parse_engram(merged).is_err()
}

/// Whether two sources are one text but for the top-level `generated` key,
/// which is the provenance of the last write and no one's words, compared as
/// LF. One side holding the key and the other not counts as the same, wherever
/// it stands. Everything else counts exactly, as text: another key's quoting,
/// the order of two other keys, a comment of hers at column 0 behind the stamp
/// and the body. A source with no frontmatter, or one that cannot be cut into
/// keys, is compared whole.
pub fn same_but_generated(a: &str, b: &str) -> bool {
    let (a, b) = (to_lf(a), to_lf(b));
    if a == b {
        return true;
    }
    let (Some(x), Some(y)) = (split_frontmatter(&a), split_frontmatter(&b)) else {
        return false;
    };
    if x.close != y.close || x.body != y.body {
        return false;
    }
    let (Some(left), Some(right)) = (key_blocks(x.yaml), key_blocks(y.yaml)) else {
        return false;
    };
    left.preamble == right.preamble && without_generated(&left) == without_generated(&right)
}

/// The text of every key block but `generated`. Of that block only what is its
/// own goes: the key line and its indented lines, not a comment or a blank line
/// at column 0 that a cut into blocks merely hangs on it.
fn without_generated(blocks: &KeyBlocks) -> String {
    let mut text = String::new();
    for block in &blocks.blocks {
        if is_generated(block) {
            block
                .text
                .split_inclusive('\n')
                .skip(1)
                .filter(|line| !line.starts_with([' ', '\t']))
                .for_each(|line| text.push_str(line));
        } else {
            text.push_str(&block.text);
        }
    }
    text
}

/// The keys a write owns outright: a frontmatter block in a write's content
/// cannot set them, and the repair of a second block leaves them out too.
pub const ENGINE_OWNED_KEYS: [&str; 4] = ["permalink", "recorded_at", "timestamp", "generated"];

/// The block between two rules as frontmatter, when it looks like engram
/// frontmatter: a YAML mapping, with no `#` comment line (between two rules
/// that is a markdown heading, which YAML would read as a comment and drop),
/// that names at least one key the engram schema knows
/// ([`RECOGNIZED_FRONTMATTER_KEYS`]). Anything else, `Note: remember this`
/// among them, is markdown that stays body text. The one rule a write's
/// content, an edit's result and verify's `E012` find a block by.
pub fn frontmatter_block(yaml: &str) -> Option<serde_yaml_ng::Mapping> {
    if yaml.lines().any(|line| line.trim_start().starts_with('#')) {
        return None;
    }
    let Ok(Value::Mapping(mapping)) = serde_yaml_ng::from_str::<Value>(yaml) else {
        return None;
    };
    mapping
        .keys()
        .any(|key| RECOGNIZED_FRONTMATTER_KEYS.contains(&key_text(key).as_str()))
        .then_some(mapping)
}

/// Whether `text` opens, after blank lines, with a block that looks like
/// engram frontmatter ([`frontmatter_block`]).
pub fn opens_with_frontmatter(text: &str) -> bool {
    let text = to_lf(text);
    split_frontmatter(text.trim_start_matches('\n'))
        .is_some_and(|parts| frontmatter_block(parts.yaml).is_some())
}

/// Whether the body of the engram `source` opens with a second block that
/// looks like engram frontmatter, under the engram's own: what a capture
/// whose content opened with a block stored before 0.34.0. Every reader takes
/// the first block only, so the keys of the second are lost to them.
pub fn body_opens_with_frontmatter(source: &str) -> bool {
    let source = to_lf(source);
    split_frontmatter(&source).is_some_and(|parts| opens_with_frontmatter(parts.body))
}

/// A second frontmatter block at the top of an engram's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondBlock {
    /// One-based file line of the block's opening rule.
    pub line: usize,
    /// The block's keys, in its order.
    pub keys: Vec<String>,
}

/// The second block the body of `source` opens with, or `None` when it opens
/// with none ([`body_opens_with_frontmatter`]).
pub fn second_block(source: &str) -> Option<SecondBlock> {
    // CRLF reads as LF, which keeps the line numbers.
    let source = to_lf(source);
    let source = source.as_ref();
    let parts = split_frontmatter(source)?;
    let lead = parts.body.len() - parts.body.trim_start_matches('\n').len();
    let inner = split_frontmatter(&parts.body[lead..])?;
    let mapping = frontmatter_block(inner.yaml)?;
    let before = parts.open.len() + parts.yaml.len() + parts.close.len() + lead;
    Some(SecondBlock {
        line: source[..before].matches('\n').count() + 1,
        keys: mapping.keys().map(key_text).collect(),
    })
}

/// What [`merge_second_block`] made of an engram with a second block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockMerge {
    /// The engram with the second block's keys in its own frontmatter and the
    /// block gone from the body.
    Merged(String),
    /// Why the block cannot be merged by these rules.
    NotRepairable(String),
}

/// Merge the second block the body of `source` opens with into the engram's
/// own frontmatter, by the rules a write lifts a block in its content by
/// (`write_engram`): the engram's own keys stand for the write's parameters
/// and win over the same key in the block; `permalink`, `recorded_at`,
/// `timestamp` and `generated` in the block are dropped; a `title`, `type`,
/// `status` or `tags` with no value is left out, and one that is a list or a
/// mapping (tags: a mapping) cannot be merged. A `type: engram`, a `status:
/// stable`, an empty title and no tags are what a write fills in when its
/// parameter is left out, so the block's value takes their place. The body
/// is the text under the block, as a write stores its content.
///
/// `None` when the body opens with no such block. A CRLF source is read as
/// LF, and the merge is LF, as every stored engram is. The frontmatter is
/// written anew, as an overwrite writes it: YAML comments and the key order
/// of the engram's own block are not kept. Only the first stacked block is
/// merged; [`merge_second_blocks`] merges them all.
pub fn merge_second_block(source: &str) -> Option<BlockMerge> {
    let source = to_lf(source);
    let source = source.as_ref();
    let parts = split_frontmatter(source)?;
    let inner = split_frontmatter(parts.body.trim_start_matches('\n'))?;
    let block = frontmatter_block(inner.yaml)?;
    let mut engram = match parse_engram(source) {
        Ok(engram) => engram,
        Err(e) => {
            return Some(BlockMerge::NotRepairable(format!(
                "the engram's own frontmatter does not parse: {e}"
            )));
        }
    };
    let own = match serde_yaml_ng::from_str::<Value>(parts.yaml) {
        Ok(Value::Mapping(own)) => own,
        _ => serde_yaml_ng::Mapping::new(),
    };
    let has_own = |key: &str| own.keys().any(|k| key_text(k) == key);
    let fm = &mut engram.frontmatter;
    for (key, value) in block {
        let key = key_text(&key);
        if ENGINE_OWNED_KEYS.contains(&key.as_str()) {
            continue;
        }
        let typed = matches!(key.as_str(), "title" | "type" | "status" | "tags");
        if typed && value.is_null() {
            continue;
        }
        match key.as_str() {
            "title" if fm.title.trim().is_empty() => match block_scalar(&value) {
                Some(title) => fm.title = title,
                None => return Some(not_plain_text(&key)),
            },
            "type" if fm.engram_type.is_empty() || fm.engram_type == "engram" => {
                match block_scalar(&value) {
                    Some(kind) => fm.engram_type = kind,
                    None => return Some(not_plain_text(&key)),
                }
            }
            "status" if matches!(fm.status.as_deref(), None | Some("stable")) => {
                match block_scalar(&value) {
                    Some(status) => fm.status = Some(status),
                    None => return Some(not_plain_text(&key)),
                }
            }
            "tags" if fm.tags.is_empty() => match block_tags(&value) {
                Some(tags) => fm.tags = tags,
                None => return Some(not_plain_text(&key)),
            },
            _ if typed || has_own(&key) => {}
            _ => {
                fm.extra.insert(key, crate::YamlValue::from_backend(value));
            }
        }
    }
    if let Err(e) = crate::temporal::normalize_temporal_fields(fm) {
        return Some(BlockMerge::NotRepairable(e.to_string()));
    }
    if let Err(e) = crate::temporal::normalize_verified(fm) {
        return Some(BlockMerge::NotRepairable(e.to_string()));
    }
    engram.body = format!("\n{}\n", inner.body.trim_matches('\n'));
    let merged = crate::emit::emit_engram(&engram);
    if parse_engram(&merged).is_err() {
        return Some(BlockMerge::NotRepairable(
            "the merged frontmatter does not parse".to_string(),
        ));
    }
    Some(BlockMerge::Merged(merged))
}

/// [`merge_second_block`] until the body opens with no block, for an engram
/// that holds several stacked ones. At most 64 rounds, then the text as far
/// as it got is not repairable. `None` when there is no block to merge.
pub fn merge_second_blocks(source: &str) -> Option<BlockMerge> {
    let mut text = match merge_second_block(source)? {
        BlockMerge::Merged(text) => text,
        refused => return Some(refused),
    };
    for _ in 0..64 {
        match merge_second_block(&text) {
            None => return Some(BlockMerge::Merged(text)),
            Some(BlockMerge::Merged(next)) => text = next,
            Some(refused) => return Some(refused),
        }
    }
    Some(BlockMerge::NotRepairable(
        "the body opens with more stacked blocks than one repair merges".to_string(),
    ))
}

fn not_plain_text(key: &str) -> BlockMerge {
    BlockMerge::NotRepairable(format!(
        "the second block gives {key} a value that is not plain text"
    ))
}

/// A one-value key (title, type, status) of a block as text, as a write
/// lifts it.
fn block_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `tags` of a block as a list, from a YAML list or a comma-separated line,
/// as a write lifts it.
fn block_tags(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Null => Some(Vec::new()),
        Value::String(s) => Some(
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        Value::Sequence(items) => items.iter().map(block_scalar).collect(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What 0.33.0 stored for a capture whose content opened with a block:
    /// its own generated block (the parameters and the defaults of the ones
    /// left out), then the content's block as body text.
    const DOUBLED: &str = "---\ntype: engram\ntitle: Cache Policy\npermalink: cache-policy\ntags: []\nstatus: stable\nrecorded_at: 2026-11-01\ngenerated: { by: agent, at: 2026-11-01T10:00:00Z }\nowner: kept\n---\n\n---\ntitle: Ignored Title\ntype: decision\nstatus: draft\ntags:\n  - from-block\nsalience: 7\nvalid_from: 2026-01-01\nowner: block\nteam: core\npermalink: somewhere/else\nrecorded_at: 2020-01-01\n---\n\n# Cache Policy\n\nWe cache for five minutes.\n";

    #[test]
    fn a_second_block_is_found_with_its_line_and_keys() {
        let block = second_block(DOUBLED).expect("found");
        assert_eq!(block.line, 12);
        assert_eq!(block.keys[..3], ["title", "type", "status"]);
        assert!(body_opens_with_frontmatter(DOUBLED));
        let single = "---\ntitle: T\n---\n\n# T\n\n---\nstatus: draft\n---\n";
        assert_eq!(second_block(single), None);
        assert!(!body_opens_with_frontmatter(single));
        // Markdown between two rules is no block.
        let rules = "---\ntitle: T\n---\n---\nNote: remember this\n---\n";
        assert_eq!(second_block(rules), None);
        assert_eq!(merge_second_block(rules), None);
    }

    #[test]
    fn a_second_block_merges_by_the_rules_a_write_lifts_it_by() {
        let Some(BlockMerge::Merged(text)) = merge_second_block(DOUBLED) else {
            panic!("not merged");
        };
        let engram = parse_engram(&text).unwrap();
        let fm = &engram.frontmatter;
        // The engram's own keys win, unless they are what a write fills in.
        assert_eq!(fm.title, "Cache Policy");
        assert_eq!(fm.engram_type, "decision");
        assert_eq!(fm.status.as_deref(), Some("draft"));
        assert_eq!(fm.tags, ["from-block"]);
        assert_eq!(fm.extra.get("owner").and_then(|v| v.as_str()), Some("kept"));
        assert_eq!(fm.extra.get("team").and_then(|v| v.as_str()), Some("core"));
        assert_eq!(fm.extra.get("salience"), Some(&crate::YamlValue::Int(7)));
        assert_eq!(
            fm.valid_from.map(|d| d.ymd()).as_deref(),
            Some("2026-01-01")
        );
        // The keys a write owns stay the engram's.
        assert_eq!(fm.permalink.as_deref(), Some("cache-policy"));
        assert_eq!(
            fm.recorded_at.map(|d| d.ymd()).as_deref(),
            Some("2026-11-01")
        );
        assert_eq!(fm.generated.as_ref().map(|g| g.by.as_str()), Some("agent"));
        // One block, and the body is the text under the second one.
        assert_eq!(second_block(&text), None);
        assert_eq!(
            engram.body,
            "\n# Cache Policy\n\nWe cache for five minutes.\n"
        );
        // A merge of a merged engram has nothing left to do.
        assert_eq!(merge_second_block(&text), None);
    }

    #[test]
    fn a_crlf_engram_s_second_block_is_found_and_merged() {
        let crlf = DOUBLED.replace('\n', "\r\n");
        assert!(body_opens_with_frontmatter(&crlf));
        assert!(opens_with_frontmatter(
            "\r\n---\r\nstatus: draft\r\n---\r\nIntro"
        ));
        assert_eq!(second_block(&crlf), second_block(DOUBLED));
        assert_eq!(merge_second_block(&crlf), merge_second_block(DOUBLED));
    }

    #[test]
    fn stacked_blocks_are_merged_until_none_is_left() {
        let stacked = DOUBLED.replacen(
            "---\n\n# Cache Policy",
            "---\n---\nresource: https://example.com/a\n---\n---\nreview_after: 2027-01-01\n---\n\n# Cache Policy",
            1,
        );
        let Some(BlockMerge::Merged(one)) = merge_second_block(&stacked) else {
            panic!("not merged");
        };
        assert!(
            second_block(&one).is_some(),
            "one round leaves the next block"
        );
        let Some(BlockMerge::Merged(all)) = merge_second_blocks(&stacked) else {
            panic!("not merged");
        };
        assert_eq!(second_block(&all), None);
        let engram = parse_engram(&all).unwrap();
        assert_eq!(engram.frontmatter.engram_type, "decision");
        assert_eq!(
            engram.frontmatter.resource.as_deref(),
            Some("https://example.com/a")
        );
        assert!(
            engram.frontmatter.stale_after.is_some() || engram.frontmatter.review_after.is_some()
        );
        assert_eq!(
            engram.body,
            "\n# Cache Policy\n\nWe cache for five minutes.\n"
        );
        assert_eq!(merge_second_blocks(&all), None);
    }

    #[test]
    fn a_second_block_a_write_would_refuse_is_not_merged() {
        let doubled = "---\ntype: engram\ntitle: T\npermalink: t\ntags: []\nstatus: stable\n---\n---\ntype: [a, b]\n---\nBody\n";
        assert!(matches!(
            merge_second_block(doubled),
            Some(BlockMerge::NotRepairable(why)) if why.contains("type")
        ));
    }

    // Review Focus 3 of the time-module move: two `generated` blocks naming
    // the same instant in different offsets rank equal on the instant, so the
    // greater block text wins, in both merge orders, as in 0.33.
    #[test]
    fn generated_merge_picks_like_0_33() {
        let lines: &dyn Fn(&str, &str, &str) -> Option<String> = &|_, _, _| None;
        let doc = |at: &str| {
            format!(
                "---\ntype: engram\ntitle: T\ngenerated: {{by: human:a, at: '{at}'}}\n---\n\nbody\n"
            )
        };
        let base = doc("2026-01-01T00:00:00+00:00");
        let utc = doc("2026-01-05T10:00:00+00:00");
        let east = doc("2026-01-05T12:00:00+02:00");
        assert_eq!(
            merge_text(Some(&base), &utc, &east, lines),
            MergedText::Clean(east.clone())
        );
        assert_eq!(
            merge_text(Some(&base), &east, &utc, lines),
            MergedText::Clean(east.clone())
        );
        // A later instant still wins over a greater text.
        let later = doc("2026-01-05T10:00:01+00:00");
        assert_eq!(
            merge_text(Some(&base), &later, &east, lines),
            MergedText::Clean(later.clone())
        );
    }

    const DOC: &str = "---\ntype: engram\ntitle: T\npermalink: t\n---\n\n# T\n\nBody.\n";

    #[test]
    fn merge_text_with_keeps_the_body_merge_out_of_the_frontmatter() {
        let lines: &dyn Fn(&str, &str, &str) -> Option<String> = &|_, _, _| None;
        let body: &dyn Fn(&str, &str, &str) -> Option<String> = &|_, _, _| Some("merged\n".into());
        let base = "---\ntype: engram\ntitle: T\ndate: 1\n---\n\nbody\n";
        let local = base.replace("body", "local");
        let upstream = base.replace("body", "upstream");
        assert_eq!(
            merge_text_with(Some(base), &local, &upstream, lines, body),
            MergedText::Clean(base.replace("\nbody\n", "merged\n"))
        );
        // A key both sides changed goes to the line merge, never the body's.
        let local = base.replace("date: 1", "date: 2");
        let upstream = base.replace("date: 1", "date: 3");
        assert_eq!(
            merge_text_with(Some(base), &local, &upstream, lines, body),
            MergedText::Conflict
        );
        // No frontmatter anywhere: the whole text is body.
        assert_eq!(
            merge_text_with(Some("a\n"), "b\n", "c\n", lines, body),
            MergedText::Clean("merged\n".into())
        );
        // One side with a frontmatter that cannot be merged by key: lines.
        assert_eq!(
            merge_text_with(Some("a\n"), "---\ntype: x\n---\n\nb\n", "c\n", lines, body),
            MergedText::Conflict
        );
    }

    #[test]
    fn same_content_reads_values_not_spelling() {
        let flat = "---\ntype: engram\ntags:\n- a\n- b\n---\n\nbody\n";
        // The indented and quoted 0.25.1 style holds the same list.
        let indented = "---\ntype: engram\ntags:\n  - \"a\"\n  - b # kept\n---\n\nbody\n";
        let flow = "---\ntype: engram\ntags: [a, b]\n---\n\nbody\n";
        assert!(same_content(flat, indented));
        assert!(same_content(flat, flow));
        // Order is a change, and so is the body.
        assert!(!same_content(
            flat,
            "---\ntype: engram\ntags:\n- b\n- a\n---\n\nbody\n"
        ));
        assert!(!same_content(
            flat,
            "---\ntype: engram\ntags:\n- a\n- b\n---\n\nbody.\n"
        ));
        // A frontmatter that does not parse is never the same.
        let broken = "---\ntype: [engram\n---\n\nbody\n";
        assert!(!same_content(broken, broken));
        assert!(same_content("just text\n", "just text\n"));
        assert!(!same_content("just text\n", flat));
    }

    /// Every key the parser reads into a typed field is a recognized key: a
    /// key that lands outside `extra` is one the schema knows.
    #[test]
    fn every_key_the_parser_types_is_recognized() {
        for key in [
            "type",
            "title",
            "permalink",
            "tags",
            "status",
            "description",
            "resource",
            "temporal_confidence",
            "recorded_at",
            "valid_from",
            "valid_to",
            "source_date",
            "verified",
            "last_verified",
            "stale_after",
            "review_after",
            "timestamp",
            "generated",
        ] {
            assert!(RECOGNIZED_FRONTMATTER_KEYS.contains(&key), "{key}");
        }
        // And nothing else is read into a typed field: an unlisted key lands
        // in `extra` as a custom key.
        let source = "---\nowner: x\n---\nbody\n";
        let engram = parse_engram(source).unwrap();
        assert!(engram.frontmatter.extra.contains_key("owner"));
    }

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

    const HARBOR: &str = "---\ntype: manifest\ntitle: Harbor\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\n---\n\n# Harbor\n\n## Scope\n\n- Engineering\n";

    fn with(text: &str, after: &str, line: &str) -> String {
        text.replacen(after, &format!("{after}{line}"), 1)
    }

    #[test]
    fn a_key_both_sides_added_with_one_value_lands_once_in_upstreams_place() {
        let local = with(HARBOR, "status: stable\n", "domain_name: harbor\n");
        let upstream = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn a_key_both_sides_added_with_different_values_is_a_conflict() {
        let local = with(HARBOR, "status: stable\n", "domain_name: harbor\n");
        let upstream = with(HARBOR, "title: Harbor\n", "domain_name: harbor-eng\n");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_key_deleted_on_one_side_and_changed_on_the_other_is_a_conflict() {
        let local = HARBOR.replace("status: stable\n", "");
        let upstream = HARBOR.replace("status: stable", "status: archived");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_key_deleted_on_one_side_and_unchanged_on_the_other_is_gone() {
        let local = HARBOR.replace("status: stable\n", "");
        let upstream = with(HARBOR, "- Engineering\n", "- Warp\n");
        let expected = with(&local, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_key_only_local_added_follows_its_local_predecessor_and_upstream_spelling_stays() {
        let local = with(HARBOR, "permalink: manifest\n", "owner: kim\n");
        let upstream = HARBOR.replace("status: stable", "status: 'archived'");
        let expected = with(&upstream, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn an_upstream_respelling_of_an_equal_value_wins() {
        let local = with(HARBOR, "- Engineering\n", "- Warp\n");
        let upstream = HARBOR.replace("status: stable", "status: 'stable'");
        let expected = with(&upstream, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_comment_only_local_added_survives_an_upstream_change() {
        let local = with(
            HARBOR,
            "permalink: manifest\n",
            "# ask kim before renaming\n",
        );
        let upstream = HARBOR.replace("status: stable", "status: archived");
        let expected = with(
            &upstream,
            "permalink: manifest\n",
            "# ask kim before renaming\n",
        );
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_repeat_already_in_local_is_repaired_when_upstream_moves() {
        let base = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n");
        let local = with(&base, "status: stable\n", "domain_name: harbor\n");
        let upstream = with(&base, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(&base), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn a_repeat_with_different_values_in_one_side_is_a_conflict() {
        let local = with(HARBOR, "status: stable\n", "status: archived\n");
        let upstream = with(HARBOR, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_side_that_cannot_be_cut_steps_aside_before_a_repeat_conflicts() {
        let local = HARBOR.replace("title: Harbor", "title: \"Harbor");
        let upstream = with(HARBOR, "status: stable\n", "status: archived\n");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn a_list_changed_on_both_sides_merges_through_the_line_merge() {
        let local = HARBOR.replace("  - manifest\n", "  - manifest\n  - c\n");
        let upstream = HARBOR.replace("tags:\n  - manifest\n", "tags:\n  - z\n  - manifest\n");
        let block_merge = |b: &str, l: &str, u: &str| {
            if b == "tags:\n  - manifest\n" {
                Some("tags:\n  - z\n  - manifest\n  - c\n".to_string())
            } else {
                trivial(b, l, u)
            }
        };
        let expected = HARBOR.replace(
            "tags:\n  - manifest\n",
            "tags:\n  - z\n  - manifest\n  - c\n",
        );
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &block_merge),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_block_line_merge_that_spills_into_another_key_is_a_conflict() {
        let local = HARBOR.replace("  - manifest\n", "  - manifest\n  - c\n");
        let upstream = HARBOR.replace("tags:\n  - manifest\n", "tags:\n  - z\n  - manifest\n");
        let spilling = |b: &str, l: &str, u: &str| {
            if b == "tags:\n  - manifest\n" {
                Some("tags:\n  - z\nowner: kim\n".to_string())
            } else {
                trivial(b, l, u)
            }
        };
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &spilling),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn mixed_line_endings_merge_by_key_as_lf() {
        let local = with(HARBOR, "status: stable\n", "domain_name: harbor\n");
        let upstream = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n");
        assert_eq!(
            merge_text(
                Some(HARBOR),
                &local,
                &upstream.replace('\n', "\r\n"),
                &trivial
            ),
            MergedText::Clean(upstream)
        );
    }

    #[test]
    fn crlf_on_every_side_merges_as_lf() {
        let base = HARBOR.replace('\n', "\r\n");
        let local = with(HARBOR, "status: stable\n", "domain_name: harbor\n").replace('\n', "\r\n");
        let upstream = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n");
        assert_eq!(
            merge_text(
                Some(&base),
                &local,
                &upstream.replace('\n', "\r\n"),
                &trivial
            ),
            MergedText::Clean(upstream)
        );
    }

    #[test]
    fn a_side_without_frontmatter_steps_aside() {
        assert_eq!(
            merge_frontmatter(Some(HARBOR), "# plain\n", HARBOR, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn a_base_without_frontmatter_counts_as_an_empty_one() {
        let base = "\n# Harbor\n\n## Scope\n\n- Engineering\n";
        let local = with(HARBOR, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(Some(base), &local, HARBOR, &trivial),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn both_sides_added_the_file_keys_on_one_side_merge() {
        let local = with(HARBOR, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(None, &local, HARBOR, &trivial),
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
        let base = with(HARBOR, "status: stable\n", "status: archived\n");
        let local = with(HARBOR, "- Engineering\n", "- Warp\n");
        let upstream = HARBOR.replace("status: stable", "status: archived");
        assert_eq!(
            merge_frontmatter(Some(&base), &local, &upstream, &trivial),
            FrontmatterMerge::NotApplicable
        );
    }

    #[test]
    fn a_repeat_in_the_base_and_in_local_is_still_a_conflict() {
        let base = with(HARBOR, "status: stable\n", "status: archived\n");
        let local = with(&base, "- Engineering\n", "- Warp\n");
        let upstream = HARBOR.replace("status: stable", "status: archived");
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
            HARBOR.replace("  - manifest\n", "  - manifest\n  - c\n"),
            HARBOR.replace("tags:\n  - manifest\n", "tags:\n  - z\n  - manifest\n"),
        )
    }

    #[test]
    fn a_block_line_merge_without_a_final_line_ending_is_a_conflict() {
        let (local, upstream) = tags_on_both_sides();
        let merge = tags_merged_as("tags:\n  - z\n  - manifest\n  - c");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &merge),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_block_line_merge_that_adds_a_preamble_is_a_conflict() {
        let (local, upstream) = tags_on_both_sides();
        let merge = tags_merged_as("# c\ntags:\n  - z\n  - manifest\n  - c\n");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &merge),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_block_line_merge_that_renames_the_key_is_a_conflict() {
        let (local, upstream) = tags_on_both_sides();
        let merge = tags_merged_as("labels:\n  - z\n  - manifest\n  - c\n");
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &merge),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn a_preamble_changed_on_one_side_is_kept() {
        let local = HARBOR.replacen("---\n", "---\n# p local\n", 1);
        let upstream = HARBOR.replace("status: stable", "status: archived");
        let expected = upstream.replacen("---\n", "---\n# p local\n", 1);
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_preamble_changed_differently_on_both_sides_is_a_conflict() {
        let local = HARBOR.replacen("---\n", "---\n# p local\n", 1);
        let upstream = HARBOR.replacen("---\n", "---\n# p upstream\n", 1);
        assert_eq!(
            merge_frontmatter(Some(HARBOR), &local, &upstream, &trivial),
            FrontmatterMerge::Conflict
        );
    }

    #[test]
    fn two_keys_only_local_added_keep_their_order_after_their_predecessor() {
        let local = with(HARBOR, "title: Harbor\n", "x: 1\ny: 2\n");
        let upstream = HARBOR.replace("status: stable", "status: archived");
        let expected = with(&upstream, "title: Harbor\n", "x: 1\ny: 2\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_key_only_local_added_with_no_predecessor_goes_first() {
        let local = HARBOR.replacen("---\n", "---\nfirst: 0\n", 1);
        let upstream = HARBOR.replacen("---\n", "---\nzero: z\n", 1);
        let expected = HARBOR.replacen("---\n", "---\nfirst: 0\nzero: z\n", 1);
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
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
            key_blocks(split_frontmatter(HARBOR).expect("frontmatter").yaml).expect("splits");
        assert!(consistent(&right.preamble, &right.blocks));
    }

    #[test]
    fn a_repeat_with_one_value_keeps_its_first_copy() {
        let local = with(
            &with(HARBOR, "title: Harbor\n", "domain_name: harbor\n"),
            "status: stable\n",
            "domain_name: 'harbor'\n",
        );
        let upstream = with(HARBOR, "- Engineering\n", "- Warp\n");
        let expected = with(&upstream, "title: Harbor\n", "domain_name: harbor\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_crlf_base_with_lf_sides_merges_as_lf() {
        let base = HARBOR.replace('\n', "\r\n");
        let local = with(HARBOR, "status: stable\n", "domain_name: harbor\n");
        let upstream = with(HARBOR, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(&base), &local, &upstream, &trivial),
            MergedText::Clean(with(&local, "- Engineering\n", "- Warp\n"))
        );
    }

    #[test]
    fn bodies_that_settle_never_reach_the_line_merge() {
        let local = with(HARBOR, "permalink: manifest\n", "owner: kim\n");
        let upstream = with(HARBOR, "- Engineering\n", "- Warp\n");
        let expected = with(&upstream, "permalink: manifest\n", "owner: kim\n");
        let refusing = |_: &str, _: &str, _: &str| None;
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &refusing),
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

    /// HARBOR with `title` and `permalink` swapped.
    fn harbor_title_after_permalink(text: &str) -> String {
        text.replace(
            "title: Harbor\npermalink: manifest\n",
            "permalink: manifest\ntitle: Harbor\n",
        )
    }

    #[test]
    fn a_local_reorder_survives_a_body_only_upstream_change() {
        let local = harbor_title_after_permalink(HARBOR);
        let upstream = with(HARBOR, "- Engineering\n", "- Warp\n");
        let expected = with(&local, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_key_only_upstream_added_lands_after_its_upstream_predecessor_in_a_local_order() {
        let local = harbor_title_after_permalink(HARBOR);
        let upstream = with(HARBOR, "permalink: manifest\n", "owner: kim\n");
        let expected = with(&local, "permalink: manifest\n", "owner: kim\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn an_upstream_reorder_wins_when_local_kept_the_base_order() {
        let local = with(HARBOR, "- Engineering\n", "- Warp\n");
        let upstream = harbor_title_after_permalink(HARBOR);
        let expected = with(&upstream, "- Engineering\n", "- Warp\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn when_both_sides_reordered_upstreams_order_wins() {
        let local = harbor_title_after_permalink(HARBOR);
        let upstream = HARBOR.replace(
            "tags:\n  - manifest\nstatus: stable\n",
            "status: stable\ntags:\n  - manifest\n",
        );
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }

    #[test]
    fn without_a_base_upstreams_order_wins() {
        let local = harbor_title_after_permalink(HARBOR);
        assert_eq!(
            merge_text(None, &local, HARBOR, &trivial),
            MergedText::Clean(HARBOR.to_string())
        );
    }

    #[test]
    fn a_local_comment_survives_an_upstream_requoting_of_the_same_value() {
        let local = HARBOR.replace("status: stable", "status: stable # keep");
        let upstream = HARBOR.replace("status: stable", "status: \"stable\"");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn a_local_comment_on_the_key_line_is_a_conflict_when_upstream_changes_the_value() {
        let local = HARBOR.replace("status: stable", "status: stable # reviewed by kim");
        let upstream = HARBOR.replace("status: stable", "status: archived");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Conflict
        );
    }

    #[test]
    fn a_local_comment_under_the_key_is_a_conflict_when_upstream_changes_the_value() {
        let local = with(HARBOR, "title: Harbor\n", "# the ship's engineer\n");
        let upstream = HARBOR.replace("title: Harbor", "title: Montgomery Scott");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Conflict
        );
    }

    #[test]
    fn a_local_comment_is_kept_when_the_line_merge_carries_upstreams_value() {
        let local = HARBOR.replace("tags:\n", "tags:\n  # keep\n");
        let upstream = HARBOR.replace("  - manifest\n", "  - manifest\n  - warp\n");
        let merge = tags_merged_as("tags:\n  # keep\n  - manifest\n  - warp\n");
        let expected = HARBOR.replace(
            "tags:\n  - manifest\n",
            "tags:\n  # keep\n  - manifest\n  - warp\n",
        );
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &merge),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_line_merge_that_loses_upstreams_value_under_a_local_comment_is_a_conflict() {
        let local = HARBOR.replace("tags:\n", "tags:\n  # keep\n");
        let upstream = HARBOR.replace("  - manifest\n", "  - manifest\n  - warp\n");
        let merge = tags_merged_as("tags:\n  # keep\n  - manifest\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &merge),
            MergedText::Conflict
        );
    }

    #[test]
    fn a_local_comment_on_a_key_upstream_removed_is_a_conflict() {
        let local = HARBOR.replace("status: stable", "status: stable # reviewed by kim");
        let upstream = HARBOR.replace("status: stable\n", "");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Conflict
        );
    }

    #[test]
    fn an_upstream_comment_on_the_key_line_is_a_conflict_when_local_changes_the_value() {
        let local = HARBOR.replace("status: stable", "status: archived");
        let upstream = HARBOR.replace("status: stable", "status: stable # reviewed by kim");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Conflict
        );
    }

    #[test]
    fn an_upstream_comment_under_the_key_is_a_conflict_when_local_changes_the_value() {
        let local = HARBOR.replace("title: Harbor", "title: Montgomery Scott");
        let upstream = with(HARBOR, "title: Harbor\n", "# the ship's engineer\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Conflict
        );
    }

    #[test]
    fn an_upstream_comment_is_kept_when_the_line_merge_carries_locals_value() {
        let local = HARBOR.replace("  - manifest\n", "  - manifest\n  - warp\n");
        let upstream = HARBOR.replace("tags:\n", "tags:\n  # keep\n");
        let merge = tags_merged_as("tags:\n  # keep\n  - manifest\n  - warp\n");
        let expected = HARBOR.replace(
            "tags:\n  - manifest\n",
            "tags:\n  # keep\n  - manifest\n  - warp\n",
        );
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &merge),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_line_merge_that_loses_locals_value_under_an_upstream_comment_is_a_conflict() {
        let local = HARBOR.replace("  - manifest\n", "  - manifest\n  - warp\n");
        let upstream = HARBOR.replace("tags:\n", "tags:\n  # keep\n");
        let merge = tags_merged_as("tags:\n  # keep\n  - manifest\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &merge),
            MergedText::Conflict
        );
    }

    #[test]
    fn an_upstream_comment_on_a_key_local_removed_is_a_conflict() {
        let local = HARBOR.replace("status: stable\n", "");
        let upstream = HARBOR.replace("status: stable", "status: stable # reviewed by kim");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Conflict
        );
    }

    #[test]
    fn both_respellings_of_an_equal_list_are_line_merged() {
        let local = HARBOR.replace("tags:\n", "tags:\n  # keep\n");
        let upstream = HARBOR.replace("  - manifest\n", "  - \"manifest\"\n");
        let merge = tags_merged_as("tags:\n  # keep\n  - \"manifest\"\n");
        let expected = HARBOR.replace(
            "tags:\n  - manifest\n",
            "tags:\n  # keep\n  - \"manifest\"\n",
        );
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &merge),
            MergedText::Clean(expected)
        );
    }

    #[test]
    fn a_line_merge_of_two_respellings_that_changes_the_value_keeps_local() {
        let local = HARBOR.replace("tags:\n", "tags:\n  # keep\n");
        let upstream = HARBOR.replace("  - manifest\n", "  - \"manifest\"\n");
        let merge = tags_merged_as("tags:\n  - other\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &merge),
            MergedText::Clean(local.clone())
        );
    }

    #[test]
    fn a_key_both_added_with_one_value_in_two_spellings_takes_upstreams_spelling() {
        let local = with(HARBOR, "title: Harbor\n", "domain_name: 'harbor'\n");
        let upstream = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n");
        assert_eq!(
            merge_text(Some(HARBOR), &local, &upstream, &trivial),
            MergedText::Clean(upstream.clone())
        );
    }
}

#[cfg(test)]
mod same_but_generated_tests {
    use super::same_but_generated;

    const WITH_LINE: &str = "---\ntitle: Plan\ngenerated:\n  at: 2026-11-06T10:00:00Z\n  by: human:alice\nstatus: draft\n---\nbody\n";

    #[test]
    fn same_but_generated_ignores_only_that_key() {
        // Another stamp, the stamp gone, the stamp in another place, a second copy.
        for same in [
            "---\ntitle: Plan\ngenerated:\n  at: 2026-11-07T10:00:00Z\n  by: human:bob\nstatus: draft\n---\nbody\n",
            "---\ntitle: Plan\nstatus: draft\n---\nbody\n",
            "---\ngenerated:\n  at: 2026-11-07T10:00:00Z\ntitle: Plan\nstatus: draft\n---\nbody\n",
            "---\ntitle: Plan\ngenerated: a\nstatus: draft\ngenerated: b\n---\nbody\n",
            "---\ntitle: Plan\n\"generated\": quoted\nstatus: draft\n---\nbody\n",
        ] {
            assert!(same_but_generated(WITH_LINE, same), "{same}");
            assert!(same_but_generated(same, WITH_LINE), "{same}");
        }
        // Every other change counts: a value, quoting, the order of two other
        // keys, the body, a comment of hers at column 0 behind the stamp, the
        // key spelled another way, a key of that name inside another key, and
        // a body line that only looks like the key.
        for differs in [
            "---\ntitle: Other\ngenerated: x\nstatus: draft\n---\nbody\n",
            "---\ntitle: \"Plan\"\ngenerated: x\nstatus: draft\n---\nbody\n",
            "---\nstatus: draft\ngenerated: x\ntitle: Plan\n---\nbody\n",
            "---\ntitle: Plan\ngenerated: x\nstatus: draft\n---\nbody two\n",
            "---\ntitle: Plan\ngenerated: x\n# her note\nstatus: draft\n---\nbody\n",
            "---\ntitle: Plan\nGenerated: x\nstatus: draft\n---\nbody\n",
            "---\ntitle: Plan\nstatus: draft\nother:\n  generated: x\n---\nbody\n",
            "---\n# her note\ntitle: Plan\nstatus: draft\n---\nbody\n",
        ] {
            assert!(!same_but_generated(WITH_LINE, differs), "{differs}");
            assert!(!same_but_generated(differs, WITH_LINE), "{differs}");
        }
        assert!(!same_but_generated(
            "---\ntitle: Plan\n---\ngenerated: a\n",
            "---\ntitle: Plan\n---\ngenerated: b\n",
        ));
    }

    #[test]
    fn same_but_generated_reads_as_lf() {
        assert!(same_but_generated(
            "---\r\ntitle: Plan\r\ngenerated: a\r\n---\r\nbody\r\n",
            "---\ntitle: Plan\ngenerated: b\n---\nbody\n",
        ));
    }

    #[test]
    fn same_but_generated_asks_for_the_whole_text_where_it_cannot_cut_keys() {
        // No frontmatter at all.
        assert!(same_but_generated("plain\n", "plain\n"));
        assert!(same_but_generated("plain\n", "plain\r\n"));
        assert!(!same_but_generated("plain\n", "other\n"));
        // The closing line counts as text too.
        assert!(!same_but_generated(
            "---\ntitle: Plan\ngenerated: a\n---",
            "---\ntitle: Plan\ngenerated: b\n---\n",
        ));
        // A stamp is no reason to call a file with frontmatter the same as one with none.
        assert!(!same_but_generated(
            "---\ngenerated: a\n---\nplain\n",
            "plain\n",
        ));
        // Frontmatter that cannot be cut into keys.
        let broken = "---\n{title: Plan}\n---\nbody\n";
        assert!(same_but_generated(broken, broken));
        assert!(!same_but_generated(
            broken,
            "---\n{title: Plan, generated: a}\n---\nbody\n",
        ));
    }
}
