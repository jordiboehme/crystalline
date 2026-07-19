//! Human renderings of the engine's JSON results for the CLI data commands.
//!
//! Each renderer takes the raw `serde_json::Value` a tool returned and writes a
//! terminal-friendly view of it. They are used only when the global `--json`
//! flag is off; with `--json` the caller prints the value unchanged. A renderer
//! only formats the value it is handed - it never queries the engine again - and
//! when the value lacks the keys it expects it degrades to pretty JSON rather
//! than panicking or printing a half-formed line, so an unfamiliar shape is
//! shown in full instead of mangled.

use std::collections::HashMap;
use std::io::{self, Write};

use serde_json::Value;

/// Emit pretty JSON, byte-identical to the CLI's `print_value(value, false)`
/// path. Every renderer falls back to this when the value is not the shape it
/// knows how to format.
fn pretty_fallback(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    writeln!(out, "{text}")
}

/// `read`: the engram address on the first line, a blank line, then the engram
/// content verbatim (real newlines, no JSON escaping).
pub fn render_read(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(domain), Some(permalink), Some(content)) = (
        v.get("domain").and_then(Value::as_str),
        v.get("permalink").and_then(Value::as_str),
        v.get("content").and_then(Value::as_str),
    ) else {
        return pretty_fallback(v, out);
    };
    writeln!(out, "crystalline://{domain}/{permalink}")?;
    writeln!(out)?;
    write!(out, "{content}")
}

/// `search`: one line per hit, then a `showing N of TOTAL (page P)` footer.
pub fn render_search(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(hits), Some(total), Some(count)) = (
        v.get("hits").and_then(Value::as_array),
        v.get("total").and_then(Value::as_u64),
        v.get("count").and_then(Value::as_u64),
    ) else {
        return pretty_fallback(v, out);
    };
    let page = v.get("page").and_then(Value::as_u64).unwrap_or(1);
    render_hit_list(hits, count, total, page, out)
}

/// `recent`: the same hit list as search. Recent activity does not paginate, so
/// the footer reports the whole result as a single page.
pub fn render_recent(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(engrams), Some(count)) = (
        v.get("engrams").and_then(Value::as_array),
        v.get("count").and_then(Value::as_u64),
    ) else {
        return pretty_fallback(v, out);
    };
    render_hit_list(engrams, count, count, 1, out)
}

/// The shared body of `search` and `recent`: one primary line per engram with
/// its domain, title and address, an indented snippet line when the engram
/// carries one, and a paged footer. An empty result prints a friendly line.
fn render_hit_list(
    items: &[Value],
    count: u64,
    total: u64,
    page: u64,
    out: &mut impl Write,
) -> io::Result<()> {
    if total == 0 {
        return writeln!(out, "no results");
    }
    for item in items {
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        let domain = item.get("domain").and_then(Value::as_str).unwrap_or("");
        let permalink = item.get("permalink").and_then(Value::as_str).unwrap_or("");
        writeln!(
            out,
            "{title}  [{domain}]  crystalline://{domain}/{permalink}"
        )?;
        if let Some(snippet) = item.get("snippet").and_then(Value::as_str) {
            let snippet = snippet.trim();
            if !snippet.is_empty() {
                writeln!(out, "    {snippet}")?;
            }
        }
    }
    writeln!(out, "showing {count} of {total} (page {page})")
}

/// `context`: a header naming the anchor, then one line per related engram
/// labelled with the relation type it was reached over (or its domain when no
/// relation edge points at it).
pub fn render_context(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(anchor), Some(nodes)) = (
        v.get("anchor").and_then(Value::as_str),
        v.get("nodes").and_then(Value::as_array),
    ) else {
        return pretty_fallback(v, out);
    };
    let empty = Vec::new();
    let edges = v.get("edges").and_then(Value::as_array).unwrap_or(&empty);

    // The first inbound relation type per node, used to label how each related
    // engram connects into the neighbourhood.
    let mut rel_by_node: HashMap<i64, &str> = HashMap::new();
    for edge in edges {
        if let (Some(to), Some(rel)) = (
            edge.get("to").and_then(Value::as_i64),
            edge.get("rel_type").and_then(Value::as_str),
        ) {
            rel_by_node.entry(to).or_insert(rel);
        }
    }

    writeln!(out, "context for {anchor}")?;
    let mut related = 0usize;
    for node in nodes {
        if node.get("seed").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        related += 1;
        let title = node
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        let domain = node.get("domain").and_then(Value::as_str).unwrap_or("");
        let permalink = node.get("permalink").and_then(Value::as_str).unwrap_or("");
        let label = node
            .get("id")
            .and_then(Value::as_i64)
            .and_then(|id| rel_by_node.get(&id).copied())
            .unwrap_or(domain);
        writeln!(
            out,
            "  {label}: {title}  crystalline://{domain}/{permalink}"
        )?;
    }
    if related == 0 {
        writeln!(out, "  (no related engrams)")?;
    }
    Ok(())
}

