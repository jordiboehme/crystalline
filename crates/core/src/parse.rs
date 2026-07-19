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
    Engram, Frontmatter, Heading, LinkTarget, Observation, Relation, SchemaDef, WikiLink,
};
use crate::yaml::YamlValue;

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
            "last_verified" => set_date(&mut fm.last_verified, &mut extra, &key, value),
            "review_after" => set_date(&mut fm.review_after, &mut extra, &key, value),
            "timestamp" => set_timestamp(&mut fm.timestamp, &mut extra, &key, value),
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
    let mut fence: Option<(char, usize)> = None;
    for (i, raw) in body.split('\n').enumerate() {
        let text = raw.trim_end_matches('\r');
        let line_no = body_line_start + i;
        match fence {
            None => {
                if let Some((c, n, _)) = fence_marker(text) {
                    fence = Some((c, n));
                    out.push(BodyLine {
                        line_no,
                        text,
                        in_fence: true,
                    });
                } else {
                    out.push(BodyLine {
                        line_no,
                        text,
                        in_fence: false,
                    });
                }
            }
            Some((fc, fcount)) => {
                let mut closes = false;
                if let Some((c, n, _)) = fence_marker(text)
                    && c == fc
                    && n >= fcount
                {
                    let after = &text.trim_start()[n..];
                    if after.trim().is_empty() {
                        closes = true;
                    }
                }
                out.push(BodyLine {
                    line_no,
                    text,
                    in_fence: true,
                });
                if closes {
                    fence = None;
                }
            }
        }
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

    for bl in body_lines(body, body_line_start) {
        if bl.in_fence {
            continue;
        }
        let line = bl.text;
        let line_no = bl.line_no;

        if let Some((level, text)) = parse_heading(line) {
            headings.push(Heading {
                line: line_no,
                level,
                text,
            });
            continue;
        }

        // Top-level bullets (zero indent) can be observations or relations.
        let mut relation_target: Option<LinkTarget> = None;
        if let Some(content) = top_level_bullet(line) {
            if let Some((category, rest)) = parse_observation_head(content) {
                let (obs_content, tags, context) = split_observation(&rest);
                observations.push(Observation {
                    line: line_no,
                    category,
                    content: obs_content,
                    tags,
                    context,
                });
            } else if let Some((rel_type, target)) = parse_relation(content) {
                relation_target = Some(target.clone());
                relations.push(Relation {
                    line: line_no,
                    rel_type,
                    target,
                });
            }
        }

        // Wikilinks anywhere on the line, excluding a relation target and
        // deduplicated per line. Nothing to find on a line without "[[", and
        // masking is only needed when a backtick could hide one.
        if line.contains("[[") {
            let masked: Cow<'_, str> = if line.contains('`') {
                Cow::Owned(mask_inline_code(line))
            } else {
                Cow::Borrowed(line)
            };
            let mut seen: Vec<LinkTarget> = Vec::new();
            let mut excluded = relation_target.is_none();
            for inner in find_wikilinks(&masked) {
                let target = LinkTarget::parse(&inner);
                if !excluded
                    && let Some(rt) = &relation_target
                    && &target == rt
                {
                    excluded = true;
                    continue;
                }
                if !seen.contains(&target) {
                    seen.push(target.clone());
                    links.push(WikiLink {
                        line: line_no,
                        target,
                    });
                }
            }
        }
    }

    (observations, relations, links, headings)
}

/// Parse an ATX heading, returning `(level, text)`.
pub(crate) fn parse_heading(line: &str) -> Option<(u8, String)> {
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

fn fence_marker(line: &str) -> Option<(char, usize, usize)> {
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

fn top_level_bullet(line: &str) -> Option<&str> {
    // Zero indent only; nested or indented bullets are not observations or
    // relations.
    line.strip_prefix("- ")
}

fn parse_observation_head(content: &str) -> Option<(String, String)> {
    if !content.starts_with('[') {
        return None;
    }
    let rest = &content[1..];
    let close = rest.find(']')?;
    let category = &rest[..close];
    if category.is_empty() || category.contains('[') {
        return None;
    }
    let body = rest[close + 1..].trim_start();
    Some((category.trim().to_string(), body.to_string()))
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

fn parse_relation(content: &str) -> Option<(String, LinkTarget)> {
    let open = content.find("[[")?;
    let after = &content[open + 2..];
    let close = after.find("]]")?;
    let inner = &after[..close];
    let before = content[..open].trim();

    let rel_type = if before.len() >= 2 && before.starts_with('"') && before.ends_with('"') {
        before[1..before.len() - 1].to_string()
    } else if !before.is_empty() && !before.contains(char::is_whitespace) {
        before.to_string()
    } else {
        return None;
    };
    Some((rel_type, LinkTarget::parse(inner)))
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

    // Pins the current mask_inline_code/scan_body behavior before the
    // single-buffer and contains-guard refactor so the refactor cannot
    // change observable output.

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

    #[test]
    fn scan_body_returns_no_links_when_line_has_no_wikilink_markers() {
        let body = "just `code` here, nothing to link";
        let (_, _, links, _) = scan_body(body, 1);
        assert!(links.is_empty());
    }
}
