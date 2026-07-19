//! String-surgical tag rewriting.
//!
//! [`retag`] renames (or merges) a tag inside one Engram's markdown source
//! without re-emitting it: every byte outside the rewritten tag tokens is kept
//! verbatim, so a non-canonical file (unusual spacing, comments, key order)
//! survives a rename untouched. It rewrites two places, and only these two:
//!
//! 1. the frontmatter `tags` entry, in all three shapes the parser accepts (a
//!    block sequence, a flow sequence `[a, b]` and a comma-separated scalar
//!    `a, b`); and
//! 2. trailing observation hashtags on a top-level `- [category] ...` bullet,
//!    the exact tokens [`crate::parse`]'s `split_observation` would peel off the
//!    end (before an optional trailing `(context)` group).
//!
//! It never touches a hashtag inside prose, a hashtag on a non-observation
//! bullet or a `#token` in the middle of an observation: only the trailing run
//! the parser reads as tags. Matching is case-insensitive (tag identity is
//! case-folded), and when the new name is already present the old entry is
//! dropped rather than duplicated, so a merge dedupes.

use std::ops::Range;

use crate::parse::{is_hashtag, locate};

/// Whether a string is a canonical lowercase-with-hyphens tag: non-empty, no
/// leading or trailing hyphen and only lowercase ASCII letters, digits and
/// hyphens. The shared spelling behind verify's E007 and the `tags rename` /
/// `tags merge` name validation, so the CLI and the linter agree on what a
/// well-formed tag is.
pub fn is_lower_hyphen(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Rewrite every occurrence of tag `old` to `new` in one Engram's markdown
/// `source`, string-surgically. Returns the rewritten source and the number of
/// tag tokens changed, or `None` when the tag does not occur (nothing to write).
///
/// `old` and `new` are compared case-folded, so a verbatim `Foo` in the file is
/// matched by `old = "foo"`; the replacement is written as `new` verbatim, which
/// callers pass already normalized to lowercase-with-hyphens. When `new` is
/// already present alongside `old` (on the frontmatter list, or among one
/// bullet's trailing tags), the `old` entry is dropped instead of renamed, so
/// merging two tags never leaves a duplicate.
pub fn retag(source: &str, old: &str, new: &str) -> Option<(String, usize)> {
    let old_f = old.to_lowercase();
    let (fm_edited, fm_count) = retag_frontmatter(source, &old_f, new);
    let (body_edited, body_count) = retag_body(&fm_edited, &old_f, new);
    let total = fm_count + body_count;
    if total == 0 {
        None
    } else {
        Some((body_edited, total))
    }
}

// --- per-item planning -------------------------------------------------------

/// What to do with one tag item (a frontmatter list entry or a trailing
/// hashtag).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    /// Leave the item unchanged.
    Keep,
    /// Rewrite the item's value to the new name.
    Rename,
    /// Remove the item (it would duplicate an already-present new name).
    Drop,
}

/// Plan an action per item over the ordered folded values. The first `old`
/// becomes the new name unless the new name is already present (or was just
/// emitted), in which case every `old` is dropped so no duplicate remains.
/// Returns the per-item actions and how many items matched `old`.
fn plan(values: &[String], old_f: &str, new: &str) -> (Vec<Act>, usize) {
    let new_f = new.to_lowercase();
    let mut emitted = values.iter().any(|v| v.to_lowercase() == new_f);
    let mut acts = Vec::with_capacity(values.len());
    let mut count = 0usize;
    for v in values {
        if v.to_lowercase() == old_f {
            count += 1;
            if emitted {
                acts.push(Act::Drop);
            } else {
                acts.push(Act::Rename);
                emitted = true;
            }
        } else {
            acts.push(Act::Keep);
        }
    }
    (acts, count)
}

