//! TOON rendering for list-shaped MCP tool results.
//!
//! TOON (Token-Oriented Object Notation) encodes the JSON data model with
//! YAML-like indentation and CSV-like rows for uniform arrays, cutting the
//! repeated key names that dominate list payloads. The win exists only while
//! an array stays tabular-eligible: every row an object with identical keys
//! and primitive-only cells. The engine's JSON breaks both conditions in two
//! known ways - optional fields make rows ragged and tag lists put an array
//! in a cell - so `render` runs a pre-pass fixing exactly those before
//! encoding. The pre-pass exists only on this path: the engine value handed
//! to the CLI, to `json` mode and to non-list tools is never touched.
//!
//! The emitter itself is a hand-written encoder for the canonical TOON v3
//! dialect with the comma delimiter: `key: value` lines, `key:` blocks for
//! nested objects, inline `key[N]: a,b` for primitive arrays, the
//! `key[N]{f1,f2}:` tabular block for uniform object arrays and the hyphenated
//! expanded list for everything else, with conservative string quoting. The
//! exact-output tests below are the conformance guard for that dialect: their
//! expected strings, not this prose, are the binding contract.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Render a list-shaped engine value as TOON. The pre-pass runs first, then
/// the object is emitted per the module dialect. A non-object root cannot be a
/// TOON document, so it falls back to the compact JSON of the untouched value,
/// keeping today's behavior for the defensive path rather than erroring the
/// tool call - engine payloads are always objects.
pub(crate) fn render(value: &Value) -> String {
    let normalized = normalize(value.clone());
    let Value::Object(map) = &normalized else {
        return value.to_string();
    };
    let mut lines = Vec::new();
    emit_object(map, 0, &mut lines);
    lines.join("\n")
}

/// Emit every entry of an object as its own line (or block) at `indent`. Keys
/// iterate in serde_json's Map order, which is alphabetical here since no crate
/// enables `preserve_order`, matching the `json` mode's key order.
fn emit_object(map: &Map<String, Value>, indent: usize, lines: &mut Vec<String>) {
    for (key, value) in map {
        emit_entry(key, value, indent, lines);
    }
}

/// Emit one object entry. A scalar is `key: value`; an object value is a `key:`
/// header followed by its entries at +2 (an empty object is the header alone);
/// an array delegates to `emit_array`.
fn emit_entry(key: &str, value: &Value, indent: usize, lines: &mut Vec<String>) {
    let pad = " ".repeat(indent);
    let key_token = string_token(key);
    match value {
        Value::Object(map) => {
            lines.push(format!("{pad}{key_token}:"));
            emit_object(map, indent + 2, lines);
        }
        Value::Array(items) => emit_array(&key_token, items, indent, lines),
        scalar => lines.push(format!("{pad}{key_token}: {}", scalar_token(scalar))),
    }
}

/// Emit an array in the form its contents allow: `key[0]:` when empty, inline
/// `key[N]: a,b` when every element is a scalar, the `key[N]{fields}:` tabular
/// block when every element is a like-keyed object of primitive cells, and the
/// hyphenated expanded list otherwise. `key_token` is the already-quoted key
/// (empty for a bare array that is itself a list item).
fn emit_array(key_token: &str, items: &[Value], indent: usize, lines: &mut Vec<String>) {
    let pad = " ".repeat(indent);
    let count = items.len();
    if items.is_empty() {
        lines.push(format!("{pad}{key_token}[0]:"));
    } else if items.iter().all(is_scalar) {
        let cells: Vec<String> = items.iter().map(scalar_token).collect();
        lines.push(format!("{pad}{key_token}[{count}]: {}", cells.join(",")));
    } else if let Some(fields) = is_tabular(items) {
        let header: Vec<String> = fields.iter().map(|f| string_token(f)).collect();
        lines.push(format!(
            "{pad}{key_token}[{count}]{{{}}}:",
            header.join(",")
        ));
        let row_pad = " ".repeat(indent + 2);
        for item in items {
            let obj = item.as_object().expect("is_tabular guarantees objects");
            let cells: Vec<String> = fields
                .iter()
                .map(|f| scalar_token(&obj[f.as_str()]))
                .collect();
            lines.push(format!("{row_pad}{}", cells.join(",")));
        }
    } else {
        lines.push(format!("{pad}{key_token}[{count}]:"));
        for item in items {
            emit_list_item(item, indent + 2, lines);
        }
    }
}

