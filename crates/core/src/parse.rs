//! Parsing an Engram from markdown source.
//!
//! [`parse_engram`] returns a fully typed [`Engram`]. [`parse_engram_lossless`]
//! additionally keeps the raw frontmatter text and byte spans so the surgical
//! editors in [`crate::emit`] can string-edit the original source without a
//! full re-emission.
//!
//! The body is scanned line by line with code-fence tracking; inline code
//! spans are masked before link and observation detection so knowledge inside
//! code never leaks into the graph.

use std::borrow::Cow;
use std::ops::Range;

use chrono::{DateTime, FixedOffset, NaiveDate};
use indexmap::IndexMap;

use crate::engram::{
    Engram, Frontmatter, Generated, Heading, LinkTarget, Observation, Relation, SchemaDef,
    Verified, WikiLink,
};
use crate::yaml::YamlValue;

/// The generation of what the parser extracts from an engram: its
/// observations, relations, links, headings and tags.
///
/// **Bump it in the same change whenever the parser starts extracting
/// something different from the same text.** The index records the
/// generation each domain was last parsed with, and a sync that finds an
/// older one reparses every engram of that domain once - a file domain from
/// its files, a virtual domain from the content the database stores -
/// without rewriting a byte of either, and then records this value. Without
/// the bump, an engram nobody edits keeps the rows an older parser derived
/// until its next write.
///
/// - 0: every index before the generation was recorded.
/// - 1: a bullet that wraps onto further lines is one observation or
///   relation (0.22).
/// - 2: task boxes (`[ ]`, `[x]`, `[X]`, `[~]`) and directive-led bullets
///   stopped reading as observations and relations (0.28.0).
/// - 3: relative Markdown links to another engram's file (`[p](../p-12.md)`)
///   are links, and `- rel_type [text](path.md)` is a relation (0.29.0).
/// - 4: a relative link to a folder's `index.md` or `log.md` is not a link to
///   an engram (0.29.0).
/// - 5: nor is a wikilink spelled as the path of one (0.29.0).
/// - 6: the type of a relation written with a relative link is lowercase
///   (`part_of`) or a quoted phrase, so `- See [Three](three.md)` is prose
///   (0.29.1).
/// - 7: a bullet that opens with a footnote reference (`- [^1] text`) is no
///   observation (0.29.1).
/// - 8: a bare link destination ends only at an ASCII space or control
///   character, so a U+3000 in a linked file name is part of the path
///   (0.29.1).
/// - 9: a `[[...]]` inside an inline link's destination is part of that
///   path, not a wikilink (0.29.1).
/// - 10: a `#heading` after a wikilink target is a fragment, not part of the
///   target (0.31.0).
pub const PARSE_GENERATION: u32 = 10;

/// An error encountered while parsing an Engram.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    /// The file begins with a UTF-8 byte order mark.
    #[error("file starts with a UTF-8 byte order mark")]
    Bom,
    /// The file contains a null byte.
    #[error("file contains a null byte at line {line}, column {column}")]
    NullByte {
        /// One-based line of the null byte.
        line: usize,
        /// One-based column of the null byte.
        column: usize,
    },
    /// The frontmatter block is not valid YAML.
    #[error("frontmatter YAML is invalid: {message}")]
    Yaml {
        /// The YAML backend's error message.
        message: String,
    },
    /// The frontmatter parsed to something other than a mapping.
    #[error("frontmatter is not a mapping")]
    FrontmatterNotMapping,
    /// The file's bytes are not UTF-8 text. Raised by whoever reads the
    /// bytes, since a `&str` can never hold such a file.
    #[error("file is not valid UTF-8 (first bad byte on line {line})")]
    NotUtf8 {
        /// One-based line holding the first invalid sequence.
        line: usize,
    },
}

/// A parsed Engram plus enough of the original source to edit it losslessly.
#[derive(Debug, Clone)]
pub struct LosslessEngram {
    /// The typed Engram.
    pub engram: Engram,
    /// The full original source, retained verbatim.
    pub source: String,
    /// Whether a frontmatter block was present.
    pub has_frontmatter: bool,
    /// The raw YAML text between the delimiters (no delimiters).
    pub raw_frontmatter: String,
    /// Byte span of the raw frontmatter within `source`.
    pub frontmatter_span: Range<usize>,
    /// Byte span of the body within `source`.
    pub body_span: Range<usize>,
    /// One-based source line at which the body begins.
    pub body_line_start: usize,
}

impl LosslessEngram {
    /// Return the original source unchanged. Reconstruction is trivially
    /// byte-identical because the source is retained.
    pub fn reconstruct(&self) -> &str {
        &self.source
    }
}

/// Parse markdown source into a typed [`Engram`].
pub fn parse_engram(source: &str) -> Result<Engram, ParseError> {
    check_encoding(source)?;
    let (has_fm, fm_span, body_start) = locate(source);
    let raw_fm = if has_fm { &source[fm_span.clone()] } else { "" };
    let frontmatter = parse_frontmatter(raw_fm)?;
    let body = source[body_start..].to_string();
    let body_line_start = line_of_offset(source, body_start);
    let (observations, relations, links, headings) = scan_body(&body, body_line_start);
    Ok(Engram {
        frontmatter,
        body,
        observations,
        relations,
        links,
        headings,
    })
}

/// Parse markdown source, keeping raw frontmatter text and byte spans.
pub fn parse_engram_lossless(source: &str) -> Result<LosslessEngram, ParseError> {
    check_encoding(source)?;
    let (has_fm, fm_span, body_start) = locate(source);
    let raw_fm = if has_fm {
        source[fm_span.clone()].to_string()
    } else {
        String::new()
    };
    let frontmatter = parse_frontmatter(&raw_fm)?;
    let body = source[body_start..].to_string();
    let body_line_start = line_of_offset(source, body_start);
    let (observations, relations, links, headings) = scan_body(&body, body_line_start);
    let engram = Engram {
        frontmatter,
        body,
        observations,
        relations,
        links,
        headings,
    };
    Ok(LosslessEngram {
        engram,
        source: source.to_string(),
        has_frontmatter: has_fm,
        raw_frontmatter: raw_fm,
        frontmatter_span: fm_span,
        body_span: body_start..source.len(),
        body_line_start,
    })
}