/// Strip a single pair of matching surrounding quotes for value comparison. The
/// rewrite always emits the bare new name, so it never needs to reconstruct the
/// original quoting.
fn unquote(s: &str) -> &str {
    let t = s.trim();
    let bytes = t.as_bytes();
    if t.len() >= 2
        && ((bytes[0] == b'"' && bytes[t.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[t.len() - 1] == b'\''))
    {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

// --- frontmatter -------------------------------------------------------------

/// Rewrite the top-level `tags` frontmatter entry. Only the value tokens change;
/// the key, delimiters, indentation and every other line are preserved.
fn retag_frontmatter(source: &str, old_f: &str, new: &str) -> (String, usize) {
    let (has_fm, fm_span, _body) = locate(source);
    if !has_fm {
        return (source.to_string(), 0);
    }
    let fm = &source[fm_span.clone()];

    // Find the top-level `tags:` line and its byte span within `source`.
    let mut line_start = fm_span.start;
    for line in fm.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        if line_sets_tags(content) {
            let colon = content.find(':').unwrap();
            let after = &content[colon + 1..];
            let after_trim = after.trim();
            let value_start = line_start + colon + 1;
            let content_end = line_start + content.len();
            let line_end = line_start + line.len();

            if after_trim.is_empty() {
                // Block sequence: items live on the following `- ` lines,
                // starting after this line's newline.
                return retag_block_sequence(source, line_end, old_f, new);
            } else if after_trim.starts_with('[') {
                // Flow sequence on this line: rewrite between the brackets.
                return retag_flow_sequence(source, value_start, content_end, old_f, new);
            } else {
                // Comma-separated scalar on this line.
                return retag_comma_scalar(source, value_start, content_end, old_f, new);
            }
        }
        line_start += line.len();
    }
    (source.to_string(), 0)
}

/// Whether a frontmatter line sets the top-level `tags` key (`tags:` at column
/// zero). A nested or prefixed key (`  tags:`, `tags_x:`) does not match.
fn line_sets_tags(content: &str) -> bool {
    match content.strip_prefix("tags") {
        Some(rest) => rest.starts_with(':'),
        None => false,
    }
}

/// Rewrite a block sequence whose items are `- value` lines following the
/// `tags:` line. `items_from` is the byte offset just past the `tags:` line's
/// content (the first newline). Dropping an item removes its whole line.
fn retag_block_sequence(
    source: &str,
    items_from: usize,
    old_f: &str,
    new: &str,
) -> (String, usize) {
    // Collect the block items: contiguous `<indent>- value` lines. A line that
    // is not a block item (a new key, or end of frontmatter) ends the sequence.
    struct Item {
        line_span: Range<usize>,
        value_span: Range<usize>,
        value: String,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut pos = items_from;
    let rest = &source[items_from..];
    for line in rest.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim_start();
        let indent = content.len() - trimmed.len();
        if let Some(after_dash) = trimmed.strip_prefix("- ") {
            let value_text = after_dash.trim_end();
            let val_indent = after_dash.len() - after_dash.trim_start().len();
            let vstart = pos + indent + 2 + val_indent;
            let vend = vstart + value_text.trim_start().len();
            items.push(Item {
                line_span: pos..pos + line.len(),
                value_span: vstart..vend,
                value: unquote(value_text).to_string(),
            });
            pos += line.len();
        } else {
            break;
        }
    }
    if items.is_empty() {
        return (source.to_string(), 0);
    }

    let values: Vec<String> = items.iter().map(|i| i.value.clone()).collect();
    let (acts, count) = plan(&values, old_f, new);
    if count == 0 {
        return (source.to_string(), 0);
    }

    // Rebuild from the end so earlier byte offsets stay valid.
    let mut out = source.to_string();
    for (item, act) in items.iter().zip(&acts).rev() {
        match act {
            Act::Keep => {}
            Act::Rename => {
                out.replace_range(item.value_span.clone(), new);
            }
            Act::Drop => {
                out.replace_range(item.line_span.clone(), "");
            }
        }
    }
    (out, count)
}

/// Rewrite a flow sequence `[a, b, c]` between `value_start` (just past the
/// colon) and `content_end` (end of the tags line). Items are comma-separated
/// inside the brackets; dropping removes the item and one adjacent comma.
fn retag_flow_sequence(
    source: &str,
    value_start: usize,
    content_end: usize,
    old_f: &str,
    new: &str,
) -> (String, usize) {
    let segment = &source[value_start..content_end];
    let open_rel = match segment.find('[') {
        Some(i) => i,
        None => return (source.to_string(), 0),
    };
    let close_rel = match segment.rfind(']') {
        Some(i) if i > open_rel => i,
        _ => return (source.to_string(), 0),
    };
    let inner = &segment[open_rel + 1..close_rel];
    let inner_base = value_start + open_rel + 1;

    let (rebuilt, count) = rewrite_comma_items(inner, old_f, new);
    if count == 0 {
        return (source.to_string(), 0);
    }
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..inner_base]);
    out.push_str(&rebuilt);
    out.push_str(&source[inner_base + inner.len()..]);
    (out, count)
}

