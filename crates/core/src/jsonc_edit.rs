//! Span-preserving edits to JSON and JSONC files a harness shares with the
//! person.
//!
//! Install writes one value into a file it does not own (`mcpServers` in
//! `~/.cursor/mcp.json`, a SessionStart group in `~/.gemini/settings.json`),
//! and uninstall takes it out again. Everything around that value - other
//! keys, their order, comments, blank lines, trailing commas, tabs or spaces,
//! CRLF, a missing final newline - must come back byte for byte. So this
//! module never re-serializes a file. It parses the text for byte ranges and
//! then splices: an insert adds exactly one run of text next to the last
//! member of a container, and a removal deletes exactly the run that insert
//! added. The two rules are written as inverses of each other, so uninstall
//! after install gives back the original bytes.
//!
//! The one case that cannot come back is a container the person had left
//! empty (`"mcpServers": {}`): `Prune::Emptied` removes every container a
//! removal empties, because nothing records which of them install created.
//!
//! A file that does not exist yet is written by [`render_new`]: 2-space
//! indent, arrays of scalars on one line, objects one key per line and a
//! trailing newline, with the key order the caller gave in [`JsonIn`].

use std::fmt;

use jsonc_parser::ast;
use jsonc_parser::common::Ranged;
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};

/// One step of a path into a JSON document, borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seg<'a> {
    Key(&'a str),
    Index(usize),
}

/// One step of a path into a JSON document, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegBuf {
    Key(String),
    Index(usize),
}

/// A value to write, with its key order kept as given. The workspace's
/// `serde_json` map is sorted, which would write `args` before `command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonIn {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<JsonIn>),
    Object(Vec<(String, JsonIn)>),
}

impl JsonIn {
    /// The same value as a `serde_json::Value`, for comparisons that do not
    /// care about key order.
    pub fn to_value(&self) -> serde_json::Value {
        match self {
            JsonIn::Null => serde_json::Value::Null,
            JsonIn::Bool(b) => serde_json::Value::Bool(*b),
            JsonIn::Int(n) => serde_json::Value::from(*n),
            JsonIn::Str(s) => serde_json::Value::String(s.clone()),
            JsonIn::Array(items) => {
                serde_json::Value::Array(items.iter().map(JsonIn::to_value).collect())
            }
            JsonIn::Object(members) => serde_json::Value::Object(
                members
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_value()))
                    .collect(),
            ),
        }
    }
}

/// Why a file was not edited. Nothing is written when any edit fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsoncError {
    /// The text is not valid JSON or JSONC. Line and column are one-based.
    Parse {
        line: usize,
        column: usize,
        message: String,
    },
    /// The value at this dotted path (empty for the top level) is not an
    /// object, so a key cannot be written into it.
    NotAnObject { at: String },
    /// The value at this dotted path is not an array.
    NotAnArray { at: String },
    /// The array at this dotted path has no element at the index.
    IndexOutOfRange { at: String },
}

impl fmt::Display for JsoncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsoncError::Parse {
                line,
                column,
                message,
            } => write!(
                f,
                "line {line}, column {column}: {message}. Fix the JSON there and run the command again"
            ),
            JsoncError::NotAnObject { at } if at.is_empty() => write!(
                f,
                "the file does not hold a JSON object at the top level. Make it an object ({{}}) and run the command again"
            ),
            JsoncError::NotAnObject { at } => write!(
                f,
                "\"{at}\" is not a JSON object. Make it an object ({{}}) or remove it and run the command again"
            ),
            JsoncError::NotAnArray { at } => write!(
                f,
                "\"{at}\" is not a JSON array. Make it an array ([]) or remove it and run the command again"
            ),
            JsoncError::IndexOutOfRange { at } => write!(
                f,
                "\"{at}\" does not exist. Check the file and run the command again"
            ),
        }
    }
}

impl std::error::Error for JsoncError {}

/// What a removal does with the containers above the removed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prune {
    /// Leave them, even when they are now empty.
    Never,
    /// Remove each parent container this removal left empty, walking up to
    /// (not including) the top-level object.
    Emptied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Replace the value at a path, or insert the key (creating missing
    /// parent objects). No change when the value is already equal.
    Set { path: Vec<SegBuf>, value: JsonIn },
    /// Append to the array at a path, creating the array and missing parents.
    Push { path: Vec<SegBuf>, value: JsonIn },
    /// Delete a key or an array element. A missing path is no change.
    Remove { path: Vec<SegBuf>, prune: Prune },
}

/// Parse JSON or JSONC (comments and trailing commas allowed) into a value.
/// Blank text reads as an empty object, the same as a missing file.
pub fn parse_value(text: &str) -> Result<serde_json::Value, JsoncError> {
    Ok(parse(text)?
        .map(serde_json::Value::from)
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())))
}

