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

/// `evolve`: a scope-and-total summary, the family counts, then one numbered
/// block per finding carrying its evidence and the exact next action, followed
/// by the per-rule instruction legend, any truncation notes and the fixed
/// guidance, printed from `engine::EVOLVE_GUIDANCE` rather than from the
/// response key so the CLI and the tool can never state different authority.
///
/// The number is the finding's rank across the whole result, not its position
/// on the page, so an item keeps its number as the reader pages. The class is
/// printed uppercase in the header line of every block, which is what keeps a
/// `MECHANICAL` item (complete intent the archive already records) visually
/// apart from a `JUDGMENT` one (change what the archive claims, propose first)
/// without colour.
pub fn render_evolve(v: &Value, out: &mut impl Write) -> io::Result<()> {
    let (Some(queue), Some(total)) = (
        v.get("queue").and_then(Value::as_array),
        v.get("total").and_then(Value::as_u64),
    ) else {
        return pretty_fallback(v, out);
    };
    let scanned = v
        .get("engrams_scanned")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let page = v.get("page").and_then(Value::as_u64).unwrap_or(1);
    let count = v
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(queue.len() as u64);
    let domains: Vec<&str> = v
        .get("scope")
        .and_then(|s| s.get("domains"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let today = v
        .get("scope")
        .and_then(|s| s.get("today"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let scope = if domains.is_empty() {
        "no domains".to_string()
    } else {
        domains.join(", ")
    };
    let engram_word = if scanned == 1 { "engram" } else { "engrams" };
    let finding_word = if total == 1 { "finding" } else { "findings" };
    writeln!(out, "Sweep of {scope} as of {today}")?;
    writeln!(
        out,
        "{scanned} {engram_word} scanned, {total} {finding_word} (showing {count}, page {page})"
    )?;
    if let Some(unparsed) = v.get("unparsed").and_then(Value::as_u64)
        && unparsed > 0
    {
        let word = if unparsed == 1 { "engram" } else { "engrams" };
        writeln!(out, "{unparsed} unreadable {word} skipped")?;
    }

    let families: Vec<String> = v
        .get("families")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| {
                    let name = f.get("family").and_then(Value::as_str).unwrap_or("");
                    let n = f.get("findings").and_then(Value::as_u64).unwrap_or(0);
                    format!("{name} {n}")
                })
                .collect()
        })
        .unwrap_or_default();
    if !families.is_empty() {
        writeln!(out, "{}", families.join(", "))?;
    }

    if queue.is_empty() {
        writeln!(out)?;
        writeln!(out, "nothing to work in this scope")?;
        return Ok(());
    }

    for item in queue {
        let n = item.get("n").and_then(Value::as_u64).unwrap_or(0);
        let priority = item.get("priority").and_then(Value::as_u64).unwrap_or(0);
        let rule = item.get("rule").and_then(Value::as_str).unwrap_or("");
        let class = item
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_uppercase();
        let domain = item.get("domain").and_then(Value::as_str).unwrap_or("");
        let permalink = item.get("permalink").and_then(Value::as_str).unwrap_or("");
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        writeln!(out)?;
        writeln!(out, "{n}. [{priority}] {rule} {class}")?;
        // A domain-level finding (`V203` is the one today) carries neither
        // permalink nor title, so it addresses the whole domain rather than
        // printing a trailing slash and a blank title where an engram's would
        // be.
        if permalink.is_empty() {
            writeln!(out, "   crystalline://{domain}")?;
        } else if title.is_empty() {
            writeln!(out, "   crystalline://{domain}/{permalink}")?;
        } else {
            writeln!(out, "   {title}  crystalline://{domain}/{permalink}")?;
        }
        if let Some(finding) = item.get("finding").and_then(Value::as_str)
            && !finding.is_empty()
        {
            writeln!(out, "   {finding}")?;
        }
        if let Some(evidence) = item.get("evidence").and_then(Value::as_str)
            && !evidence.is_empty()
        {
            writeln!(out, "   evidence: {evidence}")?;
        }
        if let Some(fix) = item.get("fix").and_then(Value::as_str)
            && !fix.is_empty()
        {
            writeln!(out, "   fix: {fix}")?;
        }
    }

    // The prose instruction is per rule, so it rides a legend rather than
    // repeating under ten findings that share one rule.
    if let Some(actions) = v.get("actions").and_then(Value::as_array)
        && !actions.is_empty()
    {
        writeln!(out)?;
        writeln!(out, "Actions:")?;
        for a in actions {
            let rule = a.get("rule").and_then(Value::as_str).unwrap_or("");
            let instruction = a.get("instruction").and_then(Value::as_str).unwrap_or("");
            writeln!(out, "  {rule}  {instruction}")?;
        }
    }

    if let Some(truncations) = v.get("truncations").and_then(Value::as_array)
        && !truncations.is_empty()
    {
        writeln!(out)?;
        writeln!(out, "Truncated:")?;
        for t in truncations.iter().filter_map(Value::as_str) {
            writeln!(out, "  {t}")?;
        }
    }

    // The one fixed string the engine returns on every call, printed from the
    // constant rather than re-typed here so the CLI and the tool can never
    // state different authority.
    writeln!(out)?;
    writeln!(out, "{}", crystalline_service::engine::EVOLVE_GUIDANCE)
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

    /// A two-finding queue, one of each class, with a legend and a truncation
    /// note: the whole shape in one assertion so the layout is pinned.
    #[test]
    fn evolve_numbers_by_rank_and_marks_the_class() {
        let v = json!({
            "scope": { "domains": ["eng"], "families": [], "rules": [], "min_priority": null, "today": "2026-08-02" },
            "engrams_scanned": 17,
            "unparsed": 0,
            "total": 12,
            "page": 2,
            "limit": 2,
            "count": 2,
            "families": [
                { "family": "temporal", "findings": 5 },
                { "family": "structure", "findings": 7 },
            ],
            "queue": [
                {
                    "n": 3, "priority": 90, "rule": "V005", "class": "mechanical",
                    "domain": "eng", "permalink": "old-pipeline", "title": "Old pipeline",
                    "line": null, "finding": "still stable but superseded",
                    "evidence": "status=stable", "fix": "set_frontmatter status=superseded",
                },
                {
                    "n": 4, "priority": 85, "rule": "V001", "class": "judgment",
                    "domain": "eng", "permalink": "old-window", "title": "Old window",
                    "line": null, "finding": "valid_to elapsed", "evidence": "valid_to=2026-01-01",
                    "fix": "",
                },
            ],
            "actions": [{ "rule": "V005", "instruction": "Finish the retirement." }],
            "guidance": "ignored: the CLI prints the constant",
            "truncations": ["eng - V003 capped at 10 oldest of 57"],
        });
        let out = render_to_string(render_evolve, &v);
        let expected = format!(
            "Sweep of eng as of 2026-08-02\n\
             17 engrams scanned, 12 findings (showing 2, page 2)\n\
             temporal 5, structure 7\n\
             \n\
             3. [90] V005 MECHANICAL\n\
             \x20  Old pipeline  crystalline://eng/old-pipeline\n\
             \x20  still stable but superseded\n\
             \x20  evidence: status=stable\n\
             \x20  fix: set_frontmatter status=superseded\n\
             \n\
             4. [85] V001 JUDGMENT\n\
             \x20  Old window  crystalline://eng/old-window\n\
             \x20  valid_to elapsed\n\
             \x20  evidence: valid_to=2026-01-01\n\
             \n\
             Actions:\n\
             \x20 V005  Finish the retirement.\n\
             \n\
             Truncated:\n\
             \x20 eng - V003 capped at 10 oldest of 57\n\
             \n\
             {}\n",
            crystalline_service::engine::EVOLVE_GUIDANCE
        );
        assert_eq!(out, expected);
    }

    /// A finding about the whole domain rather than one engram (`V203`) has no
    /// permalink and no title, so it addresses the domain instead of printing a
    /// dangling slash where an engram's permalink would be.
    #[test]
    fn evolve_domain_level_finding_addresses_the_domain() {
        let v = json!({
            "scope": { "domains": ["eng"], "today": "2026-08-02" },
            "engrams_scanned": 6,
            "total": 1, "page": 1, "limit": 10, "count": 1,
            "families": [{ "family": "redundancy", "findings": 1 }],
            "queue": [{
                "n": 1, "priority": 30, "rule": "V203", "class": "judgment",
                "domain": "eng", "permalink": "", "title": "", "line": null,
                "finding": "2 tag spellings look like one tag (plural variants)",
                "evidence": "#vent on 1 engram(s); #vents on 5 engram(s)",
                "fix": "crystalline tags merge vent vents",
            }],
            "actions": [], "truncations": [],
        });
        let out = render_to_string(render_evolve, &v);
        assert!(out.contains("\n   crystalline://eng\n"), "{out}");
        assert!(!out.contains("crystalline://eng/"), "{out}");
    }

    /// A clean sweep says so rather than printing an empty list, and still
    /// reports what it scanned.
    #[test]
    fn evolve_empty_queue_says_nothing_to_work() {
        let v = json!({
            "scope": { "domains": ["eng", "ops"], "today": "2026-08-02" },
            "engrams_scanned": 1,
            "total": 0, "page": 1, "limit": 10, "count": 0,
            "families": [], "queue": [], "actions": [], "truncations": [],
        });
        let out = render_to_string(render_evolve, &v);
        assert_eq!(
            out,
            "Sweep of eng, ops as of 2026-08-02\n1 engram scanned, 0 findings (showing 0, page 1)\n\nnothing to work in this scope\n"
        );
    }

    /// An engram the sweep could not read is reported rather than silently
    /// dropped from the scanned count.
    #[test]
    fn evolve_reports_unparsed_engrams() {
        let v = json!({
            "scope": { "domains": ["eng"], "today": "2026-08-02" },
            "engrams_scanned": 4, "unparsed": 1,
            "total": 0, "page": 1, "limit": 10, "count": 0,
            "families": [], "queue": [], "actions": [], "truncations": [],
        });
        let out = render_to_string(render_evolve, &v);
        assert!(out.contains("1 unreadable engram skipped"), "{out}");
    }

    #[test]
    fn evolve_falls_back_when_shape_is_wrong() {
        let v = json!({ "scope": { "domains": ["eng"] } });
        let out = render_to_string(render_evolve, &v);
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