/// Rewrite a comma-separated scalar `a, b, c` between `value_start` and
/// `content_end`.
fn retag_comma_scalar(
    source: &str,
    value_start: usize,
    content_end: usize,
    old_f: &str,
    new: &str,
) -> (String, usize) {
    let inner = &source[value_start..content_end];
    let (rebuilt, count) = rewrite_comma_items(inner, old_f, new);
    if count == 0 {
        return (source.to_string(), 0);
    }
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..value_start]);
    out.push_str(&rebuilt);
    out.push_str(&source[content_end..]);
    (out, count)
}

/// Rewrite a comma-separated item list, preserving the exact whitespace around
/// each surviving item and its commas. A renamed item keeps its surrounding
/// whitespace; a dropped item takes one adjacent comma with it (the following
/// comma, or the preceding one when it was the last item). Returns the rebuilt
/// list text and the number of items matched.
fn rewrite_comma_items(inner: &str, old_f: &str, new: &str) -> (String, usize) {
    // Decompose `lead0 core0 trail0 , lead1 core1 trail1 , ...` so every byte
    // outside a `core` (the trimmed value) is reproducible. `conn[i]` is the
    // separator joining `core[i]` to `core[i+1]`: `trail[i] , lead[i+1]`.
    let pieces: Vec<&str> = inner.split(',').collect();
    let n = pieces.len();
    let mut lead: Vec<&str> = Vec::with_capacity(n);
    let mut core: Vec<&str> = Vec::with_capacity(n);
    let mut trail: Vec<&str> = Vec::with_capacity(n);
    for p in &pieces {
        let l = p.len() - p.trim_start().len();
        let t = p.len() - p.trim_end().len();
        lead.push(&p[..l]);
        core.push(p.trim());
        trail.push(&p[p.len() - t..]);
    }
    let conn: Vec<String> = (0..n.saturating_sub(1))
        .map(|i| format!("{},{}", trail[i], lead[i + 1]))
        .collect();

    // Non-empty cores are the values the plan operates on.
    let value_idx: Vec<usize> = (0..n).filter(|&i| !core[i].is_empty()).collect();
    let values: Vec<String> = value_idx
        .iter()
        .map(|&i| unquote(core[i]).to_string())
        .collect();
    let (acts, count) = plan(&values, old_f, new);
    if count == 0 {
        return (inner.to_string(), 0);
    }
    let mut act_of: Vec<Option<Act>> = vec![None; n];
    for (vi, &ci) in value_idx.iter().enumerate() {
        act_of[ci] = Some(acts[vi]);
    }

    // A dropped core takes one adjacent connector with it.
    let mut core_dropped = vec![false; n];
    let mut conn_removed = vec![false; n.saturating_sub(1)];
    for (i, dropped) in core_dropped.iter_mut().enumerate() {
        if act_of[i] == Some(Act::Drop) {
            *dropped = true;
            if i + 1 < n {
                conn_removed[i] = true;
            } else if i > 0 {
                conn_removed[i - 1] = true;
            }
        }
    }

    let mut out = String::with_capacity(inner.len());
    out.push_str(lead[0]);
    for i in 0..n {
        if !core_dropped[i] {
            match act_of[i] {
                Some(Act::Rename) => out.push_str(new),
                _ => out.push_str(core[i]),
            }
        }
        if i + 1 < n && !conn_removed[i] {
            out.push_str(&conn[i]);
        }
    }
    out.push_str(trail[n - 1]);
    (out, count)
}