/// The value at a path, or `None` when the path does not exist.
pub fn get(text: &str, path: &[Seg<'_>]) -> Result<Option<serde_json::Value>, JsoncError> {
    let mut node = parse_value(text)?;
    for seg in path {
        let next = match (seg, &mut node) {
            (Seg::Key(k), serde_json::Value::Object(map)) => map.remove(*k),
            (Seg::Index(i), serde_json::Value::Array(items)) if *i < items.len() => {
                Some(items.swap_remove(*i))
            }
            _ => None,
        };
        match next {
            Some(v) => node = v,
            None => return Ok(None),
        }
    }
    Ok(Some(node))
}

/// Apply every edit in order. `text: None` means the file does not exist:
/// the edits then build a new document, printed by [`render_new`]. Returns
/// `None` when the result equals the input, so the caller writes nothing.
pub fn apply_edits(text: Option<&str>, edits: &[Edit]) -> Result<Option<String>, JsoncError> {
    let Some(original) = text.filter(|t| !t.trim().is_empty()) else {
        let mut root = JsonIn::Object(Vec::new());
        let mut changed = false;
        for edit in edits {
            changed |= tree::apply(&mut root, edit)?;
        }
        let empty = matches!(&root, JsonIn::Object(m) if m.is_empty());
        return Ok((changed && !empty).then(|| render_new(&root)));
    };
    let mut current = original.to_string();
    for edit in edits {
        if let Some(next) = splice::apply(&current, edit)? {
            current = next;
        }
    }
    Ok((current != original).then_some(current))
}

/// Print a value as a new file: 2-space indent, arrays of scalars on one
/// line, one key per line and a trailing newline.
pub fn render_new(value: &JsonIn) -> String {
    let mut out = render(value, "", "  ", "\n");
    out.push('\n');
    out
}

fn render(value: &JsonIn, indent: &str, unit: &str, nl: &str) -> String {
    let mut out = String::new();
    render_into(&mut out, value, indent, unit, nl);
    out
}

fn render_into(out: &mut String, value: &JsonIn, indent: &str, unit: &str, nl: &str) {
    match value {
        JsonIn::Null => out.push_str("null"),
        JsonIn::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsonIn::Int(n) => out.push_str(&n.to_string()),
        JsonIn::Str(s) => out.push_str(&quote(s)),
        JsonIn::Array(items) if items.is_empty() => out.push_str("[]"),
        JsonIn::Array(items)
            if items
                .iter()
                .all(|v| !matches!(v, JsonIn::Array(_) | JsonIn::Object(_))) =>
        {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_into(out, item, indent, unit, nl);
            }
            out.push(']');
        }
        JsonIn::Array(items) => {
            let inner = format!("{indent}{unit}");
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(nl);
                out.push_str(&inner);
                render_into(out, item, &inner, unit, nl);
            }
            out.push_str(nl);
            out.push_str(indent);
            out.push(']');
        }
        JsonIn::Object(members) if members.is_empty() => out.push_str("{}"),
        JsonIn::Object(members) => {
            let inner = format!("{indent}{unit}");
            out.push('{');
            for (i, (key, item)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(nl);
                out.push_str(&inner);
                out.push_str(&quote(key));
                out.push_str(": ");
                render_into(out, item, &inner, unit, nl);
            }
            out.push_str(nl);
            out.push_str(indent);
            out.push('}');
        }
    }
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("a string always serializes")
}

/// Strict JSON plus comments and trailing commas. The loose JSON5 forms
/// stay off: the splice rules rely on a comma between every two members.
fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn parse(text: &str) -> Result<Option<ast::Value<'_>>, JsoncError> {
    parse_to_ast(text, &CollectOptions::default(), &parse_options())
        .map(|result| result.value)
        .map_err(|e| JsoncError::Parse {
            line: e.line_display(),
            column: e.column_display(),
            message: e.kind().to_string(),
        })
}

