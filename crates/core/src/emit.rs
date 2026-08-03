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

use chrono::{DateTime, FixedOffset, NaiveDate};
use serde_yaml_ng::{Mapping, Value};

use crate::engram::{Engram, Frontmatter, Generated, SchemaDef, Verified};
use crate::parse::{locate, parse_heading};

/// The stand-in scalar the `generated` key carries through YAML serialization,
/// swapped for the flow mapping afterwards. The YAML crate only emits block
/// mappings, and `generated` must stay on one line so the surgical editors can
/// replace it as a single line; the token is deliberately plain ASCII so
/// serialization never wraps it in quotes.
const GENERATED_PLACEHOLDER: &str = "crystalline-generated-placeholder";

/// The stand-in scalar the `verified` key carries through YAML serialization,
/// swapped for the flow form afterwards for the same reason as
/// [`GENERATED_PLACEHOLDER`]: every entry stays on one line.
const VERIFIED_PLACEHOLDER: &str = "crystalline-verified-placeholder";

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
    let yaml = match &engram.frontmatter.generated {
        Some(g) => yaml.replacen(
            &format!("generated: {GENERATED_PLACEHOLDER}"),
            &generated_flow(g),
            1,
        ),
        None => yaml,
    };
    let yaml = if engram.frontmatter.verified.is_empty() {
        yaml
    } else {
        yaml.replacen(
            &format!("verified: {VERIFIED_PLACEHOLDER}"),
            &verified_block(&engram.frontmatter.verified),
            1,
        )
    };
    format!("---\n{}---\n{}", yaml, engram.body)
}

/// Render a whole `generated` frontmatter line as the OKF flow mapping, key
/// included, so it occupies exactly one line. Values are quoted only when a
/// plain scalar would be ambiguous inside a flow mapping, so the common actor
/// and RFC 3339 forms read exactly like the spec's examples.
fn generated_flow(g: &Generated) -> String {
    let mut out = format!("generated: {{ by: {}", flow_scalar(&g.by));
    if let Some(at) = g.at {
        out.push_str(&format!(", at: {}", flow_scalar(&at.to_rfc3339())));
    }
    out.push_str(" }");
    out
}

/// Render a whole `verified` frontmatter block, key included. A single entry
/// stays on the key's own line, exactly like `generated`; several entries emit
/// as a block sequence with one flow mapping per line, so the OKF list form
/// stays as readable and as line-editable as the single form.
fn verified_block(entries: &[Verified]) -> String {
    if let [only] = entries {
        return format!("verified: {}", verified_flow(only));
    }
    let mut out = String::from("verified:");
    for entry in entries {
        out.push_str(&format!("\n- {}", verified_flow(entry)));
    }
    out
}

/// Render one `verified` entry as an OKF flow mapping, without the key.
fn verified_flow(v: &Verified) -> String {
    let mut out = format!("{{ by: {}", flow_scalar(&v.by));
    if let Some(at) = v.at {
        out.push_str(&format!(", at: {}", flow_scalar(&at.to_rfc3339())));
    }
    out.push_str(" }");
    out
}

