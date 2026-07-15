//! Emitting an Engram back to markdown.
//!
//! [`emit_engram`] performs a full deterministic emission: known frontmatter
//! fields in a fixed canonical order, then schema fields, then unknown keys in
//! their original order, followed by the body verbatim. For well-formed
//! canonical files this is byte-identical to the source.
//!
//! The surgical editors string-edit the original source without a full
//! re-emission, so non-canonical files keep every untouched byte. Sections are
//! addressed by heading path such as `## API > ### Auth`.

use chrono::{DateTime, FixedOffset};
use serde_yaml_ng::{Mapping, Value};

use crate::engram::{Engram, Frontmatter, SchemaDef};
use crate::parse::{locate, parse_heading};

/// An error from a section-addressed editor.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    /// No heading matched the requested path.
    #[error("no section found for heading path: {path}")]
    SectionNotFound {
        /// The requested heading path.
        path: String,
    },
}

/// Emit an Engram to markdown deterministically.
pub fn emit_engram(engram: &Engram) -> String {
    let map = frontmatter_mapping(&engram.frontmatter);
    if map.is_empty() {
        return engram.body.clone();
    }
    let yaml = serde_yaml_ng::to_string(&Value::Mapping(map)).unwrap_or_default();
    format!("---\n{}---\n{}", yaml, engram.body)
}