/// Emit one element of an expanded list, prefixed with `- ` at `indent`. A
/// scalar sits on the hyphen line; an object puts its first field there and the
/// rest one level deeper; a nested array carries its own `[N]` header after the
/// hyphen. An empty object is a bare `-`.
fn emit_list_item(item: &Value, indent: usize, lines: &mut Vec<String>) {
    let pad = " ".repeat(indent);
    match item {
        Value::Object(map) if !map.is_empty() => {
            let start = lines.len();
            emit_object(map, indent + 2, lines);
            fold_hyphen(lines, start, indent);
        }
        Value::Object(_) => lines.push(format!("{pad}-")),
        Value::Array(items) => {
            let start = lines.len();
            emit_array("", items, indent + 2, lines);
            fold_hyphen(lines, start, indent);
        }
        scalar => lines.push(format!("{pad}- {}", scalar_token(scalar))),
    }
}

/// Fold a `- ` marker onto the first line a list item emitted at `indent + 2`.
/// The marker fills the two columns freed by dropping that extra indent, so the
/// item's continuation lines keep their alignment.
fn fold_hyphen(lines: &mut [String], start: usize, indent: usize) {
    if let Some(line) = lines.get_mut(start) {
        *line = format!("{}- {}", " ".repeat(indent), &line[indent + 2..]);
    }
}

/// The tabular-eligibility check: every element an object, all with the same
/// key set (Map order makes a positional compare a set compare) and every cell
/// a scalar. Returns the shared field names in header order, or `None`.
fn is_tabular(items: &[Value]) -> Option<Vec<&String>> {
    let fields: Vec<&String> = items.first()?.as_object()?.keys().collect();
    for item in items {
        let obj = item.as_object()?;
        if obj.len() != fields.len() || obj.keys().zip(&fields).any(|(k, f)| k != *f) {
            return None;
        }
        if obj.values().any(|v| !is_scalar(v)) {
            return None;
        }
    }
    Some(fields)
}

/// True for the four JSON scalar kinds - the only values a TOON cell, inline
/// array element or `key: value` scalar may hold.
fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

/// Format a scalar as its TOON token: `null`, `true`/`false`, the number as
/// serde_json prints it, or the quoted-when-needed string. Non-scalars never
/// reach here on the encoding paths, so the fallback is defensive only.
fn scalar_token(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => string_token(s),
        other => other.to_string(),
    }
}

/// A string as a TOON token: bare when safe, otherwise wrapped in double quotes
/// with the five string escapes applied.
fn string_token(s: &str) -> String {
    if needs_quoting(s) {
        format!("\"{}\"", escape(s))
    } else {
        s.to_owned()
    }
}

/// Whether a string must be quoted. Conservative by design: a number-lookalike
/// is anything `f64` parses, and any structural or ambiguous character forces
/// quotes. Over-quoting is always valid TOON, so when in doubt this quotes.
fn needs_quoting(s: &str) -> bool {
    if s.is_empty()
        || s.starts_with(|c: char| c.is_whitespace())
        || s.ends_with(|c: char| c.is_whitespace())
        || matches!(s, "true" | "false" | "null")
        || s.starts_with('-')
        || s.parse::<f64>().is_ok()
    {
        return true;
    }
    s.chars().any(|c| {
        matches!(
            c,
            ',' | ':' | '"' | '\\' | '[' | ']' | '{' | '}' | '\n' | '\r' | '\t'
        )
    })
}

/// Apply the five TOON string escapes; every other character passes through.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

/// The tabular-eligibility pre-pass, applied to every array whose elements
/// are all objects: missing keys are filled with null so rows stay uniform,
/// and a row field that is an array of comma-free strings joins into one
/// comma-separated cell (an empty list becomes the empty string). A string
/// list with a comma inside any element is left as an array so its boundaries
/// survive, at the cost of that array's tabular form.
fn normalize(value: Value) -> Value {
    match value {
        Value::Array(items) => {
            let items: Vec<Value> = items.into_iter().map(normalize).collect();
            if !items.is_empty() && items.iter().all(Value::is_object) {
                Value::Array(fill_rows(items))
            } else {
                Value::Array(items)
            }
        }
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

/// Uniform rows: every key that appears in any row appears in all of them,
/// null where a row lacked it, with string-list cells joined.
fn fill_rows(rows: Vec<Value>) -> Vec<Value> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        if let Value::Object(map) = row {
            keys.extend(map.keys().cloned());
        }
    }
    rows.into_iter()
        .map(|row| match row {
            Value::Object(mut map) => {
                let mut out = Map::new();
                for k in &keys {
                    let v = map.remove(k).unwrap_or(Value::Null);
                    out.insert(k.clone(), join_string_array(v));
                }
                Value::Object(out)
            }
            other => other,
        })
        .collect()
}