fn dotted(path: &[SegBuf]) -> String {
    path.iter()
        .map(|seg| match seg {
            SegBuf::Key(k) => k.clone(),
            SegBuf::Index(i) => i.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The value the missing tail of a path needs: nested objects down to
/// `leaf`. An index in a missing tail cannot be created.
fn build(prefix: &[SegBuf], rest: &[SegBuf], leaf: JsonIn) -> Result<JsonIn, JsoncError> {
    let mut value = leaf;
    for (i, seg) in rest.iter().enumerate().rev() {
        match seg {
            SegBuf::Key(k) => value = JsonIn::Object(vec![(k.clone(), value)]),
            SegBuf::Index(_) => {
                let mut at = prefix.to_vec();
                at.extend_from_slice(&rest[..=i]);
                return Err(JsoncError::IndexOutOfRange { at: dotted(&at) });
            }
        }
    }
    Ok(value)
}

/// Edits on an in-memory document, for a file that does not exist yet.
mod tree {
    use super::{Edit, JsonIn, JsoncError, Prune, SegBuf, build, dotted};

    pub(super) fn apply(root: &mut JsonIn, edit: &Edit) -> Result<bool, JsoncError> {
        match edit {
            Edit::Set { path, value } => set(root, path, 0, value.clone(), false),
            Edit::Push { path, value } => set(root, path, 0, value.clone(), true),
            Edit::Remove { path, prune } => Ok(remove(root, path, *prune)),
        }
    }

    fn set(
        node: &mut JsonIn,
        path: &[SegBuf],
        depth: usize,
        value: JsonIn,
        push: bool,
    ) -> Result<bool, JsoncError> {
        if depth == path.len() {
            if push {
                let JsonIn::Array(items) = node else {
                    return Err(JsoncError::NotAnArray { at: dotted(path) });
                };
                items.push(value);
                return Ok(true);
            }
            if node.to_value() == value.to_value() {
                return Ok(false);
            }
            *node = value;
            return Ok(true);
        }
        let at = || dotted(&path[..depth]);
        match (&path[depth], node) {
            (SegBuf::Key(k), JsonIn::Object(members)) => {
                match members.iter().rposition(|(name, _)| name == k) {
                    Some(i) => set(&mut members[i].1, path, depth + 1, value, push),
                    None => {
                        let leaf = if push {
                            JsonIn::Array(vec![value])
                        } else {
                            value
                        };
                        let built = build(&path[..=depth], &path[depth + 1..], leaf)?;
                        members.push((k.clone(), built));
                        Ok(true)
                    }
                }
            }
            (SegBuf::Index(i), JsonIn::Array(items)) => match items.get_mut(*i) {
                Some(item) => set(item, path, depth + 1, value, push),
                None => Err(JsoncError::IndexOutOfRange {
                    at: dotted(&path[..=depth]),
                }),
            },
            (SegBuf::Key(_), _) => Err(JsoncError::NotAnObject { at: at() }),
            (SegBuf::Index(_), _) => Err(JsoncError::NotAnArray { at: at() }),
        }
    }

    fn remove(root: &mut JsonIn, path: &[SegBuf], prune: Prune) -> bool {
        let Some((last, parent_path)) = path.split_last() else {
            return false;
        };
        let Some(parent) = lookup(root, parent_path) else {
            return false;
        };
        let removed = match (last, parent) {
            (SegBuf::Key(k), JsonIn::Object(members)) => {
                match members.iter().rposition(|(name, _)| name == k) {
                    Some(i) => {
                        members.remove(i);
                        true
                    }
                    None => false,
                }
            }
            (SegBuf::Index(i), JsonIn::Array(items)) if *i < items.len() => {
                items.remove(*i);
                true
            }
            _ => false,
        };
        if removed && prune == Prune::Emptied && !parent_path.is_empty() {
            let emptied = matches!(
                lookup(root, parent_path),
                Some(JsonIn::Object(m)) if m.is_empty()
            ) || matches!(lookup(root, parent_path), Some(JsonIn::Array(a)) if a.is_empty());
            if emptied {
                remove(root, parent_path, prune);
            }
        }
        removed
    }

    fn lookup<'a>(node: &'a mut JsonIn, path: &[SegBuf]) -> Option<&'a mut JsonIn> {
        let Some((first, rest)) = path.split_first() else {
            return Some(node);
        };
        let next = match (first, node) {
            (SegBuf::Key(k), JsonIn::Object(members)) => members
                .iter_mut()
                .rev()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v)?,
            (SegBuf::Index(i), JsonIn::Array(items)) => items.get_mut(*i)?,
            _ => return None,
        };
        lookup(next, rest)
    }
}

/// Edits on existing text, as byte splices.
mod splice {
    use super::{
        Edit, JsonIn, JsoncError, Prune, Ranged, SegBuf, ast, build, dotted, parse, quote, render,
    };

    pub(super) fn apply(text: &str, edit: &Edit) -> Result<Option<String>, JsoncError> {
        match edit {
            Edit::Set { path, value } => set(text, path, value, false),
            Edit::Push { path, value } => set(text, path, value, true),
            Edit::Remove { path, prune } => remove(text, path, *prune),
        }
    }