/// `vocabulary`: three labelled sections - the tags in use with their engram
/// and observation counts, the observation categories with counts and the
/// relation types with counts. An empty facet prints a `(none)` line so the
/// section headers stay stable.
pub fn render_vocabulary(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(tags), Some(categories), Some(relation_types)) = (
        v.get("tags").and_then(Value::as_array),
        v.get("categories").and_then(Value::as_array),
        v.get("relation_types").and_then(Value::as_array),
    ) else {
        return pretty_fallback(v, out);
    };

    writeln!(out, "Tags:")?;
    if tags.is_empty() {
        writeln!(out, "  (none)")?;
    }
    for t in tags {
        let name = t.get("name").and_then(Value::as_str).unwrap_or("");
        let engrams = t.get("engrams").and_then(Value::as_i64).unwrap_or(0);
        let observations = t.get("observations").and_then(Value::as_i64).unwrap_or(0);
        let eng_word = if engrams == 1 { "engram" } else { "engrams" };
        let obs_word = if observations == 1 {
            "observation"
        } else {
            "observations"
        };
        writeln!(
            out,
            "  {name}  {engrams} {eng_word}, {observations} {obs_word}"
        )?;
    }

    writeln!(out, "Categories:")?;
    render_named_counts(categories, out)?;

    writeln!(out, "Relation types:")?;
    render_named_counts(relation_types, out)?;

    // Near-duplicate tag clusters, present only when the engine found any.
    if let Some(clusters) = v.get("clusters").and_then(Value::as_array)
        && !clusters.is_empty()
    {
        writeln!(out, "Near-duplicate tags:")?;
        for c in clusters {
            let reason = c.get("reason").and_then(Value::as_str).unwrap_or("");
            let tags: Vec<&str> = c
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            writeln!(out, "  {} ({reason})", tags.join(", "))?;
        }
        writeln!(out, "  merge with `crystalline tags merge`")?;
    }

    // Tag aliases in effect, present only when the domain declares any.
    if let Some(aliases) = v.get("aliases").and_then(Value::as_array)
        && !aliases.is_empty()
    {
        writeln!(out, "Aliases:")?;
        for a in aliases {
            let alias = a.get("alias").and_then(Value::as_str).unwrap_or("");
            let canonical = a.get("canonical").and_then(Value::as_str).unwrap_or("");
            writeln!(out, "  {alias} -> {canonical}")?;
        }
    }

    Ok(())
}

/// The shared body of the `Categories` and `Relation types` sections: one
/// `name  count` line per term, or a single `(none)` line when the facet is
/// empty.
fn render_named_counts(items: &[Value], out: &mut impl Write) -> io::Result<()> {
    if items.is_empty() {
        return writeln!(out, "  (none)");
    }
    for i in items {
        let name = i.get("name").and_then(Value::as_str).unwrap_or("");
        let count = i.get("count").and_then(Value::as_i64).unwrap_or(0);
        writeln!(out, "  {name}  {count}")?;
    }
    Ok(())
}