/// Quote a scalar for a YAML flow mapping unless it is unambiguously bare. A
/// bare value is safe when it is non-empty, holds only characters that carry
/// no meaning in flow context (so no whitespace, quote or flow punctuation)
/// and neither opens nor closes with a character a parser would read as an
/// indicator.
fn flow_scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | '+' | ':' | '@')
        })
        && !value.starts_with(':')
        && !value.starts_with('-')
        && !value.ends_with(':');
    if plain {
        return value.to_string();
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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
    // `verified` takes the canonical slot the legacy `last_verified` held and
    // `stale_after` the one `review_after` held, so a file still carrying only
    // a legacy key keeps emitting it in exactly that place and round-trips byte
    // for byte until an edit migrates it.
    if !fm.verified.is_empty() {
        put("verified", Value::String(VERIFIED_PLACEHOLDER.to_string()));
    }
    if let Some(d) = fm.last_verified {
        put(
            "last_verified",
            Value::String(d.format("%Y-%m-%d").to_string()),
        );
    }
    if let Some(d) = fm.stale_after {
        put(
            "stale_after",
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
    // `generated` takes the canonical slot the legacy `timestamp` held, and a
    // file that still carries only `timestamp` keeps emitting it in exactly
    // that place, so a legacy engram round-trips byte for byte until an edit
    // migrates it.
    if fm.generated.is_some() {
        put(
            "generated",
            Value::String(GENERATED_PLACEHOLDER.to_string()),
        );
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
    set_frontmatter_line(source, &[key], format_scalar_line(key, value))
}

/// Replace the first frontmatter line that sets any of `keys` with `new_line`,
/// appending it when none of them is present. Creates a frontmatter block when
/// the source has none. The keys are tried in order, so a caller can name a
/// canonical key first and a legacy spelling second and have the legacy line
/// rewritten in place.
fn set_frontmatter_line(source: &str, keys: &[&str], new_line: String) -> String {
    let (has_fm, fm_span, _body_start) = locate(source);

    if !has_fm {
        // No frontmatter block yet: create a minimal one.
        return format!("---\n{new_line}\n---\n{source}");
    }

    let raw = &source[fm_span.clone()];
    // Which key actually appears decides which line is rewritten, so a file
    // carrying both the canonical and the legacy spelling has the canonical one
    // updated whatever order they sit in.
    let target = keys.iter().copied().find(|k| {
        raw.split_inclusive('\n')
            .any(|l| line_sets_key(l.strip_suffix('\n').unwrap_or(l), k))
    });
    let mut new_raw = String::with_capacity(raw.len() + new_line.len());
    let mut replaced = false;
    for line in raw.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        if !replaced && target.is_some_and(|k| line_sets_key(content, k)) {
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

/// Set or replace a single numeric frontmatter field in the original source,
/// leaving everything else untouched. Creates a frontmatter block if absent.
///
/// The counterpart of [`set_frontmatter_field`] for a key whose value must stay
/// a YAML number rather than a quoted string, `salience` being the one the
/// index reads that way. A whole value emits without a fractional part, so a
/// salience of 7 reads `salience: 7`.
pub fn set_frontmatter_number(source: &str, key: &str, value: f64) -> String {
    let rendered = if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    };
    set_frontmatter_line(source, &[key], format!("{key}: {rendered}"))
}

/// Record a trust history in the original source: replace the `verified` value
/// with `entries`, leaving every other byte untouched. An empty `entries` is a
/// no-op, since a verification is never cleared through this path.
///
/// Unlike [`set_frontmatter_field`] this replaces a value that may span several
/// lines: one entry emits as a single flow mapping on the key's own line, and
/// several emit as a block sequence, so the existing block sequence has to go
/// with the key line rather than be orphaned under the new value.
pub fn set_verified(source: &str, entries: &[Verified]) -> String {
    if entries.is_empty() {
        return source.to_string();
    }
    set_frontmatter_block(source, "verified", verified_block(entries))
}

/// Replace the frontmatter value of `key` with `new_block`, which may span
/// several lines and carries the key itself. Continuation lines belonging to
/// the old value - a block sequence item or an indented nested mapping - are
/// removed with the key line. Appends when the key is absent and creates a
/// frontmatter block when the source has none.
fn set_frontmatter_block(source: &str, key: &str, new_block: String) -> String {
    let (has_fm, fm_span, _body_start) = locate(source);
    if !has_fm {
        return format!("---\n{new_block}\n---\n{source}");
    }

    let raw = &source[fm_span.clone()];
    let mut new_raw = String::with_capacity(raw.len() + new_block.len());
    // 0: the key has not been seen; 1: it was just replaced and continuation
    // lines are being dropped; 2: the old value is fully behind us.
    let mut phase = 0u8;
    for line in raw.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        match phase {
            0 if line_sets_key(content, key) => {
                new_raw.push_str(&new_block);
                new_raw.push('\n');
                phase = 1;
            }
            1 if is_value_continuation(content) => {}
            1 => {
                phase = 2;
                new_raw.push_str(line);
            }
            _ => new_raw.push_str(line),
        }
    }
    if phase == 0 {
        if !new_raw.is_empty() && !new_raw.ends_with('\n') {
            new_raw.push('\n');
        }
        new_raw.push_str(&new_block);
        new_raw.push('\n');
    }
    format!(
        "{}{}{}",
        &source[..fm_span.start],
        new_raw,
        &source[fm_span.end..]
    )
}

/// True when a frontmatter line continues the value of the key above it rather
/// than starting a new one: an indented line or a block sequence item.
fn is_value_continuation(line: &str) -> bool {
    line.starts_with(' ')
        || line.starts_with('\t')
        || line == "-"
        || line.starts_with("- ")
        || line.starts_with("-\t")
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

/// Record a write in the original source: set `generated` to `actor` and `now`
/// as the OKF v0.2 flow mapping, leaving every other byte untouched.
///
/// An engram that still carries the legacy `timestamp` key and no `generated`
/// block migrates here, lazily: the `timestamp` line is replaced in place by
/// the `generated` line, so a file only ever changes shape when it is actually
/// edited and the frontmatter keeps its original order.
pub fn touch_generated(source: &str, actor: &str, now: DateTime<FixedOffset>) -> String {
    let line = generated_flow(&Generated {
        by: actor.to_string(),
        at: Some(now),
    });
    set_frontmatter_line(source, &["generated", "timestamp"], line)
}

/// Set the staleness bound in the original source, leaving every other byte
/// untouched.
///
/// An engram that still carries the legacy `review_after` key migrates here,
/// lazily: that one line is replaced by the `stale_after` line, so a file only
/// ever changes shape when it is actually edited and the frontmatter keeps its
/// original order.
pub fn set_stale_after(source: &str, date: NaiveDate) -> String {
    let line = format!("stale_after: {}", date.format("%Y-%m-%d"));
    set_frontmatter_line(source, &["stale_after", "review_after"], line)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_scalar_leaves_the_common_actor_and_instant_forms_bare() {
        for value in [
            "claude-code/1.0.5",
            "human:jordi",
            "process:crystalline-import",
            "2026-07-27T09:15:00+00:00",
            "crystalline/mcp",
        ] {
            assert_eq!(flow_scalar(value), value);
        }
    }

    #[test]
    fn flow_scalar_quotes_anything_a_flow_mapping_could_misread() {
        // Whitespace, flow punctuation, an empty value and a trailing colon all
        // have to be quoted or the line stops parsing as one mapping.
        assert_eq!(flow_scalar("Some Client"), "\"Some Client\"");
        assert_eq!(flow_scalar("a,b"), "\"a,b\"");
        assert_eq!(flow_scalar("{x}"), "\"{x}\"");
        assert_eq!(flow_scalar(""), "\"\"");
        assert_eq!(flow_scalar("trailing:"), "\"trailing:\"");
        assert_eq!(flow_scalar("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn a_quoted_actor_still_round_trips_through_the_parser() {
        let source = format!(
            "---\ntype: engram\n{}\n---\n\nbody\n",
            generated_flow(&Generated {
                by: "Some Client (beta), v2".to_string(),
                at: DateTime::parse_from_rfc3339("2026-07-27T09:15:00+00:00").ok(),
            })
        );
        let engram = crate::parse_engram(&source).unwrap();
        let g = engram.frontmatter.generated.as_ref().unwrap();
        assert_eq!(g.by, "Some Client (beta), v2");
        assert_eq!(emit_engram(&engram), source);
    }

    #[test]
    fn generated_flow_omits_an_absent_instant() {
        let line = generated_flow(&Generated {
            by: "human:jordi".to_string(),
            at: None,
        });
        assert_eq!(line, "generated: { by: human:jordi }");
    }
}