    fn root_object(text: &str) -> Result<ast::Value<'_>, JsoncError> {
        match parse(text)? {
            Some(root @ ast::Value::Object(_)) => Ok(root),
            _ => Err(JsoncError::NotAnObject { at: String::new() }),
        }
    }

    /// The deepest existing node along `path` and how many segments it took.
    /// A key into a non-object or an index into a non-array is an error.
    fn descend<'a, 'b>(
        root: &'b ast::Value<'a>,
        path: &[SegBuf],
    ) -> Result<(usize, &'b ast::Value<'a>), JsoncError> {
        let mut node = root;
        for (depth, seg) in path.iter().enumerate() {
            let next = match (seg, node) {
                (SegBuf::Key(k), ast::Value::Object(obj)) => obj
                    .properties
                    .iter()
                    .rev()
                    .find(|p| p.name.as_str() == k)
                    .map(|p| &p.value),
                (SegBuf::Index(i), ast::Value::Array(arr)) => arr.elements.get(*i),
                (SegBuf::Key(_), _) => {
                    return Err(JsoncError::NotAnObject {
                        at: dotted(&path[..depth]),
                    });
                }
                (SegBuf::Index(_), _) => {
                    return Err(JsoncError::NotAnArray {
                        at: dotted(&path[..depth]),
                    });
                }
            };
            match next {
                Some(n) => node = n,
                None => return Ok((depth, node)),
            }
        }
        Ok((path.len(), node))
    }

    fn set(
        text: &str,
        path: &[SegBuf],
        value: &JsonIn,
        push: bool,
    ) -> Result<Option<String>, JsoncError> {
        let root = root_object(text)?;
        let (depth, node) = descend(&root, path)?;
        let style = Style::of(text, &root);
        if depth == path.len() {
            if push {
                if !matches!(node, ast::Value::Array(_)) {
                    return Err(JsoncError::NotAnArray { at: dotted(path) });
                }
                return Ok(Some(insert(text, node, None, value, &style)));
            }
            let current: serde_json::Value = node.clone().into();
            if current == value.to_value() {
                return Ok(None);
            }
            let indent = line_indent(text, node.start());
            let rendered = render(value, indent, &style.unit, style.nl);
            return Ok(Some(apply_splices(
                text,
                vec![(node.start(), node.end(), rendered)],
            )));
        }
        let SegBuf::Key(key) = &path[depth] else {
            return Err(JsoncError::IndexOutOfRange {
                at: dotted(&path[..=depth]),
            });
        };
        let leaf = if push {
            JsonIn::Array(vec![value.clone()])
        } else {
            value.clone()
        };
        let built = build(&path[..=depth], &path[depth + 1..], leaf)?;
        Ok(Some(insert(text, node, Some(key), &built, &style)))
    }

    fn remove(text: &str, path: &[SegBuf], prune: Prune) -> Result<Option<String>, JsoncError> {
        let Some((last, parent_path)) = path.split_last() else {
            return Ok(None);
        };
        // A broken file is an error; a well-formed one without our value,
        // even one whose top level is not an object, is no change.
        let Some(root @ ast::Value::Object(_)) = parse(text)? else {
            return Ok(None);
        };
        let Ok((depth, parent)) = descend(&root, parent_path) else {
            return Ok(None);
        };
        if depth < parent_path.len() {
            return Ok(None);
        }
        let index = match (last, parent) {
            (SegBuf::Key(k), ast::Value::Object(obj)) => {
                obj.properties.iter().rposition(|p| p.name.as_str() == k)
            }
            (SegBuf::Index(i), ast::Value::Array(arr)) => (*i < arr.elements.len()).then_some(*i),
            _ => None,
        };
        let Some(index) = index else {
            return Ok(None);
        };
        let after = remove_item(text, parent, index);
        if prune == Prune::Emptied && !parent_path.is_empty() {
            let root = root_object(&after)?;
            if let Ok((d, node)) = descend(&root, parent_path)
                && d == parent_path.len()
                && items(node).is_empty()
            {
                return Ok(Some(
                    remove(&after, parent_path, prune)?.unwrap_or(after.clone()),
                ));
            }
        }
        Ok(Some(after))
    }

    /// The indentation and newline kind new text copies from the file.
    pub(super) struct Style {
        unit: String,
        nl: &'static str,
    }

    impl Style {
        fn of(text: &str, root: &ast::Value<'_>) -> Style {
            let nl = match text.find('\n') {
                Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
                _ => "\n",
            };
            Style {
                unit: indent_unit(text, root),
                nl,
            }
        }
    }

    fn indent_unit(text: &str, root: &ast::Value<'_>) -> String {
        if let ast::Value::Object(obj) = root
            && let Some(first) = obj.properties.first()
            && starts_line(text, first.name.start())
        {
            let base = line_indent(text, root.start());
            if let Some(unit) = line_indent(text, first.name.start()).strip_prefix(base)
                && !unit.is_empty()
            {
                return unit.to_string();
            }
        }
        for line in text.lines() {
            let body = line.trim_start_matches([' ', '\t']);
            let lead = &line[..line.len() - body.len()];
            if !lead.is_empty() && !body.trim().is_empty() {
                return if lead.starts_with('\t') {
                    "\t".to_string()
                } else {
                    lead.to_string()
                };
            }
        }
        "  ".to_string()
    }

    /// The (start, end) byte span of each member or element: from the key
    /// (or the element) to the end of its value.
    fn items(container: &ast::Value<'_>) -> Vec<(usize, usize)> {
        match container {
            ast::Value::Object(obj) => obj
                .properties
                .iter()
                .map(|p| (p.name.start(), p.value.end()))
                .collect(),
            ast::Value::Array(arr) => arr.elements.iter().map(|e| (e.start(), e.end())).collect(),
            _ => Vec::new(),
        }
    }

    /// Insert a member (`key: Some`) or an element (`key: None`) as the last
    /// one of `container`. [`remove_item`] deletes exactly this text again.
    fn insert(
        text: &str,
        container: &ast::Value<'_>,
        key: Option<&str>,
        value: &JsonIn,
        style: &Style,
    ) -> String {
        let nl = style.nl;
        let entry = |indent: &str| {
            let rendered = render(value, indent, &style.unit, nl);
            match key {
                Some(k) => format!("{}: {rendered}", quote(k)),
                None => rendered,
            }
        };
        let open = container.start();
        let close = container.end() - 1;
        let present = items(container);
        let Some(&(last_start, last_end)) = present.last() else {
            let after_open = open + 1;
            let ws_end = ws_forward(text, after_open);
            let base = line_indent(text, open);
            let indent = format!("{base}{}", style.unit);
            let splice = if text[after_open..ws_end].contains('\n') {
                (after_open, format!("{nl}{indent}{}{nl}", entry(&indent)))
            } else {
                (ws_end, format!("{nl}{indent}{}{nl}{base}", entry(&indent)))
            };
            return apply_splices(text, vec![(splice.0, splice.0, splice.1)]);
        };
        let gap = scan_gap(text, last_end, close);
        let indent = line_indent(text, last_start);
        let mut splices = Vec::new();
        let mut at = match gap.comma {
            Some(comma) => comma + 1,
            None => {
                splices.push((last_end, last_end, ",".to_string()));
                last_end
            }
        };
        let trailing = if gap.comma.is_some() { "," } else { "" };
        if starts_line(text, last_start) {
            at = past_same_line_comments(text, at, close);
            splices.push((at, at, format!("{nl}{indent}{}{trailing}", entry(indent))));
        } else {
            splices.push((at, at, format!(" {}{trailing}", entry(indent))));
        }
        apply_splices(text, splices)
    }

    /// Delete one member or element. For the last item this is the inverse
    /// of [`insert`].
    fn remove_item(text: &str, container: &ast::Value<'_>, index: usize) -> String {
        let present = items(container);
        let close = container.end() - 1;
        let (start, end) = present[index];
        let bound = present.get(index + 1).map(|n| n.0).unwrap_or(close);
        let own_comma = scan_gap(text, end, bound).comma;
        let deletions: Vec<(usize, usize)> = if present.len() == 1 {
            // The only item: the whitespace before it from its first line
            // break, and after it one line break when more than one follows
            // (the container was split over lines before), else all of it.
            let end = own_comma.map(|c| c + 1).unwrap_or(end);
            let before = ws_back(text, start);
            let from = match text[before..start].find('\n') {
                Some(i) if i > 0 && text.as_bytes()[before + i - 1] == b'\r' => before + i - 1,
                Some(i) => before + i,
                None => before,
            };
            let ws_end = ws_forward(text, end);
            let run = &text[end..ws_end];
            let to = if run.matches('\n').count() >= 2 {
                end + run.find('\n').expect("counted") + 1
            } else {
                ws_end
            };
            vec![(from, to)]
        } else if index + 1 == present.len() {
            let prev_end = present[index - 1].1;
            let gap = scan_gap(text, prev_end, start);
            if !gap.has_comment {
                vec![(prev_end, end)]
            } else {
                let run = ws_back(text, start);
                match (own_comma, gap.comma) {
                    (Some(own), _) => vec![(run, own + 1)],
                    (None, Some(comma)) => vec![(comma, comma + 1), (run, end)],
                    (None, None) => vec![(run, end)],
                }
            }
        } else {
            let to = own_comma.map(|c| ws_forward(text, c + 1)).unwrap_or(end);
            vec![(start, to)]
        };
        apply_splices(
            text,
            deletions
                .into_iter()
                .map(|(a, b)| (a, b, String::new()))
                .collect(),
        )
    }

    /// Apply (start, end, replacement) splices given in text order; two at
    /// the same position land in the order given.
    fn apply_splices(text: &str, mut splices: Vec<(usize, usize, String)>) -> String {
        splices.sort_by_key(|s| s.0);
        let mut out = text.to_string();
        for (start, end, with) in splices.into_iter().rev() {
            out.replace_range(start..end, &with);
        }
        out
    }

    struct Gap {
        comma: Option<usize>,
        has_comment: bool,
    }

    /// Scan text between two values, which holds only whitespace, comments
    /// and at most one comma.
    fn scan_gap(text: &str, from: usize, to: usize) -> Gap {
        let bytes = text.as_bytes();
        let mut gap = Gap {
            comma: None,
            has_comment: false,
        };
        let mut i = from;
        while i < to {
            match bytes[i] {
                b',' => {
                    gap.comma.get_or_insert(i);
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    gap.has_comment = true;
                    i = text[i..].find('\n').map(|n| i + n).unwrap_or(text.len());
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    gap.has_comment = true;
                    i = text[i + 2..]
                        .find("*/")
                        .map(|n| i + 2 + n + 2)
                        .unwrap_or(text.len());
                }
                _ => i += 1,
            }
        }
        gap
    }

    /// Past the comments that follow `at` on the same line, so a new member
    /// lands after them; trailing blanks with no comment after them stay
    /// where they are.
    fn past_same_line_comments(text: &str, at: usize, limit: usize) -> usize {
        let bytes = text.as_bytes();
        let mut i = at;
        let mut settled = at;
        while i < limit {
            match bytes[i] {
                b' ' | b'\t' => i += 1,
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    let line_end = text[i..].find('\n').map(|n| i + n).unwrap_or(text.len());
                    i = if line_end > i && bytes[line_end - 1] == b'\r' {
                        line_end - 1
                    } else {
                        line_end
                    };
                    settled = i;
                    break;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    i = text[i + 2..]
                        .find("*/")
                        .map(|n| i + 2 + n + 2)
                        .unwrap_or(text.len());
                    settled = i;
                }
                _ => break,
            }
        }
        settled
    }

    fn is_ws(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\r' | b'\n')
    }

    fn ws_back(text: &str, pos: usize) -> usize {
        let bytes = text.as_bytes();
        let mut i = pos;
        while i > 0 && is_ws(bytes[i - 1]) {
            i -= 1;
        }
        i
    }

    fn ws_forward(text: &str, pos: usize) -> usize {
        let bytes = text.as_bytes();
        let mut i = pos;
        while i < bytes.len() && is_ws(bytes[i]) {
            i += 1;
        }
        i
    }

    fn line_start(text: &str, pos: usize) -> usize {
        text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_indent(text: &str, pos: usize) -> &str {
        let start = line_start(text, pos);
        let line = &text[start..];
        let body = line.trim_start_matches([' ', '\t']);
        &line[..line.len() - body.len()]
    }

    fn starts_line(text: &str, pos: usize) -> bool {
        text[line_start(text, pos)..pos]
            .bytes()
            .all(|b| b == b' ' || b == b'\t')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(id: &str) -> JsonIn {
        JsonIn::Object(vec![
            ("command".into(), JsonIn::Str("crystalline".into())),
            (
                "args".into(),
                JsonIn::Array(vec![
                    JsonIn::Str("mcp".into()),
                    JsonIn::Str("--harness".into()),
                    JsonIn::Str(id.into()),
                ]),
            ),
        ])
    }
    fn key_path() -> Vec<SegBuf> {
        vec![
            SegBuf::Key("mcpServers".into()),
            SegBuf::Key("crystalline".into()),
        ]
    }
    fn install(text: Option<&str>) -> String {
        apply_edits(
            text,
            &[Edit::Set {
                path: key_path(),
                value: ours("gemini"),
            }],
        )
        .unwrap()
        .expect("a change")
    }
    fn uninstall(text: &str) -> String {
        apply_edits(
            Some(text),
            &[Edit::Remove {
                path: key_path(),
                prune: Prune::Emptied,
            }],
        )
        .unwrap()
        .expect("a change")
    }

    /// Review focus 1: every user file comes back byte for byte.
    #[test]
    fn uninstall_after_install_gives_back_every_byte() {
        let cases: &[(&str, &str)] = &[
            ("empty object", "{}"),
            ("empty object, newline", "{}\n"),
            ("empty object, split", "{\n}\n"),
            (
                "no mcpServers, 2 spaces",
                "{\n  \"theme\": \"Default\"\n}\n",
            ),
            (
                "other server, 4 spaces",
                "{\n    \"mcpServers\": {\n        \"other\": { \"command\": \"x\" }\n    }\n}\n",
            ),
            (
                "tabs",
                "{\n\t\"mcpServers\": {\n\t\t\"other\": {}\n\t}\n}\n",
            ),
            (
                "crlf",
                "{\r\n  \"mcpServers\": {\r\n    \"other\": {}\r\n  }\r\n}\r\n",
            ),
            (
                "no trailing newline",
                "{\n  \"mcpServers\": {\n    \"other\": {}\n  }\n}",
            ),
            (
                "line comment",
                "{\n  // servers I use\n  \"mcpServers\": {\n    \"other\": {} // keep\n  }\n}\n",
            ),
            (
                "block comment",
                "{\n  /* a */ \"mcpServers\": { /* b */ \"other\": {} }\n}\n",
            ),
            (
                "trailing commas",
                "{\n  \"mcpServers\": {\n    \"other\": {},\n  },\n}\n",
            ),
            (
                "cursor mcp fixture",
                include_str!("../tests/fixtures/harness/cursor-mcp.json"),
            ),
            (
                "kiro mcp fixture",
                include_str!("../tests/fixtures/harness/kiro-mcp.json"),
            ),
            (
                "gemini fixture",
                include_str!("../tests/fixtures/harness/gemini-settings.json"),
            ),
            (
                "gemini commented",
                include_str!("../tests/fixtures/harness/gemini-settings-commented.jsonc"),
            ),
            (
                "qwen crlf",
                include_str!("../tests/fixtures/harness/qwen-settings-crlf.json"),
            ),
        ];
        for (name, original) in cases {
            let installed = install(Some(original));
            assert_eq!(
                get(
                    &installed,
                    &[Seg::Key("mcpServers"), Seg::Key("crystalline")]
                )
                .unwrap(),
                Some(ours("gemini").to_value()),
                "{name}: entry readable"
            );
            assert_eq!(
                apply_edits(
                    Some(&installed),
                    &[Edit::Set {
                        path: key_path(),
                        value: ours("gemini")
                    }]
                )
                .unwrap(),
                None,
                "{name}: a second install is no change"
            );
            assert_eq!(&uninstall(&installed), original, "{name}: byte for byte");
        }
    }

    /// Our entry first, in the middle or last: removing it leaves the others
    /// exactly as written, commas included.
    #[test]
    fn removing_our_entry_from_any_position_keeps_the_others_byte_identical() {
        let first = "{\n  \"mcpServers\": {\n    \"crystalline\": {\"command\": \"crystalline\"},\n    \"a\": {},\n    \"b\": {}\n  }\n}\n";
        let middle = "{\n  \"mcpServers\": {\n    \"a\": {},\n    \"crystalline\": {\"command\": \"crystalline\"},\n    \"b\": {}\n  }\n}\n";
        let last = "{\n  \"mcpServers\": {\n    \"a\": {},\n    \"b\": {},\n    \"crystalline\": {\"command\": \"crystalline\"}\n  }\n}\n";
        let want = "{\n  \"mcpServers\": {\n    \"a\": {},\n    \"b\": {}\n  }\n}\n";
        for text in [first, middle, last] {
            assert_eq!(uninstall(text), want, "{text}");
        }
    }

    /// Decision 7: a file that does not exist is created with exactly our
    /// entry, our key order, 2 spaces and a trailing newline.
    #[test]
    fn a_missing_file_is_created_with_exactly_our_entry() {
        assert_eq!(
            install(None),
            "{\n  \"mcpServers\": {\n    \"crystalline\": {\n      \"command\": \"crystalline\",\n      \"args\": [\"mcp\", \"--harness\", \"gemini\"]\n    }\n  }\n}\n"
        );
    }

    /// Decision 5: the one case that cannot come back, named so nobody
    /// claims byte identity for it.
    #[test]
    fn a_pre_existing_empty_container_is_the_one_documented_exception() {
        let original = "{\n  \"mcpServers\": {}\n}\n";
        let back = uninstall(&install(Some(original)));
        assert_ne!(
            back, original,
            "the empty mcpServers the user had is pruned with ours"
        );
        assert_eq!(parse_value(&back).unwrap(), serde_json::json!({}));
        assert_eq!(back, "{}\n", "the exact bytes the editor leaves");
    }

    #[test]
    fn a_group_pushed_beside_a_foreign_one_comes_out_byte_for_byte() {
        let original = include_str!("../tests/fixtures/harness/gemini-settings.json");
        let group = JsonIn::Object(vec![
            ("matcher".into(), JsonIn::Str("startup".into())),
            (
                "hooks".into(),
                JsonIn::Array(vec![JsonIn::Object(vec![
                    ("type".into(), JsonIn::Str("command".into())),
                    (
                        "command".into(),
                        JsonIn::Str(
                            "crystalline prompt system --format hook-specific --harness gemini"
                                .into(),
                        ),
                    ),
                    ("timeout".into(), JsonIn::Int(10000)),
                ])]),
            ),
        ]);
        let at = vec![
            SegBuf::Key("hooks".into()),
            SegBuf::Key("SessionStart".into()),
        ];
        let pushed = apply_edits(
            Some(original),
            &[Edit::Push {
                path: at.clone(),
                value: group,
            }],
        )
        .unwrap()
        .unwrap();
        let v = parse_value(&pushed).unwrap();
        assert_eq!(
            v["hooks"]["SessionStart"].as_array().unwrap().len(),
            2,
            "appended, not replaced"
        );
        let mut idx = at.clone();
        idx.push(SegBuf::Index(1));
        let back = apply_edits(
            Some(&pushed),
            &[Edit::Remove {
                path: idx,
                prune: Prune::Emptied,
            }],
        )
        .unwrap()
        .unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn a_push_into_a_file_without_hooks_creates_the_path_and_is_undone_by_prune() {
        let original = "{\n  \"theme\": \"Default\"\n}\n";
        let at = vec![
            SegBuf::Key("hooks".into()),
            SegBuf::Key("SessionStart".into()),
        ];
        let pushed = apply_edits(
            Some(original),
            &[Edit::Push {
                path: at.clone(),
                value: JsonIn::Object(vec![]),
            }],
        )
        .unwrap()
        .unwrap();
        let mut idx = at;
        idx.push(SegBuf::Index(0));
        assert_eq!(
            apply_edits(
                Some(&pushed),
                &[Edit::Remove {
                    path: idx,
                    prune: Prune::Emptied
                }]
            )
            .unwrap()
            .unwrap(),
            original
        );
    }

    #[test]
    fn broken_files_are_refused_with_a_position() {
        let set = [Edit::Set {
            path: key_path(),
            value: ours("gemini"),
        }];
        match apply_edits(Some("{\n  \"mcpServers\": {\n    \"a\": \n"), &set).unwrap_err() {
            JsoncError::Parse { line, column, .. } => {
                assert!(line >= 3 && column >= 1, "{line}:{column}")
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(apply_edits(Some("[1, 2]"), &set).unwrap_err(), JsoncError::NotAnObject { at } if at.is_empty())
        );
        assert!(
            matches!(apply_edits(Some("{\"mcpServers\": []}"), &set).unwrap_err(), JsoncError::NotAnObject { at } if at == "mcpServers")
        );
    }

    #[test]
    fn an_edit_elsewhere_survives_uninstall() {
        let installed = install(Some("{\n  \"theme\": \"Default\"\n}\n"));
        let edited = installed.replace("\"Default\"", "\"Dracula\"");
        assert_eq!(uninstall(&edited), "{\n  \"theme\": \"Dracula\"\n}\n");
    }

    #[test]
    fn the_cursor_hooks_files_take_a_session_start_entry_and_give_it_back() {
        for original in [
            include_str!("../tests/fixtures/harness/cursor-hooks.json"),
            include_str!("../tests/fixtures/harness/cursor-hooks-commented.jsonc"),
        ] {
            let at = vec![
                SegBuf::Key("hooks".into()),
                SegBuf::Key("sessionStart".into()),
            ];
            let entry = JsonIn::Object(vec![(
                "command".into(),
                JsonIn::Str("/opt/homebrew/bin/crystalline prompt system --format cursor".into()),
            )]);
            let pushed = apply_edits(
                Some(original),
                &[Edit::Push {
                    path: at.clone(),
                    value: entry,
                }],
            )
            .unwrap()
            .unwrap();
            let v = parse_value(&pushed).unwrap();
            assert_eq!(v["hooks"]["sessionStart"].as_array().unwrap().len(), 2);
            assert_eq!(v["version"], 1);
            let mut idx = at;
            idx.push(SegBuf::Index(1));
            let back = apply_edits(
                Some(&pushed),
                &[Edit::Remove {
                    path: idx,
                    prune: Prune::Emptied,
                }],
            )
            .unwrap()
            .unwrap();
            assert_eq!(back, original);
        }
    }

    #[test]
    fn inserted_text_copies_the_indent_and_the_newline_kind() {
        let crlf = install(Some(include_str!(
            "../tests/fixtures/harness/qwen-settings-crlf.json"
        )));
        assert!(
            !crlf.replace("\r\n", "").contains('\n'),
            "only CRLF: {crlf:?}"
        );
        let tabs = install(Some("{\n\t\"mcpServers\": {\n\t\t\"other\": {}\n\t}\n}\n"));
        assert!(
            tabs.contains("\t\t\"crystalline\": {\n\t\t\t\"command\": \"crystalline\",\n"),
            "{tabs:?}"
        );
    }

    /// A same-line comment after the last member stays on that member's
    /// line, and our entry goes on the next line.
    #[test]
    fn a_line_comment_after_the_last_member_stays_with_it() {
        let installed = install(Some(
            "{\n  \"mcpServers\": {\n    \"other\": {} // keep\n  }\n}\n",
        ));
        assert_eq!(
            installed,
            "{\n  \"mcpServers\": {\n    \"other\": {}, // keep\n    \"crystalline\": {\n      \"command\": \"crystalline\",\n      \"args\": [\"mcp\", \"--harness\", \"gemini\"]\n    }\n  }\n}\n"
        );
    }

    /// An object the person split over two lines comes back split; the
    /// price is one blank line while our entry is in it.
    #[test]
    fn a_split_empty_object_gets_a_blank_line_that_marks_it() {
        assert_eq!(
            install(Some("{\n}\n")),
            "{\n  \"mcpServers\": {\n    \"crystalline\": {\n      \"command\": \"crystalline\",\n      \"args\": [\"mcp\", \"--harness\", \"gemini\"]\n    }\n  }\n\n}\n"
        );
    }

    #[test]
    fn an_older_entry_is_replaced_in_place() {
        let original = "{\n  \"mcpServers\": {\n    \"crystalline\": {\"command\": \"/old/crystalline\"},\n    \"b\": {}\n  }\n}\n";
        assert_eq!(
            install(Some(original)),
            "{\n  \"mcpServers\": {\n    \"crystalline\": {\n      \"command\": \"crystalline\",\n      \"args\": [\"mcp\", \"--harness\", \"gemini\"]\n    },\n    \"b\": {}\n  }\n}\n"
        );
    }

    #[test]
    fn removing_what_is_not_there_is_no_change_but_a_broken_file_is_an_error() {
        let remove = [Edit::Remove {
            path: key_path(),
            prune: Prune::Emptied,
        }];
        assert_eq!(apply_edits(Some("{\"theme\": 1}"), &remove).unwrap(), None);
        assert_eq!(apply_edits(None, &remove).unwrap(), None);
        assert_eq!(apply_edits(Some("[1]"), &remove).unwrap(), None);
        assert!(matches!(
            apply_edits(Some("{\"theme\": "), &remove).unwrap_err(),
            JsoncError::Parse { line: 1, .. }
        ));
    }

    #[test]
    fn a_push_into_a_value_that_is_not_an_array_is_refused() {
        let push = [Edit::Push {
            path: vec![
                SegBuf::Key("hooks".into()),
                SegBuf::Key("SessionStart".into()),
            ],
            value: JsonIn::Null,
        }];
        assert!(matches!(
            apply_edits(Some("{\"hooks\": {\"SessionStart\": {}}}"), &push).unwrap_err(),
            JsoncError::NotAnArray { at } if at == "hooks.SessionStart"
        ));
    }
}