/// `write`: a single confirmation line carrying the new engram's address.
pub fn render_write(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(domain), Some(permalink)) = (
        v.get("domain").and_then(Value::as_str),
        v.get("permalink").and_then(Value::as_str),
    ) else {
        return pretty_fallback(v, out);
    };
    let action = v.get("action").and_then(Value::as_str).unwrap_or("wrote");
    writeln!(out, "{action} crystalline://{domain}/{permalink}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render_to_string(f: impl Fn(&Value, &mut Vec<u8>) -> io::Result<()>, v: &Value) -> String {
        let mut buf = Vec::new();
        f(v, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn read_prints_address_blank_line_then_verbatim_content() {
        let v = json!({
            "domain": "eng",
            "permalink": "alpha",
            "content": "line one\nline \"two\"\n",
        });
        let out = render_to_string(render_read, &v);
        assert_eq!(out, "crystalline://eng/alpha\n\nline one\nline \"two\"\n");
    }

    #[test]
    fn read_falls_back_to_pretty_json_when_content_missing() {
        let v = json!({ "domain": "eng", "permalink": "alpha" });
        let out = render_to_string(render_read, &v);
        assert_eq!(
            out,
            format!("{}\n", serde_json::to_string_pretty(&v).unwrap())
        );
    }

    #[test]
    fn search_lists_each_hit_with_snippet_and_footer() {
        let v = json!({
            "mode": "text",
            "total": 3,
            "page": 2,
            "limit": 1,
            "count": 1,
            "hits": [
                { "domain": "eng", "permalink": "alpha", "title": "Alpha", "snippet": "  a snippet  " },
            ],
        });
        let out = render_to_string(render_search, &v);
        assert_eq!(
            out,
            "Alpha  [eng]  crystalline://eng/alpha\n    a snippet\nshowing 1 of 3 (page 2)\n"
        );
    }

    #[test]
    fn search_empty_prints_no_results() {
        let v =
            json!({ "mode": "text", "total": 0, "page": 1, "limit": 10, "count": 0, "hits": [] });
        let out = render_to_string(render_search, &v);
        assert_eq!(out, "no results\n");
    }

    #[test]
    fn recent_footer_is_single_page() {
        let v = json!({
            "timeframe": "7d",
            "count": 2,
            "engrams": [
                { "domain": "eng", "permalink": "alpha", "title": "Alpha" },
                { "domain": "eng", "permalink": "beta", "title": "Beta" },
            ],
        });
        let out = render_to_string(render_recent, &v);
        assert_eq!(
            out,
            "Alpha  [eng]  crystalline://eng/alpha\nBeta  [eng]  crystalline://eng/beta\nshowing 2 of 2 (page 1)\n"
        );
    }

    #[test]
    fn context_labels_related_by_relation_then_domain() {
        let v = json!({
            "anchor": "crystalline://eng/alpha",
            "depth": 1,
            "timeframe": null,
            "nodes": [
                { "id": 1, "domain": "eng", "permalink": "alpha", "title": "Alpha", "seed": true },
                { "id": 2, "domain": "eng", "permalink": "beta", "title": "Beta", "seed": false },
                { "id": 3, "domain": "ops", "permalink": "gamma", "title": "Gamma", "seed": false },
            ],
            "edges": [
                { "from": 1, "to": 2, "rel_type": "depends_on", "kind": "relation" },
            ],
        });
        let out = render_to_string(render_context, &v);
        assert_eq!(
            out,
            "context for crystalline://eng/alpha\n  depends_on: Beta  crystalline://eng/beta\n  ops: Gamma  crystalline://ops/gamma\n"
        );
    }

    #[test]
    fn context_with_only_the_seed_says_no_related() {
        let v = json!({
            "anchor": "crystalline://eng/alpha",
            "depth": 1,
            "timeframe": null,
            "nodes": [
                { "id": 1, "domain": "eng", "permalink": "alpha", "title": "Alpha", "seed": true },
            ],
            "edges": [],
        });
        let out = render_to_string(render_context, &v);
        assert_eq!(
            out,
            "context for crystalline://eng/alpha\n  (no related engrams)\n"
        );
    }

    #[test]
    fn vocabulary_lists_tags_categories_and_relation_types() {
        let v = json!({
            "domain": "eng",
            "tags": [
                { "name": "database", "engrams": 2, "observations": 2 },
                { "name": "api", "engrams": 1, "observations": 1 },
            ],
            "categories": [
                { "name": "decision", "count": 2 },
                { "name": "pattern", "count": 1 },
            ],
            "relation_types": [
                { "name": "depends_on", "count": 1 },
            ],
        });
        let out = render_to_string(render_vocabulary, &v);
        assert_eq!(
            out,
            "Tags:\n  database  2 engrams, 2 observations\n  api  1 engram, 1 observation\nCategories:\n  decision  2\n  pattern  1\nRelation types:\n  depends_on  1\n"
        );
    }

    #[test]
    fn vocabulary_lists_aliases_after_the_other_sections() {
        let v = json!({
            "domain": "eng",
            "tags": [{ "name": "color", "engrams": 2, "observations": 0 }],
            "categories": [],
            "relation_types": [],
            "aliases": [
                { "alias": "colour", "canonical": "color" },
                { "alias": "hue", "canonical": "color" },
            ],
        });
        let out = render_to_string(render_vocabulary, &v);
        assert_eq!(
            out,
            "Tags:\n  color  2 engrams, 0 observations\nCategories:\n  (none)\nRelation types:\n  (none)\nAliases:\n  colour -> color\n  hue -> color\n"
        );
    }

    #[test]
    fn vocabulary_omits_the_aliases_section_when_absent() {
        // The existing shape without an `aliases` key prints no Aliases section.
        let v = json!({ "domain": "eng", "tags": [], "categories": [], "relation_types": [] });
        let out = render_to_string(render_vocabulary, &v);
        assert!(!out.contains("Aliases:"), "no Aliases section: {out}");
    }

    #[test]
    fn vocabulary_empty_facets_print_none() {
        let v = json!({ "domain": null, "tags": [], "categories": [], "relation_types": [] });
        let out = render_to_string(render_vocabulary, &v);
        assert_eq!(
            out,
            "Tags:\n  (none)\nCategories:\n  (none)\nRelation types:\n  (none)\n"
        );
    }

    #[test]
    fn vocabulary_falls_back_when_shape_is_wrong() {
        let v = json!({ "domain": "eng" });
        let out = render_to_string(render_vocabulary, &v);
        assert_eq!(
            out,
            format!("{}\n", serde_json::to_string_pretty(&v).unwrap())
        );
    }

    #[test]
    fn write_confirms_action_and_address() {
        let v = json!({ "domain": "eng", "permalink": "zeta", "action": "created" });
        let out = render_to_string(render_write, &v);
        assert_eq!(out, "created crystalline://eng/zeta\n");
    }

    #[test]
    fn write_falls_back_when_permalink_missing() {
        let v = json!({ "domain": "eng", "action": "created" });
        let out = render_to_string(render_write, &v);
        assert_eq!(
            out,
            format!("{}\n", serde_json::to_string_pretty(&v).unwrap())
        );
    }
}