/// A cell that is an array of strings, every one free of commas, becomes one
/// comma-joined string, since tabular rows only allow primitive cells (an empty
/// list joins to the empty string). When any element itself contains a comma
/// the join would erase the element boundaries, so the array is left untouched:
/// the row keeps a non-primitive cell, forfeits tabular eligibility and falls
/// back to the expanded list form, which preserves every string exactly.
/// Anything that is not an all-strings array passes through unchanged.
fn join_string_array(value: Value) -> Value {
    match &value {
        Value::Array(items)
            if items
                .iter()
                .all(|v| v.as_str().is_some_and(|s| !s.contains(','))) =>
        {
            Value::String(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ragged_rows_are_filled_to_the_union_of_keys() {
        let v = json!({ "hits": [ {"a": 1}, {"a": 2, "b": 3} ] });
        assert_eq!(
            normalize(v),
            json!({ "hits": [ {"a": 1, "b": null}, {"a": 2, "b": 3} ] })
        );
    }

    #[test]
    fn string_list_cells_join_to_one_comma_separated_cell() {
        let v = json!({ "hits": [
            { "permalink": "a", "tags": ["mcp", "rust"] },
            { "permalink": "b", "tags": [] }
        ]});
        assert_eq!(
            normalize(v),
            json!({ "hits": [
                { "permalink": "a", "tags": "mcp,rust" },
                { "permalink": "b", "tags": "" }
            ]})
        );
    }

    #[test]
    fn top_level_string_arrays_and_non_string_arrays_pass_through() {
        // `folders` is already an optimal primitive array and the nested
        // object array inside a row is not a string list; neither changes.
        let v = json!({
            "folders": ["a", "b"],
            "rows": [ { "nested": [ {"x": 1} ] } ]
        });
        assert_eq!(normalize(v.clone()), v);
    }

    #[test]
    fn scalars_quote_exactly_per_the_spec_rules() {
        let v = json!({
            "dash": "-x", "empty": "", "flag": "true", "multiline": "l1\nl2",
            "pad": " x ", "plain": "hello world",
            "snippet": "commas, colons: and \"quotes\"", "version": "2024"
        });
        let expected = "dash: \"-x\"\nempty: \"\"\nflag: \"true\"\nmultiline: \"l1\\nl2\"\npad: \" x \"\nplain: hello world\nsnippet: \"commas, colons: and \\\"quotes\\\"\"\nversion: \"2024\"";
        assert_eq!(render(&v), expected);
    }

    #[test]
    fn uniform_object_arrays_emit_the_tabular_form() {
        let v = json!({
            "count": 2,
            "hits": [
                { "domain": "eng", "permalink": "a", "score": 0.9 },
                { "domain": "eng", "permalink": "b", "score": 0.8 }
            ],
            "total": 2
        });
        let expected =
            "count: 2\nhits[2]{domain,permalink,score}:\n  eng,a,0.9\n  eng,b,0.8\ntotal: 2";
        assert_eq!(render(&v), expected);
    }

    #[test]
    fn primitive_arrays_nested_and_empty_containers_render_canonically() {
        let v = json!({
            "bare": {},
            "folders": ["a", "b c", "d,e"],
            "meta": { "depth": 1, "kind": "scan" },
            "none": []
        });
        let expected =
            "bare:\nfolders[3]: a,b c,\"d,e\"\nmeta:\n  depth: 1\n  kind: scan\nnone[0]:";
        assert_eq!(render(&v), expected);
    }

    #[test]
    fn null_and_bool_cells_render_bare_in_rows() {
        let v = json!({ "hits": [
            { "line": null, "ok": true },
            { "line": 3, "ok": false }
        ]});
        let expected = "hits[2]{line,ok}:\n  null,true\n  3,false";
        assert_eq!(render(&v), expected);
    }

    #[test]
    fn mixed_arrays_fall_back_to_the_expanded_list_form() {
        let v = json!({ "items": [1, { "a": 1, "b": "x" }, "y"] });
        let expected = "items[3]:\n  - 1\n  - a: 1\n    b: x\n  - y";
        assert_eq!(render(&v), expected);
    }

    #[test]
    fn render_applies_the_pre_pass_before_encoding() {
        let v = json!({ "hits": [
            { "a": 1, "tags": ["x", "y"] },
            { "a": 2, "b": true, "tags": [] }
        ]});
        let expected = "hits[2]{a,b,tags}:\n  1,null,\"x,y\"\n  2,true,\"\"";
        assert_eq!(render(&v), expected);
    }

    #[test]
    fn comma_bearing_string_lists_stay_arrays_and_fall_back_to_the_list_form() {
        // A free-text bullet list (list_domains' `when_to_use`) whose strings
        // carry commas must not be comma-joined: the join would erase the
        // bullet boundaries. The array stays put, the row forfeits tabular
        // eligibility and the expanded list form preserves both strings exactly.
        let v = json!({ "domains": [
            { "name": "eng", "when_to_use": ["Route here for a, b", "Also c"] }
        ]});
        let expected =
            "domains[1]:\n  - name: eng\n    when_to_use[2]: \"Route here for a, b\",Also c";
        assert_eq!(render(&v), expected);
    }
}