fn frontmatter_mapping(fm: &Frontmatter) -> Mapping {
    let mut map = Mapping::new();
    let mut put = |k: &str, v: Value| {
        map.insert(Value::String(k.to_string()), v);
    };

    if !fm.engram_type.is_empty() {
        put("type", Value::String(fm.engram_type.clone()));
    }
    if !fm.title.is_empty() {
        put("title", Value::String(fm.title.clone()));
    }
    if let Some(v) = &fm.permalink {
        put("permalink", Value::String(v.clone()));
    }
    if let Some(v) = &fm.description {
        put("description", Value::String(v.clone()));
    }
    if !fm.tags.is_empty() {
        put(
            "tags",
            Value::Sequence(fm.tags.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(v) = &fm.status {
        put("status", Value::String(v.clone()));
    }
    if let Some(d) = fm.recorded_at {
        put(
            "recorded_at",
            Value::String(d.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(d) = fm.valid_from {
        put(
            "valid_from",
            Value::String(d.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(d) = fm.valid_to {
        put("valid_to", Value::String(d.format("%Y-%m-%d").to_string()));
    }
    if let Some(d) = fm.source_date {
        put(
            "source_date",
            Value::String(d.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(d) = fm.last_verified {
        put(
            "last_verified",
            Value::String(d.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(d) = fm.review_after {
        put(
            "review_after",
            Value::String(d.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(v) = &fm.temporal_confidence {
        put("temporal_confidence", Value::String(v.clone()));
    }
    if let Some(v) = &fm.resource {
        put("resource", Value::String(v.clone()));
    }
    if let Some(ts) = fm.timestamp {
        put("timestamp", Value::String(ts.to_rfc3339()));
    }
    if let Some(schema) = &fm.schema_def {
        emit_schema_fields(&mut map, schema);
    }
    for (k, v) in &fm.extra {
        map.insert(Value::String(k.clone()), v.to_backend());
    }
    map
}

fn emit_schema_fields(map: &mut Mapping, schema: &SchemaDef) {
    if let Some(entity) = &schema.entity {
        map.insert(
            Value::String("entity".into()),
            Value::String(entity.clone()),
        );
    }
    if let Some(version) = schema.version {
        map.insert(
            Value::String("version".into()),
            Value::Number(version.into()),
        );
    }
    if !schema.schema.is_empty() {
        let mut inner = Mapping::new();
        for (k, v) in &schema.schema {
            inner.insert(Value::String(k.clone()), v.to_backend());
        }
        map.insert(Value::String("schema".into()), Value::Mapping(inner));
    }
    if !schema.settings.is_empty() {
        let mut inner = Mapping::new();
        for (k, v) in &schema.settings {
            inner.insert(Value::String(k.clone()), v.to_backend());
        }
        map.insert(Value::String("settings".into()), Value::Mapping(inner));
    }
}

// --- surgical editors --------------------------------------------------------

/// Set or replace a single scalar frontmatter field in the original source,
/// leaving everything else untouched. Creates a frontmatter block if absent.
pub fn set_frontmatter_field(source: &str, key: &str, value: &str) -> String {
    let new_line = format_scalar_line(key, value);
    let (has_fm, fm_span, _body_start) = locate(source);

    if !has_fm {
        // No frontmatter block yet: create a minimal one.
        return format!("---\n{new_line}\n---\n{source}");
    }

    let raw = &source[fm_span.clone()];
    let mut new_raw = String::with_capacity(raw.len() + new_line.len());
    let mut replaced = false;
    for line in raw.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        if !replaced && line_sets_key(content, key) {
            new_raw.push_str(&new_line);
            if line.ends_with('\n') {
                new_raw.push('\n');
            }
            replaced = true;
        } else {
            new_raw.push_str(line);
        }
    }
    if !replaced {
        if !new_raw.is_empty() && !new_raw.ends_with('\n') {
            new_raw.push('\n');
        }
        new_raw.push_str(&new_line);
        new_raw.push('\n');
    }
    format!(
        "{}{}{}",
        &source[..fm_span.start],
        new_raw,
        &source[fm_span.end..]
    )
}

/// Remove a single frontmatter field from the original source, leaving every
/// other byte untouched. A no-op returning the source unchanged when the key
/// or the frontmatter block is absent.
///
/// Only safe for a single-line scalar field. That is guaranteed for a date
/// field that parsed into a `NaiveDate` and for an explicit null `key:` line;
/// it must not be used on a key whose value spans several lines (a block
/// sequence or a nested mapping), which would orphan the trailing lines.
pub fn remove_frontmatter_field(source: &str, key: &str) -> String {
    let (has_fm, fm_span, _body_start) = locate(source);
    if !has_fm {
        return source.to_string();
    }

    let raw = &source[fm_span.clone()];
    let mut new_raw = String::with_capacity(raw.len());
    let mut removed = false;
    for line in raw.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        if !removed && line_sets_key(content, key) {
            removed = true;
        } else {
            new_raw.push_str(line);
        }
    }
    if !removed {
        return source.to_string();
    }
    format!(
        "{}{}{}",
        &source[..fm_span.start],
        new_raw,
        &source[fm_span.end..]
    )
}

/// Set the `timestamp` field to `now` (RFC 3339) in the original source.
pub fn touch_timestamp(source: &str, now: DateTime<FixedOffset>) -> String {
    set_frontmatter_field(source, "timestamp", &now.to_rfc3339())
}

fn format_scalar_line(key: &str, value: &str) -> String {
    let mut m = Mapping::new();
    m.insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
    serde_yaml_ng::to_string(&Value::Mapping(m))
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

fn line_sets_key(line: &str, key: &str) -> bool {
    match line.strip_prefix(key) {
        Some(rest) => rest.starts_with(':'),
        None => false,
    }
}

// --- section editing ---------------------------------------------------------

struct HeadingSpan {
    level: u8,
    text: String,
    line_start: usize,
    line_end: usize,
}

fn heading_spans(source: &str) -> Vec<HeadingSpan> {
    let (_, _, body_start) = locate(source);
    let mut spans = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut offset = body_start;
    for raw in source[body_start..].split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw).trim_end_matches('\r');
        let line_start = offset;
        let line_end = offset + raw.len();
        offset = line_end;

        match fence {
            None => {
                if let Some((c, n)) = fence_open(line) {
                    fence = Some((c, n));
                    continue;
                }
                if let Some((level, text)) = parse_heading(line) {
                    spans.push(HeadingSpan {
                        level,
                        text,
                        line_start,
                        line_end,
                    });
                }
            }
            Some((fc, fcount)) => {
                if let Some((c, n)) = fence_open(line)
                    && c == fc
                    && n >= fcount
                {
                    let after = &line.trim_start()[n..];
                    if after.trim().is_empty() {
                        fence = None;
                    }
                }
            }
        }
    }
    spans
}

fn fence_open(line: &str) -> Option<(char, usize)> {
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
    Some((first, count))
}

fn parse_segment(seg: &str) -> (Option<u8>, String) {
    let seg = seg.trim();
    let hashes = seg.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        (Some(hashes as u8), seg[hashes..].trim().to_string())
    } else {
        (None, seg.to_string())
    }
}

fn heading_matches(h: &HeadingSpan, level: Option<u8>, text: &str) -> bool {
    if let Some(l) = level
        && h.level != l
    {
        return false;
    }
    h.text == text || h.text.eq_ignore_ascii_case(text)
}

fn section_end_index(headings: &[HeadingSpan], p: usize) -> usize {
    let level = headings[p].level;
    for (i, h) in headings.iter().enumerate().skip(p + 1) {
        if h.level <= level {
            return i;
        }
    }
    headings.len()
}

fn resolve_path(headings: &[HeadingSpan], path: &str) -> Option<usize> {
    let segments: Vec<(Option<u8>, String)> = path.split('>').map(parse_segment).collect();
    if segments.is_empty() {
        return None;
    }
    let mut search_start = 0usize;
    let mut search_end = headings.len();
    let mut matched = None;
    for (level, text) in &segments {
        let mut found = None;
        for (i, h) in headings
            .iter()
            .enumerate()
            .take(search_end)
            .skip(search_start)
        {
            if heading_matches(h, *level, text) {
                found = Some(i);
                break;
            }
        }
        let fi = found?;
        matched = Some(fi);
        search_start = fi + 1;
        search_end = section_end_index(headings, fi);
    }
    matched
}

/// Replace the content under a section addressed by heading path. By default
/// deeper subsections are preserved; pass `include_subsections` to replace
/// them too.
pub fn replace_section(
    source: &str,
    path: &str,
    new_content: &str,
    include_subsections: bool,
) -> Result<String, EditError> {
    let headings = heading_spans(source);
    let p = resolve_path(&headings, path).ok_or_else(|| EditError::SectionNotFound {
        path: path.to_string(),
    })?;
    let own_start = headings[p].line_end;
    let sec_end_idx = section_end_index(&headings, p);
    let section_end = headings
        .get(sec_end_idx)
        .map(|h| h.line_start)
        .unwrap_or(source.len());
    let boundary = if include_subsections {
        section_end
    } else if p + 1 < sec_end_idx {
        headings[p + 1].line_start
    } else {
        section_end
    };

    let body = new_content.trim_matches('\n');
    let region = if body.is_empty() {
        "\n".to_string()
    } else if boundary < source.len() {
        format!("\n{body}\n\n")
    } else {
        format!("\n{body}\n")
    };
    Ok(format!(
        "{}{}{}",
        &source[..own_start],
        region,
        &source[boundary..]
    ))
}

/// Insert content immediately before a section's heading line.
pub fn insert_before_section(source: &str, path: &str, content: &str) -> Result<String, EditError> {
    let headings = heading_spans(source);
    let p = resolve_path(&headings, path).ok_or_else(|| EditError::SectionNotFound {
        path: path.to_string(),
    })?;
    let at = headings[p].line_start;
    let block = format!("{}\n\n", content.trim_matches('\n'));
    Ok(format!("{}{}{}", &source[..at], block, &source[at..]))
}

/// Insert content immediately after a section's heading line.
pub fn insert_after_section(source: &str, path: &str, content: &str) -> Result<String, EditError> {
    let headings = heading_spans(source);
    let p = resolve_path(&headings, path).ok_or_else(|| EditError::SectionNotFound {
        path: path.to_string(),
    })?;
    let at = headings[p].line_end;
    let block = format!("\n{}\n", content.trim_matches('\n'));
    Ok(format!("{}{}{}", &source[..at], block, &source[at..]))
}

/// Append content to the end of the body.
pub fn append_body(source: &str, content: &str) -> String {
    let mut s = source.to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push('\n');
    s.push_str(content.trim_matches('\n'));
    s.push('\n');
    s
}

/// Prepend content to the start of the body, after any frontmatter.
pub fn prepend_body(source: &str, content: &str) -> String {
    let (_, _, body_start) = locate(source);
    let block = format!("{}\n\n", content.trim_matches('\n'));
    format!(
        "{}{}{}",
        &source[..body_start],
        block,
        &source[body_start..]
    )
}