fn check_encoding(source: &str) -> Result<(), ParseError> {
    if source.starts_with('\u{feff}') {
        return Err(ParseError::Bom);
    }
    if let Some(pos) = source.find('\0') {
        let line = line_of_offset(source, pos);
        let line_start = source[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let column = source[line_start..pos].chars().count() + 1;
        return Err(ParseError::NullByte { line, column });
    }
    Ok(())
}

/// Locate the frontmatter block. Returns `(has_frontmatter, raw_fm_span,
/// body_start_byte)`. The span covers the YAML text between the delimiters.
pub(crate) fn locate(source: &str) -> (bool, Range<usize>, usize) {
    let first_nl = match source.find('\n') {
        Some(i) => i,
        None => return (false, 0..0, 0),
    };
    let first_line = &source[..first_nl];
    if first_line.trim_end_matches('\r') != "---" {
        return (false, 0..0, 0);
    }
    let fm_start = first_nl + 1;
    let mut idx = fm_start;
    loop {
        let (line_end, next, had_nl) = match source[idx..].find('\n') {
            Some(r) => (idx + r, idx + r + 1, true),
            None => (source.len(), source.len(), false),
        };
        let line = &source[idx..line_end];
        if line.trim_end_matches('\r') == "---" {
            return (true, fm_start..idx, next);
        }
        if !had_nl {
            // No closing delimiter: treat as a document without frontmatter.
            return (false, 0..0, 0);
        }
        idx = next;
    }
}

fn parse_frontmatter(raw: &str) -> Result<Frontmatter, ParseError> {
    if raw.trim().is_empty() {
        return Ok(Frontmatter::default());
    }
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(raw).map_err(|e| ParseError::Yaml {
            message: e.to_string(),
        })?;
    let mapping = match value {
        serde_yaml_ng::Value::Mapping(m) => m,
        serde_yaml_ng::Value::Null => return Ok(Frontmatter::default()),
        _ => return Err(ParseError::FrontmatterNotMapping),
    };

    let pairs: Vec<(String, serde_yaml_ng::Value)> = mapping
        .into_iter()
        .map(|(k, v)| (key_to_string(&k), v))
        .collect();

    let engram_type = pairs
        .iter()
        .find(|(k, _)| k == "type")
        .and_then(|(_, v)| v.as_str())
        .unwrap_or("")
        .to_string();
    let is_schema = engram_type == "schema";

    let mut fm = Frontmatter::default();
    let mut extra: IndexMap<String, YamlValue> = IndexMap::new();
    let mut schema_def = SchemaDef::default();
    let mut saw_schema_key = false;

    for (key, value) in pairs {
        match key.as_str() {
            "type" => fm.engram_type = scalar_string(&value).unwrap_or_default(),
            "title" => fm.title = scalar_string(&value).unwrap_or_default(),
            "permalink" => fm.permalink = opt_scalar_string(&value),
            "tags" => fm.tags = parse_tags(&value),
            "status" => fm.status = opt_scalar_string(&value),
            "description" => fm.description = opt_scalar_string(&value),
            "resource" => fm.resource = opt_scalar_string(&value),
            "temporal_confidence" => fm.temporal_confidence = opt_scalar_string(&value),
            "recorded_at" => set_date(&mut fm.recorded_at, &mut extra, &key, value),
            "valid_from" => set_date(&mut fm.valid_from, &mut extra, &key, value),
            "valid_to" => set_date(&mut fm.valid_to, &mut extra, &key, value),
            "source_date" => set_date(&mut fm.source_date, &mut extra, &key, value),
            "verified" => set_verified(&mut fm.verified, &mut extra, &key, value),
            "last_verified" => set_date(&mut fm.last_verified, &mut extra, &key, value),
            "stale_after" => set_date(&mut fm.stale_after, &mut extra, &key, value),
            "review_after" => set_date(&mut fm.review_after, &mut extra, &key, value),
            "timestamp" => set_timestamp(&mut fm.timestamp, &mut extra, &key, value),
            "generated" => set_generated(&mut fm.generated, &mut extra, &key, value),
            "entity" if is_schema => {
                saw_schema_key = true;
                schema_def.entity = opt_scalar_string(&value);
            }
            "version" if is_schema => {
                saw_schema_key = true;
                schema_def.version = value.as_i64();
            }
            "schema" if is_schema => {
                saw_schema_key = true;
                schema_def.schema = to_indexmap(value);
            }
            "settings" if is_schema => {
                saw_schema_key = true;
                schema_def.settings = to_indexmap(value);
            }
            _ => {
                extra.insert(key, YamlValue::from_backend(value));
            }
        }
    }

    if is_schema || saw_schema_key {
        fm.schema_def = Some(schema_def);
    }
    fm.extra = extra;
    Ok(fm)
}

fn key_to_string(key: &serde_yaml_ng::Value) -> String {
    match key {
        serde_yaml_ng::Value::String(s) => s.clone(),
        other => other.as_str().map(str::to_string).unwrap_or_else(|| {
            serde_yaml_ng::to_string(other)
                .unwrap_or_default()
                .trim_end()
                .to_string()
        }),
    }
}

fn scalar_string(v: &serde_yaml_ng::Value) -> Option<String> {
    match v {
        serde_yaml_ng::Value::String(s) => Some(s.clone()),
        serde_yaml_ng::Value::Bool(b) => Some(b.to_string()),
        serde_yaml_ng::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn opt_scalar_string(v: &serde_yaml_ng::Value) -> Option<String> {
    match v {
        serde_yaml_ng::Value::Null => None,
        other => scalar_string(other).filter(|s| !s.is_empty()),
    }
}

fn parse_tags(v: &serde_yaml_ng::Value) -> Vec<String> {
    match v {
        serde_yaml_ng::Value::Sequence(seq) => seq
            .iter()
            .filter_map(scalar_string)
            .filter(|s| !s.is_empty())
            .collect(),
        serde_yaml_ng::Value::String(s) => s
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn set_date(
    field: &mut Option<NaiveDate>,
    extra: &mut IndexMap<String, YamlValue>,
    key: &str,
    value: serde_yaml_ng::Value,
) {
    if let Some(s) = value.as_str()
        && let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d")
    {
        *field = Some(d);
        return;
    }
    // Not a parseable ISO date: keep it verbatim so nothing is lost. This is
    // only reachable for non-canonical files; verify flags it later.
    extra.insert(key.to_string(), YamlValue::from_backend(value));
}

fn set_timestamp(
    field: &mut Option<DateTime<FixedOffset>>,
    extra: &mut IndexMap<String, YamlValue>,
    key: &str,
    value: serde_yaml_ng::Value,
) {
    if let Some(s) = value.as_str()
        && let Ok(ts) = DateTime::parse_from_rfc3339(s)
    {
        *field = Some(ts);
        return;
    }
    extra.insert(key.to_string(), YamlValue::from_backend(value));
}

/// Parse the OKF v0.2 `generated: { by, at }` mapping. `by` is required by the
/// spec, so a value that is not a mapping carrying a non-empty `by` is kept
/// verbatim in `extra` instead of being silently reshaped; verify flags it
/// later, exactly as it does a malformed `timestamp`.
fn set_generated(
    field: &mut Option<Generated>,
    extra: &mut IndexMap<String, YamlValue>,
    key: &str,
    value: serde_yaml_ng::Value,
) {
    if let serde_yaml_ng::Value::Mapping(map) = &value {
        let by = map
            .get(serde_yaml_ng::Value::String("by".into()))
            .and_then(scalar_string)
            .filter(|s| !s.trim().is_empty());
        if let Some(by) = by {
            let at = map
                .get(serde_yaml_ng::Value::String("at".into()))
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok());
            // The model the writer reported, when it reported one. Read as a
            // string and nothing else, the way a `verified` entry reads it
            // (`crystalline_core::engram::reported_model`), so the same value
            // means the same thing in both blocks: absent, blank or not a
            // string all read as absence, and the block still names the actor,
            // which is what the spec requires of it.
            let model = map
                .get(serde_yaml_ng::Value::String("model".into()))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string);
            *field = Some(Generated { by, model, at });
            return;
        }
    }
    extra.insert(key.to_string(), YamlValue::from_backend(value));
}

/// Parse the OKF v0.2 `verified` family: a list of `{ by, at }` entries, with a
/// bare mapping read as a one-element list per §11. A value that is not a
/// well-formed entry or list of entries is kept verbatim in `extra` instead of
/// being silently reshaped; verify flags it later, exactly as it does a
/// malformed `generated`.
fn set_verified(
    field: &mut Vec<Verified>,
    extra: &mut IndexMap<String, YamlValue>,
    key: &str,
    value: serde_yaml_ng::Value,
) {
    let yaml = YamlValue::from_backend(value);
    match Verified::parse_list(&yaml) {
        Some(entries) => *field = entries,
        None => {
            extra.insert(key.to_string(), yaml);
        }
    }
}

fn to_indexmap(value: serde_yaml_ng::Value) -> IndexMap<String, YamlValue> {
    match YamlValue::from_backend(value) {
        YamlValue::Mapping(m) => m,
        _ => IndexMap::new(),
    }
}

fn line_of_offset(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

// --- body scanning -----------------------------------------------------------

/// A body line with its absolute line number and fence state.
pub(crate) struct BodyLine<'a> {
    pub line_no: usize,
    pub text: &'a str,
    pub in_fence: bool,
}

/// Iterate body lines, tracking fenced code blocks. Lines inside a fence (and
/// the fence markers themselves) are flagged `in_fence`.
pub(crate) fn body_lines(body: &str, body_line_start: usize) -> Vec<BodyLine<'_>> {
    let mut out = Vec::new();
    let mut fence = FenceTracker::new();
    for (i, raw) in body.split('\n').enumerate() {
        let text = raw.trim_end_matches('\r');
        out.push(BodyLine {
            line_no: body_line_start + i,
            text,
            in_fence: fence.in_fence(text),
        });
    }
    out
}

fn scan_body(
    body: &str,
    body_line_start: usize,
) -> (Vec<Observation>, Vec<Relation>, Vec<WikiLink>, Vec<Heading>) {
    let mut observations = Vec::new();
    let mut relations = Vec::new();
    let mut links = Vec::new();
    let mut headings = Vec::new();

    let lines = body_lines(body, body_line_start);
    // A wrapped relation's target may sit on a continuation line, so the
    // exclusion of that target from the prose links lasts for the whole
    // bullet: the index of its last line, and the target still to exclude.
    let mut relation_target: Option<(usize, LinkTarget)> = None;
    for (idx, bl) in lines.iter().enumerate() {
        if bl.in_fence {
            continue;
        }
        let line = bl.text;
        let line_no = bl.line_no;
        if relation_target.as_ref().is_some_and(|(end, _)| idx > *end) {
            relation_target = None;
        }

        if let Some((level, text)) = parse_heading(line) {
            headings.push(Heading {
                line: line_no,
                level,
                text,
            });
            continue;
        }

        // Top-level bullets (zero indent) can be observations or relations,
        // read over the bullet's first line and every line that continues it.
        if let Some(first) = top_level_bullet(line) {
            let end = bullet_end(&lines, idx);
            let end_line = lines[end].line_no;
            let content = bullet_text(first, &lines[idx + 1..=end]);
            if let Some((category, rest)) = parse_observation_head(&content) {
                let (obs_content, tags, context) = split_observation(&rest);
                observations.push(Observation {
                    line: line_no,
                    end_line,
                    category,
                    content: obs_content,
                    tags,
                    context,
                });
            } else if let Some((rel_type, target)) = parse_relation(&content) {
                relation_target = Some((end, target.clone()));
                relations.push(Relation {
                    line: line_no,
                    end_line,
                    rel_type,
                    target,
                });
            }
        }

        // Wikilinks anywhere on the line, excluding a relation target and
        // deduplicated per line. Nothing to find on a line without "[[".
        if line.contains("[[") {
            let masked = mask_for_wikilinks(line);
            let mut seen: Vec<LinkTarget> = Vec::new();
            for target in find_wikilinks(&masked)
                .iter()
                .filter_map(|inner| LinkTarget::wikilink(inner))
                .filter(|target| !names_a_reserved_file(&target.target))
            {
                push_prose_link(target, line_no, &mut relation_target, &mut seen, &mut links);
            }
        }

        // Relative Markdown links to another engram's file, read the same way:
        // outside inline code, the relation's own target excluded once,
        // deduplicated per line.
        if line.contains("](") {
            let mut seen: Vec<LinkTarget> = Vec::new();
            for (_, path) in relative_note_links(line) {
                push_prose_link(
                    LinkTarget::relative(&path),
                    line_no,
                    &mut relation_target,
                    &mut seen,
                    &mut links,
                );
            }
        }
    }

    (observations, relations, links, headings)
}

/// Add one prose link found on a line, unless it is the target of the relation
/// bullet it sits in or the line already linked it.
fn push_prose_link(
    target: LinkTarget,
    line: usize,
    relation_target: &mut Option<(usize, LinkTarget)>,
    seen: &mut Vec<LinkTarget>,
    links: &mut Vec<WikiLink>,
) {
    // `same_place`: `[[a]]` and `[[a|text]]` are one target here.
    if relation_target
        .as_ref()
        .is_some_and(|(_, rt)| target.same_place(rt))
    {
        // Excluded once: a second link to the same target in the bullet is
        // prose like any other.
        *relation_target = None;
        return;
    }
    if !seen.iter().any(|s| s.same_place(&target)) {
        seen.push(target.clone());
        links.push(WikiLink { line, target });
    }
}

/// The inline links on one line that point at another engram's file: a
/// relative destination (no scheme, no leading `/`, not a bare `#fragment`)
/// whose path ends in `.md` in any case. Each comes with the byte offset of the
/// `[` that opens its text, and its path read the way a reflection link is
/// read: angle brackets or bare, backslash escapes removed, percent-decoded,
/// without the `#fragment` and the title. A link inside inline code, an
/// image and a link to a folder's `index.md` or `log.md` are not links to a
/// note.
fn relative_note_links(line: &str) -> Vec<(usize, String)> {
    use crate::reflection::links::{code_spans, read_destination};

    let spans = code_spans(line);
    let in_code = |at: usize| spans.iter().any(|(s, e)| (*s..*e).contains(&at));
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(found) = line[cursor..].find("](") {
        let close_text = cursor + found;
        let open = close_text + 2;
        cursor = open;
        if link_text_start(line, close_text).is_none() {
            continue;
        }
        let Some(dest) = read_destination(line, open) else {
            continue;
        };
        cursor = dest.close;
        if let Some(start) = note_link_start(line, close_text, &dest.path)
            && !in_code(close_text)
            && !names_a_reserved_file(&dest.path)
        {
            out.push((start, dest.path));
        }
    }
    out
}

/// Whether a link's path (a relative link's, or a wikilink spelled as a
/// path) ends in a folder's `index.md` or `log.md`: the files Crystalline
/// keeps beside the engrams, never an engram. A link to one is sound, like a
/// link to a web page, and no engram's link. The names are exact, as the
/// walker reserves them: a file named `Index.md` is an engram like any other.
fn names_a_reserved_file(path: &str) -> bool {
    crate::index::is_reserved_path(path)
}

/// The byte offset of the `[` that opens a link to another engram's file,
/// whose text closes at `close_text` and whose decoded destination is `path`:
/// a relative path ending in `.md` in any case, and not an image. `None` for
/// any other link. Whether the link sits in inline code is the caller's
/// question.
pub(crate) fn note_link_start(line: &str, close_text: usize, path: &str) -> Option<usize> {
    let start = link_text_start(line, close_text)?;
    let image = line[..start].ends_with('!');
    let note = path.to_ascii_lowercase().ends_with(".md");
    (!image && note && crate::reflection::links::is_relative(path)).then_some(start)
}

/// The byte offset of the `[` that opens the link text closed by the `]` at
/// `close`: the nearest unescaped `[` before it at bracket depth zero.
fn link_text_start(line: &str, close: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut depth = 0usize;
    let mut i = close;
    while i > 0 {
        i -= 1;
        let escaped = i > 0 && bytes[i - 1] == b'\\';
        match bytes[i] {
            b']' if !escaped => depth += 1,
            b'[' if !escaped => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// The index of the last line of the top-level bullet that starts at
/// `lines[start]`: the bullet's own line plus every line that continues its
/// first paragraph, the way CommonMark reads a list item. A continuation line
/// is indented under the bullet or, as a lazy continuation, not indented at
/// all. The first of these ends the bullet's text: a blank line, a code fence,
/// a heading, another list item (a nested `  - ...` is its own bullet), a
/// block quote, a thematic break, an HTML block start (`<`) or a table row
/// (`|`). Returns `start` for a bullet on one line.
///
/// A line that ends inside an unclosed `[[` link, or a bullet's first line
/// whose `[category` bracket is still open, is not joined with the next one:
/// the wikilink rename and the near-miss check read one line at a time, so a
/// link or a category split over two lines stays the per-line typo it always
/// was rather than becoming a relation or an observation they cannot see.
pub(crate) fn bullet_end(lines: &[BodyLine<'_>], start: usize) -> usize {
    if unclosed_category(lines[start].text) {
        return start;
    }
    let mut end = start;
    for (offset, bl) in lines.iter().enumerate().skip(start + 1) {
        if bl.in_fence || !continues_bullet(bl.text) || ends_inside_link(lines[end].text) {
            break;
        }
        end = offset;
    }
    end
}

/// Whether a line that follows a bullet's text continues it rather than
/// ending it. See [`bullet_end`] for the rule. Also what the section editors
/// ask before they put a block directly above an existing line, so an
/// inserted bullet never swallows the prose under it.
pub(crate) fn continues_bullet(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || parse_heading(line).is_some() {
        return false;
    }
    // A fence indented deeper than a top-level fence (one inside the item)
    // ends the text too, though the fence tracking does not see it.
    if fence_marker(trimmed).is_some() {
        return false;
    }
    !(starts_list_item(trimmed)
        || trimmed.starts_with('>')
        || trimmed.starts_with('<')
        || trimmed.starts_with('|')
        || is_thematic_break(trimmed))
}

/// Whether the line's last `[[` (outside inline code and link destinations)
/// has no `]]` after it.
fn ends_inside_link(line: &str) -> bool {
    let masked = mask_for_wikilinks(line);
    match masked.rfind("[[") {
        Some(open) => !masked[open + 2..].contains("]]"),
        None => false,
    }
}

/// Whether a top-level bullet line opens a `[category` bracket it never
/// closes on that line.
fn unclosed_category(line: &str) -> bool {
    line.strip_prefix("- [")
        .is_some_and(|rest| !rest.contains(']'))
}

/// Whether trimmed line text opens a list item that may interrupt a
/// paragraph: a `-`, `*` or `+` marker followed by a space, a tab or nothing,
/// or an ordered marker numbered 1 (`1.` or `1)`), the only number CommonMark
/// lets interrupt a paragraph. Any other number is paragraph text, so
/// `2024. was the year` continues the bullet.
fn starts_list_item(trimmed: &str) -> bool {
    let marker_then_gap = |rest: &str| rest.is_empty() || rest.starts_with([' ', '\t']);
    if let Some(rest) = trimmed.strip_prefix(['-', '*', '+']) {
        return marker_then_gap(rest);
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits)
        && trimmed[..digits].parse::<u32>() == Ok(1)
        && let Some(rest) = trimmed[digits..].strip_prefix(['.', ')'])
    {
        return marker_then_gap(rest);
    }
    false
}

/// Whether trimmed line text is a thematic break: three or more of one of
/// `-`, `*` or `_`, with nothing else but spaces and tabs.
fn is_thematic_break(trimmed: &str) -> bool {
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 0;
    for c in trimmed.chars() {
        if c == first {
            count += 1;
        } else if c != ' ' && c != '\t' {
            return false;
        }
    }
    count >= 3
}

/// The text of a wrapped bullet: the content after `- ` on its first line
/// and every continuation line, each trimmed, joined with single spaces.
fn bullet_text<'a>(first: &'a str, continuation: &[BodyLine<'_>]) -> Cow<'a, str> {
    if continuation.is_empty() {
        return Cow::Borrowed(first);
    }
    let mut text = first.trim_end().to_string();
    for bl in continuation {
        text.push(' ');
        text.push_str(bl.text.trim());
    }
    Cow::Owned(text)
}

/// Parse an ATX heading, returning `(level, text)`.
pub fn parse_heading(line: &str) -> Option<(u8, String)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &rest[hashes..];
    if !after.is_empty() && !after.starts_with(' ') && !after.starts_with('\t') {
        return None;
    }
    Some((hashes as u8, strip_closing_hashes(after.trim())))
}

fn strip_closing_hashes(s: &str) -> String {
    let trimmed = s.trim_end();
    let without = trimmed.trim_end_matches('#');
    if without.len() == trimmed.len() {
        return trimmed.to_string();
    }
    if without.is_empty() {
        return String::new();
    }
    if without.ends_with(' ') || without.ends_with('\t') {
        without.trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn fence_marker(line: &str) -> Option<(char, usize, usize)> {
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
        return None;
    }
    Some((first, count, indent))
}

/// The one fenced-code rule, shared by every walk over a body's lines. A
/// fence opens on a marker (see `fence_marker`) and closes only on a marker of
/// the same character, at least as long, with nothing after it.
///
/// Feed it each line in order, line ending already stripped.
#[derive(Debug, Default, Clone)]
pub struct FenceTracker {
    open: Option<(char, usize)>,
}

impl FenceTracker {
    /// A tracker outside any fence.
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance over one line. True for an opening or closing marker line and
    /// for every line inside the fence, false for a line outside fenced code.
    pub fn in_fence(&mut self, line: &str) -> bool {
        match self.open {
            None => {
                if let Some((c, n, _)) = fence_marker(line) {
                    self.open = Some((c, n));
                    true
                } else {
                    false
                }
            }
            Some((open_char, open_count)) => {
                if let Some((c, n, _)) = fence_marker(line)
                    && c == open_char
                    && n >= open_count
                    && line.trim_start()[n..].trim().is_empty()
                {
                    self.open = None;
                }
                true
            }
        }
    }
}

fn top_level_bullet(line: &str) -> Option<&str> {
    // Zero indent only; nested or indented bullets are not observations or
    // relations.
    line.strip_prefix("- ")
}

/// The leading `[category]` of a bullet's content and the text after it, or
/// `None` when the content does not start with one. A bracket directly
/// followed by `(` is a markdown inline link and one directly followed by `[`
/// a reference link, so neither is a category, and a bracket that opens with
/// `^` is a footnote reference (`- [^1] text`), as it is to
/// `crate::linked`'s reference definitions. The one place that decides it:
/// the parser, the tag rewriter and the Markdown lint all ask here.
pub(crate) fn split_category(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix('[')?;
    let close = rest.find(']')?;
    let category = &rest[..close];
    if category.is_empty() || category.contains('[') || category.starts_with('^') {
        return None;
    }
    // A GFM task box, open, done or not applicable, is not a category.
    if matches!(category, " " | "x" | "X" | "~") {
        return None;
    }
    let after = &rest[close + 1..];
    if after.starts_with(['(', '[']) {
        return None;
    }
    Some((category, after))
}

fn parse_observation_head(content: &str) -> Option<(String, String)> {
    let (category, after) = split_category(content)?;
    Some((category.trim().to_string(), after.trim_start().to_string()))
}

fn split_observation(body: &str) -> (String, Vec<String>, Option<String>) {
    let mut s = body.trim_end().to_string();

    // A trailing parenthesized group is context.
    let mut context = None;
    if s.ends_with(')')
        && let Some(open) = s.rfind('(')
    {
        let inner = s[open + 1..s.len() - 1].trim().to_string();
        context = Some(inner);
        s = s[..open].trim_end().to_string();
    }

    // Trailing hashtags, stripped from the end.
    let mut tags = Vec::new();
    loop {
        let t = s.trim_end();
        let start = t.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        let token = &t[start..];
        if is_hashtag(token) {
            tags.push(token[1..].to_string());
            s = t[..start].trim_end().to_string();
        } else {
            break;
        }
    }
    tags.reverse();

    (s.trim().to_string(), tags, context)
}

pub(crate) fn is_hashtag(token: &str) -> bool {
    let Some(body) = token.strip_prefix('#') else {
        return false;
    };
    !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/')
}

/// A relation bullet's type and target: `rel_type [[Target]]`, or
/// `rel_type [text](path.md)` with a relative link to another engram's file.
/// The wikilink form is read first, exactly as it always was.
pub(crate) fn parse_relation(content: &str) -> Option<(String, LinkTarget)> {
    wiki_relation(content).or_else(|| relative_relation(content))
}

fn wiki_relation(content: &str) -> Option<(String, LinkTarget)> {
    let open = content.find("[[")?;
    // A `[[` in a link destination (`- rel[Doc](a[[x]]b.md)`) is part of a
    // path, as the prose scan and the move rewrites read it.
    if content.contains("](") {
        let at = content[..open].chars().count();
        if mask_for_wikilinks(content).chars().nth(at) != Some('[') {
            return None;
        }
    }
    let after = &content[open + 2..];
    let close = after.find("]]")?;
    let inner = &after[..close];
    let rel_type = relation_type(&content[..open])?;
    let target = LinkTarget::wikilink(inner)?;
    // A folder's `index.md` or `log.md` is no engram to relate to.
    (!names_a_reserved_file(&target.target)).then_some((rel_type, target))
}

/// `rel_type [text](path.md)`: the bullet's first `[` opens a relative link to
/// a note, and what stands before it is the type.
fn relative_relation(content: &str) -> Option<(String, LinkTarget)> {
    let open = content.find('[')?;
    let (_, path) = relative_note_links(content)
        .into_iter()
        .find(|(start, _)| *start == open)?;
    let rel_type = relative_relation_type(&content[..open])?;
    Some((rel_type, LinkTarget::relative(&path)))
}

/// The type of a relation written with a relative link: a quoted phrase, or
/// a token that starts with a lowercase letter and holds only lowercase
/// letters, digits, `_` and `-`, the way `part_of` is written. Narrower than
/// [`relation_type`] because a converted document is full of bullets like
/// `- See [Three](chapter-3.md)`, where the word before the link is prose,
/// not a type. The wikilink form keeps the free token: engrams written
/// before the relative form existed depend on it.
fn relative_relation_type(before: &str) -> Option<String> {
    let trimmed = before.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return Some(trimmed[1..trimmed.len() - 1].to_string());
    }
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    let rest_ok = chars.all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    (first.is_lowercase() && rest_ok).then(|| trimmed.to_string())
}

/// The relation type written before a relation's target: a quoted phrase, or a
/// single token that does not open with `:` or `[`.
fn relation_type(before: &str) -> Option<String> {
    let before = before.trim();
    if before.len() >= 2 && before.starts_with('"') && before.ends_with('"') {
        Some(before[1..before.len() - 1].to_string())
    } else if !before.is_empty()
        && !before.contains(char::is_whitespace)
        && !before.starts_with([':', '['])
    {
        Some(before.to_string())
    } else {
        None
    }
}

/// Replace inline code span contents (including the backticks) with spaces so
/// wikilinks and observation markers inside code are ignored.
///
/// Masks in place in a single `Vec<char>`: every index the scan reads is
/// fully read before any slot in that span is written, and once a span is
/// blanked the scan pointer jumps past it, so a read never observes a
/// write from an earlier iteration.
pub(crate) fn mask_inline_code(line: &str) -> String {
    let mut chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '`' {
            let mut j = i;
            while j < n && chars[j] == '`' {
                j += 1;
            }
            let run = j - i;
            let mut k = j;
            let mut found = None;
            while k < n {
                if chars[k] == '`' {
                    let mut m = k;
                    while m < n && chars[m] == '`' {
                        m += 1;
                    }
                    if m - k == run {
                        found = Some(m);
                        break;
                    }
                    k = m;
                } else {
                    k += 1;
                }
            }
            if let Some(end) = found {
                for slot in chars.iter_mut().take(end).skip(i) {
                    *slot = ' ';
                }
                i = end;
                continue;
            } else {
                i = j;
                continue;
            }
        }
        i += 1;
    }
    chars.into_iter().collect()
}

/// The inside of every `[[...]]` of an engram body, as written, one list per
/// body line and in order, outside fenced code, inline code and link
/// destinations: what the parser reads as wikilinks, before it trims them. A
/// repair that quotes a reference reads its text from here.
pub fn wikilinks_by_line(body: &str) -> Vec<Vec<String>> {
    body_lines(body, 1)
        .into_iter()
        .map(|line| {
            if line.in_fence || !line.text.contains("[[") {
                return Vec::new();
            }
            find_wikilinks(&mask_for_wikilinks(line.text))
        })
        .collect()
}

/// A line as every wikilink scan reads it: inline code masked
/// ([`mask_inline_code`]) and then the destination of every inline link
/// ([`mask_link_destinations`]). A `[[x]]` in either is no wikilink, so
/// `[Doc](a[[x]]b.md)` links a file and never the engram `x`. Each mask runs
/// only when the line can hold what it hides, and both keep the line's char
/// count, the alignment the move rewrites in `crate::relink` depend on.
pub(crate) fn mask_for_wikilinks(line: &str) -> Cow<'_, str> {
    let code: Cow<'_, str> = if line.contains('`') {
        Cow::Owned(mask_inline_code(line))
    } else {
        Cow::Borrowed(line)
    };
    if !code.contains("](") {
        return code;
    }
    match mask_link_destinations(&code) {
        Cow::Owned(masked) => Cow::Owned(masked),
        Cow::Borrowed(_) => code,
    }
}

/// `line` with the inside of every inline link destination, between `](`
/// and its `)`, replaced by spaces, one per char so the char count stays.
/// The link text is kept: `[odd [[x]]](b.md)` still holds a wikilink. The
/// destinations are found the way [`relative_note_links`] finds them, a `](`
/// that closes a link text and opens a destination that closes on the line,
/// but every one counts, not only a link to a note. Call it on a line with
/// inline code already masked, so a `](` inside code opens nothing.
pub(crate) fn mask_link_destinations(line: &str) -> Cow<'_, str> {
    use crate::reflection::links::read_destination;

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0;
    while let Some(found) = line[cursor..].find("](") {
        let close_text = cursor + found;
        let open = close_text + 2;
        cursor = open;
        if link_text_start(line, close_text).is_none() {
            continue;
        }
        let Some(dest) = read_destination(line, open) else {
            continue;
        };
        ranges.push((open, dest.close));
        cursor = dest.close;
    }
    if ranges.is_empty() {
        return Cow::Borrowed(line);
    }
    let masked = line
        .char_indices()
        .map(|(at, c)| {
            if ranges.iter().any(|(s, e)| (*s..*e).contains(&at)) {
                ' '
            } else {
                c
            }
        })
        .collect();
    Cow::Owned(masked)
}

fn find_wikilinks(masked: &str) -> Vec<String> {
    let mut res = Vec::new();
    let mut rest = masked;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        if let Some(close) = after.find("]]") {
            res.push(after[..close].to_string());
            rest = &after[close + 2..];
        } else {
            break;
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engram::LinkForm;

    // Pins the current mask_inline_code/scan_body behavior before the
    // single-buffer and contains-guard refactor so the refactor cannot
    // change observable output.

    #[test]
    fn fence_tracker_closes_only_on_the_same_marker_at_least_as_long_with_nothing_after() {
        let mut fence = FenceTracker::new();
        assert!(!fence.in_fence("prose"));
        assert!(fence.in_fence("````"), "the opener is a fence line");
        assert!(fence.in_fence("```"), "a shorter run does not close");
        assert!(fence.in_fence("~~~~"), "another character does not close");
        assert!(
            fence.in_fence("```` tail"),
            "text after the run does not close"
        );
        assert!(
            fence.in_fence("   `````  "),
            "a longer run closes, the closer is a fence line"
        );
        assert!(!fence.in_fence("after"), "the fence is shut");
        assert!(fence.in_fence("~~~"), "a tilde fence opens");
        assert!(fence.in_fence("    ```"), "indent over three is content");
        assert!(fence.in_fence("~~~"));
        assert!(!fence.in_fence("done"));

        // A fence that is never closed runs to the end of the input: every
        // line after the opener stays inside it.
        let mut unclosed = FenceTracker::new();
        assert!(unclosed.in_fence("```rust"), "the opener is a fence line");
        assert!(unclosed.in_fence("let x = 1;"));
        assert!(
            unclosed.in_fence("``` trailing"),
            "text after the run does not close"
        );
        assert!(unclosed.in_fence(""), "an empty last line is still inside");
    }

    #[test]
    fn mask_inline_code_leaves_unterminated_backtick_run_unmasked() {
        let line = "text `unterminated [[Link]] end";
        assert_eq!(mask_inline_code(line), line);
    }

    #[test]
    fn mask_inline_code_masks_whole_span_with_unequal_nested_backtick_runs() {
        let line = "before ``a`b`` after";
        let masked = mask_inline_code(line);
        assert_eq!(masked.chars().count(), line.chars().count());
        assert!(masked.starts_with("before "));
        assert!(masked.ends_with(" after"));
        let middle = &masked["before ".len()..masked.len() - " after".len()];
        assert!(
            middle.chars().all(|c| c == ' '),
            "expected the whole unequal nested run masked to spaces, got {middle:?}"
        );
    }

    #[test]
    fn mask_inline_code_is_identity_when_no_backticks_present() {
        let line = "plain text with [[Wikilink]] and no backticks at all";
        assert_eq!(mask_inline_code(line), line);
    }

    #[test]
    fn mask_inline_code_blanks_multibyte_content_by_char_not_byte() {
        let prefix = "日本";
        let inner = "コード";
        let suffix = "語";
        let line = format!("{prefix}`{inner}`{suffix}");
        let masked = mask_inline_code(&line);
        assert_eq!(masked.chars().count(), line.chars().count());
        assert!(masked.starts_with(prefix));
        assert!(masked.ends_with(suffix));
        assert!(!masked.contains(inner));
        for ch in inner.chars() {
            assert!(!masked.contains(ch));
        }
        assert!(!masked.contains('`'));
    }

    #[test]
    fn mask_inline_code_hides_wikilink_inside_code_span() {
        let line = "prefix `[[Inside]]` suffix";
        let masked = mask_inline_code(line);
        assert!(find_wikilinks(&masked).is_empty());
    }

    #[test]
    fn scan_body_keeps_wikilink_outside_code_span_and_drops_one_inside() {
        let body = "see `[[Inside]]` and [[Outside]] too";
        let (_, _, links, _) = scan_body(body, 1);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target.target, "Outside");
        assert_eq!(links[0].line, 1);
    }

    /// A `[[...]]` inside an inline link's destination is part of that
    /// path, not a wikilink. The link text is still read.
    #[test]
    fn a_wikilink_inside_a_link_destination_is_no_link() {
        let (_, relations, links, _) = scan_body("- part_of [Doc](a[[x]]b.md)\n", 1);
        assert_eq!(relations.len(), 1, "{relations:?}");
        assert_eq!(relations[0].target.target, "a[[x]]b.md");
        assert_eq!(relations[0].target.form, LinkForm::Relative);
        assert!(links.is_empty(), "{links:?}");

        // Nor does such a `[[` make a wikilink relation to `x` of the text
        // before it: the bullet relates to the file.
        let (_, relations, links, _) = scan_body("- rel[Doc](a[[x]]b.md)\n", 1);
        assert_eq!(relations.len(), 1, "{relations:?}");
        assert_eq!(relations[0].target.target, "a[[x]]b.md");
        assert_eq!(relations[0].target.form, LinkForm::Relative);
        assert!(links.is_empty(), "{links:?}");

        let body = "See [t](https://x.test/[[x]]), ![i](p[[x]].png), \
                    [r](<a [[x]].md> \"[[x]]\"), [odd [[y]]](b.md) and [[z]].\n";
        let (_, _, links, _) = scan_body(body, 1);
        let targets: Vec<&str> = links.iter().map(|l| l.target.target.as_str()).collect();
        assert_eq!(targets, ["y", "z", "a [[x]].md", "b.md"]);

        // Multibyte text in a masked destination keeps the char alignment.
        let (_, _, links, _) = scan_body("[\u{65e5}](\u{65e5}\u{672c}[[x]].md) [[\u{8a9e}]]\n", 1);
        let targets: Vec<&str> = links.iter().map(|l| l.target.target.as_str()).collect();
        assert_eq!(targets, ["\u{8a9e}", "\u{65e5}\u{672c}[[x]].md"]);

        assert_eq!(
            wikilinks_by_line("[a](a[[x]]b.md) [[y]] `[c](d` [[w]] )\n"),
            vec![vec!["y".to_string(), "w".to_string()], vec![]]
        );
    }

    #[test]
    fn scan_body_returns_no_links_when_line_has_no_wikilink_markers() {
        let body = "just `code` here, nothing to link";
        let (_, _, links, _) = scan_body(body, 1);
        assert!(links.is_empty());
    }

    // --- wrapped bullets ---------------------------------------------------

    fn observations_of(body: &str) -> Vec<Observation> {
        scan_body(body, 1).0
    }

    #[test]
    fn task_items_are_not_observations() {
        for line in [
            "- [ ] ship it\n",
            "- [x] ship it\n",
            "- [X] ship it\n",
            "- [~] ship it\n",
        ] {
            assert!(
                observations_of(line).is_empty(),
                "{line:?} read as an observation"
            );
        }
        // A real category still parses.
        assert_eq!(observations_of("- [fact] ship it\n")[0].category, "fact");
    }

    /// A converted document's footnotes often sit in a list: `[^1]` opens a
    /// footnote reference, never a category, as it never opens a reference
    /// definition (`linked.rs`).
    #[test]
    fn a_footnote_reference_is_not_a_category() {
        for line in ["- [^1] the source\n", "- [^note] more\n", "- [^] x\n"] {
            let (observations, relations, _, _) = scan_body(line, 1);
            assert!(observations.is_empty(), "{line:?} read as an observation");
            assert!(relations.is_empty(), "{line:?} read as a relation");
            assert_eq!(split_category(&line[2..]), None, "{line:?}");
        }
        // A caret later in the bracket is still a category.
        assert_eq!(observations_of("- [a^b] x\n")[0].category, "a^b");
    }

    #[test]
    fn a_directive_or_task_box_is_never_a_relation_type() {
        for body in [
            "- :status[Done]{green} [[Target]]\n",
            "- [x] [[Target]]\n",
            "- [ ] [[Target]]\n",
        ] {
            let (_, relations, links, _) = scan_body(body, 1);
            assert!(relations.is_empty(), "{body:?} read as a relation");
            assert_eq!(
                links.len(),
                1,
                "the wikilink stays a prose link in {body:?}"
            );
        }
    }

    #[test]
    fn an_aliased_wikilink_is_a_link_to_its_target() {
        let body = "- part_of [[overview|the overview]]\n\nSee [[ops:pumps/p-12|the pump]], [[|b]] and [[a|]].\n";
        let (_, relations, links, _) = scan_body(body, 1);
        assert_eq!(relations.len(), 1, "{relations:?}");
        assert_eq!(relations[0].target.target, "overview");
        assert_eq!(relations[0].target.alias.as_deref(), Some("the overview"));
        let targets: Vec<_> = links
            .iter()
            .map(|l| (l.target.domain.as_deref(), l.target.target.as_str()))
            .collect();
        // The relation target is not a prose link too, and `[[|b]]` is no link.
        assert_eq!(targets, vec![(Some("ops"), "pumps/p-12"), (None, "a")]);
    }

    #[test]
    fn a_relation_to_an_empty_target_is_no_relation() {
        let (_, relations, links, _) = scan_body("- part_of [[|b]]\n", 1);
        assert!(relations.is_empty(), "{relations:?}");
        assert!(links.is_empty(), "{links:?}");
    }

    #[test]
    fn the_parse_generation_moved_for_relative_links() {
        assert_eq!(PARSE_GENERATION, 10);
    }

    /// `index.md` and `log.md` are the files Crystalline keeps in every
    /// folder, never engrams: a link back to one is sound and no edge.
    #[test]
    fn a_relative_link_to_a_folders_index_or_log_is_no_link() {
        let body = "Back to [the folder](index.md), [up](../index.md), [log](log.md) \
                    and [p](p.md).\n- part_of [Index](index.md)\n";
        let (_, relations, links, _) = scan_body(body, 1);
        assert!(relations.is_empty(), "{relations:?}");
        let targets: Vec<&str> = links.iter().map(|l| l.target.target.as_str()).collect();
        assert_eq!(targets, vec!["p.md"]);
    }

    /// Only the exact names are reserved, as the walker reserves them: an
    /// engram a person named `Index.md` or `LOG.md` keeps its links.
    #[test]
    fn an_engram_named_index_in_another_case_keeps_its_links() {
        let body = "See [the index](Index.md), [[guides/LOG.md]] and [[ops:notes/Index.md]].\n";
        let (_, _, links, _) = scan_body(body, 1);
        let targets: Vec<&str> = links.iter().map(|l| l.target.target.as_str()).collect();
        assert_eq!(targets, vec!["guides/LOG.md", "notes/Index.md", "Index.md"]);
    }

    /// A wikilink spelled as the path of a folder's `index.md` or `log.md`
    /// names that file too, never an engram, prefixed or not.
    #[test]
    fn a_wikilink_to_a_folders_index_or_log_is_no_link() {
        let body = "See [[guides/index.md]], [[ops:log.md|the log]], [[Guides/index.md]] \
                    and [[p.md]].\n- part_of [[index.md]]\n";
        let (_, relations, links, _) = scan_body(body, 1);
        assert!(relations.is_empty(), "{relations:?}");
        let targets: Vec<&str> = links.iter().map(|l| l.target.target.as_str()).collect();
        assert_eq!(targets, vec!["p.md"]);
    }

    #[test]
    fn relative_md_links_are_untyped_links() {
        let (_, relations, links, _) = scan_body("Read [p](../pumps/p-12.md#top) first.\n", 1);
        assert!(relations.is_empty());
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].line, 1);
        assert_eq!(links[0].target.form, LinkForm::Relative);
        assert_eq!(links[0].target.target, "../pumps/p-12.md");
        assert_eq!(links[0].target.domain, None);

        // Angle brackets, percent-encoding, a title and an upper-case ending
        // all read the same target the way a reflection link is read.
        let (_, _, links, _) = scan_body(
            "[a](<my notes/A b.md>) and [b](my%20notes/A%20b.md \"Title\") and [c](./C.MD)\n",
            1,
        );
        let targets: Vec<&str> = links.iter().map(|l| l.target.target.as_str()).collect();
        assert_eq!(
            targets,
            ["my notes/A b.md", "./C.MD"],
            "one per line and target"
        );
        assert!(links.iter().all(|l| l.target.form == LinkForm::Relative));
    }

    #[test]
    fn a_unicode_space_in_a_relative_link_is_part_of_the_path() {
        let body = "- part_of [Doc](../\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md)\n\
                    See [B](b\u{a0}c.md \"B\").\n";
        let (_, relations, links, _) = scan_body(body, 1);
        assert_eq!(relations.len(), 1, "{relations:?}");
        assert_eq!(
            relations[0].target.target,
            "../\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md"
        );
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].target.target, "b\u{a0}c.md");
    }

    #[test]
    fn a_relation_bullet_takes_a_relative_link() {
        let (_, relations, links, _) = scan_body("- part_of [Overview](../overview.md)\n", 1);
        assert_eq!(relations.len(), 1, "{relations:?}");
        assert_eq!(relations[0].rel_type, "part_of");
        assert_eq!(relations[0].target.target, "../overview.md");
        assert_eq!(relations[0].target.form, LinkForm::Relative);
        assert!(
            links.is_empty(),
            "the relation's own link is not prose too: {links:?}"
        );

        // A quoted phrase works as it does for a wikilink.
        let (_, relations, _, _) = scan_body("- \"is part of\" [Overview](overview.md)\n", 1);
        assert_eq!(relations[0].rel_type, "is part of");

        // A bullet that is only a link stays a prose link.
        let (_, relations, links, _) = scan_body("- [Overview](../overview.md)\n", 1);
        assert!(relations.is_empty(), "{relations:?}");
        assert_eq!(links.len(), 1);

        // A wikilink relation reads exactly as before.
        let (_, relations, links, _) = scan_body("- part_of [[Overview]]\n", 1);
        assert_eq!(relations[0].target.form, LinkForm::Wiki);
        assert!(links.is_empty());
    }

    /// A converted document's `- See [Three](chapter-3.md)` is prose, not a
    /// relation of type `See`: the relative form takes a lowercase type or a
    /// quoted phrase. The wikilink form keeps its free type.
    #[test]
    fn a_relative_relation_takes_a_lowercase_type_or_a_quoted_phrase() {
        let (_, relations, links, _) = scan_body("- See [Three](chapter-3.md#chapter-3)\n", 1);
        assert!(relations.is_empty(), "{relations:?}");
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(links[0].target.target, "chapter-3.md");
        assert_eq!(links[0].target.form, LinkForm::Relative);

        for (body, rel_type) in [
            ("- part_of [Doc](../doc.md)\n", "part_of"),
            ("- follows [Prev](prev.md)\n", "follows"),
            ("- \"relates to\" [X](x.md)\n", "relates to"),
            ("- v2-of [X](x.md)\n", "v2-of"),
        ] {
            let (_, relations, links, _) = scan_body(body, 1);
            assert_eq!(relations.len(), 1, "{body:?}: {relations:?}");
            assert_eq!(relations[0].rel_type, rel_type);
            assert!(links.is_empty(), "{body:?}: {links:?}");
        }

        for body in [
            "- Part_of [Doc](doc.md)\n",
            "- partOf [Doc](doc.md)\n",
            "- 2nd [Doc](doc.md)\n",
            "- _x [Doc](doc.md)\n",
            "- see: [Doc](doc.md)\n",
            "- \"open [Doc](doc.md)\n",
        ] {
            let (_, relations, links, _) = scan_body(body, 1);
            assert!(relations.is_empty(), "{body:?}: {relations:?}");
            assert_eq!(links.len(), 1, "{body:?} keeps its prose link");
        }

        // The wikilink form reads any token, as it always did.
        let (_, relations, _, _) = scan_body("- See [[Three]]\n", 1);
        assert_eq!(relations.len(), 1, "{relations:?}");
        assert_eq!(relations[0].rel_type, "See");
    }

    #[test]
    fn links_in_code_and_non_md_targets_are_not_edges() {
        let body = "Code `[p](p.md)` here.\n\
                    \n\
                    ```\n\
                    [fenced](fenced.md)\n\
                    ```\n\
                    \n\
                    [img](pic.png) [web](https://example.test/a.md) [abs](/root/a.md)\n\
                    [frag](#a.md) [mail](mailto:a@example.test.md) [dir](notes/)\n\
                    ![shot](diagram.md) [addr](crystalline://ops/a.md)\n";
        let (_, relations, links, _) = scan_body(body, 1);
        assert!(relations.is_empty(), "{relations:?}");
        assert!(links.is_empty(), "{links:?}");
    }

    #[test]
    fn a_wikilink_serializes_without_a_form() {
        let (_, _, links, _) = scan_body("[[A]] and [a](a.md)\n", 1);
        let json = serde_json::to_value(&links).unwrap();
        assert_eq!(
            json[0]["target"],
            serde_json::json!({ "domain": null, "target": "A" })
        );
        assert_eq!(json[1]["target"]["form"], "relative");
    }

    #[test]
    fn a_bullet_indented_over_two_lines_is_one_observation() {
        let obs = observations_of(
            "- [fact] targets are all repo-kind, which is why\n  they run with the default runner\n",
        );
        assert_eq!(obs.len(), 1);
        assert_eq!(
            obs[0].content,
            "targets are all repo-kind, which is why they run with the default runner"
        );
        assert_eq!(obs[0].line, 1);
        assert_eq!(obs[0].end_line, 2);
    }

    #[test]
    fn a_bullet_wrapped_over_three_lines_joins_every_line() {
        let body = "intro\n\n- [decision] one\n    two\n  three\nafter a blank\n";
        let obs = observations_of(body);
        // `after a blank` is a lazy continuation too: no blank line comes first.
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "one two three after a blank");
        assert_eq!(obs[0].line, 3);
        assert_eq!(obs[0].end_line, 6);
    }

    #[test]
    fn a_lazy_continuation_line_belongs_to_the_bullet() {
        let obs = observations_of("- [fact] the first half\nand the second half\n");
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "the first half and the second half");
        assert_eq!((obs[0].line, obs[0].end_line), (1, 2));
    }

    #[test]
    fn a_wrapped_category_bullet_reads_a_tag_on_its_continuation_line() {
        let obs = observations_of(
            "- [risk] the cache can serve a stale row\n  after a failover #cache #ops (seen in 0.21)\n",
        );
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].category, "risk");
        assert_eq!(
            obs[0].content,
            "the cache can serve a stale row after a failover"
        );
        assert_eq!(obs[0].tags, vec!["cache", "ops"]);
        assert_eq!(obs[0].context.as_deref(), Some("seen in 0.21"));
    }

    #[test]
    fn a_hashtag_on_the_first_line_of_a_wrapped_bullet_is_mid_text() {
        // Joined, `#ops` no longer ends the bullet, so it is text, not a tag:
        // only a tag run at the end of the whole bullet is the bullet's tags.
        let obs = observations_of("- [fact] restart the node #ops\n  before the upgrade\n");
        assert_eq!(obs[0].content, "restart the node #ops before the upgrade");
        assert!(obs[0].tags.is_empty());
    }

    #[test]
    fn a_blank_line_ends_the_bullet_and_the_paragraph_is_not_joined() {
        let obs = observations_of("- [fact] one line only\n\nA paragraph after it.\n");
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "one line only");
        assert_eq!(obs[0].end_line, 1);
    }

    #[test]
    fn a_nested_list_item_is_not_joined_into_its_parent() {
        let body = "- [fact] the parent\n  wraps here\n  - a nested item\n    that wraps\n- [fact] the sibling\n";
        let obs = observations_of(body);
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].content, "the parent wraps here");
        assert_eq!(obs[0].end_line, 2);
        assert_eq!(obs[1].content, "the sibling");
        assert_eq!(obs[1].line, 5);
    }

    #[test]
    fn the_next_bullet_heading_quote_or_rule_ends_a_bullet() {
        let body = "- [a] one\n* star item\n- [b] two\n## Heading\n- [c] three\n> quoted\n- [d] four\n---\n- [e] five\n1. ordered\n- [f] six\n2024. was the year\n";
        let obs = observations_of(body);
        let contents: Vec<&str> = obs.iter().map(|o| o.content.as_str()).collect();
        assert_eq!(
            contents,
            vec![
                "one",
                "two",
                "three",
                "four",
                "five",
                "six 2024. was the year"
            ]
        );
    }

    #[test]
    fn a_code_fence_inside_a_bullet_ends_the_observation_text() {
        let body = "- [howto] run it like this\n  ```sh\n  crystalline sync\n  ```\n- [howto] or deeper\n    ```\n    code\n    ```\n";
        let obs = observations_of(body);
        assert_eq!(obs.len(), 2);
        assert_eq!(obs[0].content, "run it like this");
        assert_eq!(obs[0].end_line, 1);
        assert_eq!(obs[1].content, "or deeper");
        assert_eq!(obs[1].end_line, 5);
    }

    #[test]
    fn a_wrapped_relation_is_one_relation_and_its_target_is_not_a_prose_link() {
        let body = "- relates_to\n  [[Target Engram]]\n- depends_on [[Other]] for the\n  reasons in [[Notes]]\n";
        let (_, relations, links, _) = scan_body(body, 1);
        assert_eq!(relations.len(), 2);
        assert_eq!(relations[0].rel_type, "relates_to");
        assert_eq!(relations[0].target.target, "Target Engram");
        assert_eq!((relations[0].line, relations[0].end_line), (1, 2));
        assert_eq!(relations[1].rel_type, "depends_on");
        assert_eq!(relations[1].target.target, "Other");
        assert_eq!(relations[1].end_line, 4);
        let prose: Vec<(&str, usize)> = links
            .iter()
            .map(|l| (l.target.target.as_str(), l.line))
            .collect();
        assert_eq!(prose, vec![("Notes", 4)]);
    }

    #[test]
    fn a_crlf_wrapped_bullet_is_joined_without_carriage_returns() {
        let obs = observations_of("- [fact] first half\r\n  second half #tag\r\n\r\nprose\r\n");
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "first half second half");
        assert_eq!(obs[0].tags, vec!["tag"]);
        assert!(!obs[0].content.contains('\r'));
        assert_eq!((obs[0].line, obs[0].end_line), (1, 2));
    }

    #[test]
    fn a_line_ending_inside_an_open_link_or_category_is_not_joined() {
        // Read per line, as the rename and the near-miss check read it: no
        // relation to `Long Name` and no observation of category `cat egory`.
        let (obs, relations, links, _) =
            scan_body("- relates_to [[Long\n  Name]]\n- [cat\n  egory] text\n", 1);
        assert!(relations.is_empty(), "{relations:?}");
        assert!(links.is_empty(), "{links:?}");
        assert!(obs.is_empty(), "{obs:?}");
        // A link closed on its own line still lets the bullet wrap, and one
        // inside inline code is not a link at all.
        let obs = observations_of(
            "- [fact] see [[Done]] and\n  more\n- [fact] code `[[x` here\n  wraps\n",
        );
        let contents: Vec<&str> = obs.iter().map(|o| o.content.as_str()).collect();
        assert_eq!(
            contents,
            vec!["see [[Done]] and more", "code `[[x` here wraps"]
        );
    }

    #[test]
    fn a_bullet_that_starts_with_a_link_is_not_a_category() {
        let obs = observations_of("- [label](https://x) - text\n");
        assert!(obs.is_empty(), "{obs:?}");
        let obs = observations_of("- [label][ref] text\n- [fact](x)\n");
        assert!(obs.is_empty(), "{obs:?}");
        let obs = observations_of("- [fact] x\n- [fact]y\n");
        let got: Vec<(&str, &str)> = obs
            .iter()
            .map(|o| (o.category.as_str(), o.content.as_str()))
            .collect();
        assert_eq!(got, vec![("fact", "x"), ("fact", "y")]);
    }

    #[test]
    fn a_wrapped_bullet_that_starts_with_a_link_is_not_a_category() {
        let (obs, relations, _, _) = scan_body(
            "- [label](https://x) - text that\n  wraps here\n- [fact] next\n",
            1,
        );
        assert!(relations.is_empty(), "{relations:?}");
        let contents: Vec<&str> = obs.iter().map(|o| o.content.as_str()).collect();
        assert_eq!(contents, vec!["next"]);
    }

    #[test]
    fn an_html_block_or_a_table_row_ends_a_bullet() {
        let obs = observations_of(
            "- [fact] one\n<!-- note -->\n- [fact] two\n| a | b |\n|---|---|\n- [fact] three\n  <b>inline</b> is html too\n",
        );
        let contents: Vec<&str> = obs.iter().map(|o| o.content.as_str()).collect();
        assert_eq!(contents, vec!["one", "two", "three"]);
    }

    #[test]
    fn a_wrapped_bullet_reports_its_first_line_in_a_whole_file() {
        let source =
            "---\ntype: engram\ntitle: T\n---\n# T\n\n- [fact] one\n  continued\n- [fact] two\n";
        let engram = parse_engram(source).unwrap();
        let lines: Vec<(usize, usize)> = engram
            .observations
            .iter()
            .map(|o| (o.line, o.end_line))
            .collect();
        assert_eq!(lines, vec![(7, 8), (9, 9)]);
        assert_eq!(engram.observations[0].content, "one continued");
    }

    // The Fluid engram page shows this field on its meta line, so a block
    // scalar, a plain continuation and a trailing comment must all come out
    // as the one-line text a reader sees.
    #[test]
    fn description_reads_block_scalars_continuations_and_comments() {
        let engram = |description: &str| {
            let source =
                format!("---\ntype: guide\ntitle: T\ndescription: {description}\n---\n\n# T\n");
            parse_engram(&source).unwrap().frontmatter.description
        };
        assert_eq!(
            engram(">-\n  Why we deploy\n  at night").as_deref(),
            Some("Why we deploy at night")
        );
        assert_eq!(engram("|-\n  one\n  two").as_deref(), Some("one\ntwo"));
        assert_eq!(
            engram("Plain text\n  that continues").as_deref(),
            Some("Plain text that continues")
        );
        assert_eq!(
            engram("\"quoted\n  across lines\"").as_deref(),
            Some("quoted across lines")
        );
        assert_eq!(engram("kept # a comment").as_deref(), Some("kept"));
    }
}