// --- observations ------------------------------------------------------------

/// Rewrite trailing observation hashtags in the body, leaving the frontmatter
/// and every non-observation line untouched.
fn retag_body(source: &str, old_f: &str, new: &str) -> (String, usize) {
    let (_has_fm, _fm_span, body_start) = locate(source);
    let head = &source[..body_start];
    let body = &source[body_start..];

    let mut out = String::with_capacity(source.len());
    out.push_str(head);
    let mut count = 0usize;
    let mut in_fence = false;
    for line in body.split_inclusive('\n') {
        let (content, nl) = match line.strip_suffix('\n') {
            Some(c) => (c, "\n"),
            None => (line, ""),
        };
        if is_fence(content) {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let (new_content, n) = retag_observation_line(content, old_f, new);
        out.push_str(&new_content);
        out.push_str(nl);
        count += n;
    }
    (out, count)
}

/// Whether a line opens or closes a fenced code block (three or more backticks
/// or tildes, up to three leading spaces). Observation scanning skips fences.
fn is_fence(content: &str) -> bool {
    let trimmed = content.trim_start_matches(' ');
    if content.len() - trimmed.len() > 3 {
        return false;
    }
    let first = match trimmed.chars().next() {
        Some(c) if c == '`' || c == '~' => c,
        _ => return false,
    };
    trimmed.chars().take_while(|c| *c == first).count() >= 3
}

/// Rewrite the trailing hashtag run of one line when it is a top-level
/// observation bullet. Returns the (possibly unchanged) line content and how
/// many hashtags were rewritten.
fn retag_observation_line(content: &str, old_f: &str, new: &str) -> (String, usize) {
    // The line, minus a trailing `\r`, must be a `- [category] ...` bullet.
    let cr = content.ends_with('\r');
    let line = content.strip_suffix('\r').unwrap_or(content);
    if !is_observation_bullet(line) {
        return (content.to_string(), 0);
    }

    // Mirror split_observation to bound the trailing hashtag region.
    let trimmed_end = line.trim_end().len();
    let mut scan_end = trimmed_end;
    // A trailing `(context)` group sits after the tags; skip it first.
    if line[..scan_end].ends_with(')')
        && let Some(open) = line[..scan_end].rfind('(')
    {
        scan_end = line[..open].trim_end().len();
    }

    // Walk the maximal run of trailing hashtag tokens.
    let mut spans: Vec<Range<usize>> = Vec::new();
    let mut pos = scan_end;
    loop {
        let seg = &line[..pos];
        let tok_start = seg.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        let token = &line[tok_start..pos];
        if is_hashtag(token) {
            spans.push(tok_start..pos);
            pos = line[..tok_start].trim_end().len();
        } else {
            break;
        }
    }
    if spans.is_empty() {
        return (content.to_string(), 0);
    }
    spans.reverse(); // text order

    let tags_start = spans.first().unwrap().start;
    let region = &line[tags_start..scan_end];
    let (new_region, count) = rewrite_hashtag_region(region, old_f, new);
    if count == 0 {
        return (content.to_string(), 0);
    }

    let mut rebuilt = String::with_capacity(line.len());
    rebuilt.push_str(&line[..tags_start]);
    rebuilt.push_str(&new_region);
    rebuilt.push_str(&line[scan_end..]);
    if cr {
        rebuilt.push('\r');
    }
    (rebuilt, count)
}

/// Whether the line is a top-level observation bullet (`- [category] ...` at
/// zero indent, matching the parser's `top_level_bullet` plus
/// `parse_observation_head`).
fn is_observation_bullet(line: &str) -> bool {
    let Some(content) = line.strip_prefix("- ") else {
        return false;
    };
    if !content.starts_with('[') {
        return false;
    }
    let rest = &content[1..];
    let Some(close) = rest.find(']') else {
        return false;
    };
    let category = &rest[..close];
    !category.is_empty() && !category.contains('[')
}

/// Rewrite the hashtag tokens inside the trailing region `#a #b #c`, preserving
/// the exact whitespace between surviving tokens. A dropped token takes its
/// preceding separator with it (or, if it was first, its following one).
fn rewrite_hashtag_region(region: &str, old_f: &str, new: &str) -> (String, usize) {
    // Tokens are `#...` runs; separators are the whitespace between them. The
    // region starts and ends on a token.
    let mut tokens: Vec<&str> = Vec::new();
    let mut seps: Vec<&str> = Vec::new();
    let mut idx = 0;
    let bytes = region.as_bytes();
    while idx < region.len() {
        // A token: up to the next whitespace.
        let start = idx;
        while idx < region.len() && !bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        tokens.push(&region[start..idx]);
        // Separator: whitespace run.
        let sep_start = idx;
        while idx < region.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if sep_start != idx {
            seps.push(&region[sep_start..idx]);
        }
    }

    let values: Vec<String> = tokens
        .iter()
        .map(|t| t.strip_prefix('#').unwrap_or(t).to_string())
        .collect();
    let (acts, count) = plan(&values, old_f, new);
    if count == 0 {
        return (region.to_string(), 0);
    }

    // Build (sep_before, token) units, then drop the leading separator of the
    // first surviving unit so the region never starts with whitespace.
    let mut kept: Vec<(String, String)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let sep_before = if i == 0 { "" } else { seps[i - 1] };
        match acts[i] {
            Act::Keep => kept.push((sep_before.to_string(), (*token).to_string())),
            Act::Rename => kept.push((sep_before.to_string(), format!("#{new}"))),
            Act::Drop => {}
        }
    }
    if let Some(first) = kept.first_mut() {
        first.0 = String::new();
    }
    let rebuilt: String = kept.iter().map(|(s, t)| format!("{s}{t}")).collect();
    (rebuilt, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retagged(source: &str, old: &str, new: &str) -> (String, usize) {
        retag(source, old, new).expect("retag changed the source")
    }

    #[test]
    fn is_lower_hyphen_accepts_canonical_and_rejects_the_rest() {
        assert!(is_lower_hyphen("multi-word"));
        assert!(is_lower_hyphen("api2"));
        assert!(!is_lower_hyphen(""));
        assert!(!is_lower_hyphen("-x"));
        assert!(!is_lower_hyphen("x-"));
        assert!(!is_lower_hyphen("Foo"));
        assert!(!is_lower_hyphen("a_b"));
        assert!(!is_lower_hyphen("a b"));
    }

    #[test]
    fn block_sequence_rename() {
        let src =
            "---\ntype: engram\ntitle: T\ntags:\n  - foo\n  - keep\nstatus: current\n---\n\nbody\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntype: engram\ntitle: T\ntags:\n  - bar\n  - keep\nstatus: current\n---\n\nbody\n"
        );
    }

    #[test]
    fn block_sequence_merge_drops_the_line() {
        let src = "---\ntitle: T\ntags:\n  - foo\n  - bar\nstatus: current\n---\n\nbody\n";
        // Merge foo into bar: bar already present, so the foo line is removed.
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntitle: T\ntags:\n  - bar\nstatus: current\n---\n\nbody\n"
        );
    }

    #[test]
    fn flow_sequence_rename_preserves_spacing() {
        let src = "---\ntags: [foo, keep]\n---\n\nbody\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(out, "---\ntags: [bar, keep]\n---\n\nbody\n");
    }

    #[test]
    fn flow_sequence_merge_drops_item_and_comma() {
        let src = "---\ntags: [foo, bar, keep]\n---\n\nbody\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(out, "---\ntags: [bar, keep]\n---\n\nbody\n");
    }

    #[test]
    fn comma_scalar_rename_preserves_delimiters() {
        let src = "---\ntags: foo, keep\n---\n\nbody\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(out, "---\ntags: bar, keep\n---\n\nbody\n");
    }

    #[test]
    fn quoted_item_is_matched_and_replaced_bare() {
        let src = "---\ntags: [\"foo\", keep]\n---\n\nbody\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(out, "---\ntags: [bar, keep]\n---\n\nbody\n");
    }

    #[test]
    fn case_insensitive_match_writes_new_verbatim() {
        let src = "---\ntags:\n  - Foo\n---\n\nbody\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(out, "---\ntags:\n  - bar\n---\n\nbody\n");
    }

    #[test]
    fn observation_hashtag_rename_is_boundary_aware() {
        // #data must not match inside #database.
        let src = "---\ntags:\n  - t\n---\n\n- [decision] chose it #data #database\n";
        let (out, n) = retagged(src, "data", "info");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntags:\n  - t\n---\n\n- [decision] chose it #info #database\n"
        );
    }

    #[test]
    fn observation_hashtag_respects_trailing_context() {
        let src = "---\ntags:\n  - t\n---\n\n- [decision] chose it #foo (in prod)\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntags:\n  - t\n---\n\n- [decision] chose it #bar (in prod)\n"
        );
    }

    #[test]
    fn observation_hashtag_merge_dedupes() {
        let src = "---\ntags:\n  - t\n---\n\n- [decision] chose it #foo #bar\n";
        // Merge foo into bar: #bar already trails, so #foo is dropped.
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntags:\n  - t\n---\n\n- [decision] chose it #bar\n"
        );
    }

    #[test]
    fn prose_and_mid_observation_hashtags_are_untouched() {
        // A prose hashtag and a mid-observation hashtag must never change; only
        // the trailing #foo does.
        let src = "---\ntags:\n  - t\n---\n\nSee #foo in the docs.\n\n- [note] see #foo for details #foo\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntags:\n  - t\n---\n\nSee #foo in the docs.\n\n- [note] see #foo for details #bar\n"
        );
    }

    #[test]
    fn byte_preservation_only_touches_the_tag_tokens() {
        let src = "---\ntype: engram\ntitle: Weird  Spacing\ntags:  [foo ,  keep]\ncustom: value # not a tag\n---\n\n# Heading\n\nProse with #foo mid-sentence stays.\n\n- [decision] pick it #foo (context here)\n\nMore prose.\n";
        let (out, n) = retag(src, "foo", "bar").unwrap();
        // Two occurrences: the flow-sequence item and the trailing hashtag.
        assert_eq!(n, 2);
        let expected = "---\ntype: engram\ntitle: Weird  Spacing\ntags:  [bar ,  keep]\ncustom: value # not a tag\n---\n\n# Heading\n\nProse with #foo mid-sentence stays.\n\n- [decision] pick it #bar (context here)\n\nMore prose.\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn no_occurrence_returns_none() {
        let src = "---\ntags:\n  - keep\n---\n\n- [note] nothing here\n";
        assert!(retag(src, "foo", "bar").is_none());
    }

    #[test]
    fn hashtag_inside_code_fence_is_untouched() {
        let src = "---\ntags:\n  - t\n---\n\n```\n- [decision] fake #foo\n```\n\n- [decision] real #foo\n";
        let (out, n) = retagged(src, "foo", "bar");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntags:\n  - t\n---\n\n```\n- [decision] fake #foo\n```\n\n- [decision] real #bar\n"
        );
    }
}
