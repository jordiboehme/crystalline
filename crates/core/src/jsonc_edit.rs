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
    /// The file is valid, but the edit at this dotted path (or these paths,
    /// joined by ", ") could not be applied without changing something
    /// else in it, so nothing is written. Never the person's fault.
    Unsafe { at: String },
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
            JsoncError::Unsafe { at } => write!(
                f,
                "Crystalline could not change \"{at}\" without touching other parts of this file's layout, so nothing was written. The file is unchanged; make this change by hand"
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
    Ok(parse(without_bom(text).1)?
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
    let (bom, text) = match text {
        Some(t) => {
            let (bom, rest) = without_bom(t);
            (bom, Some(rest))
        }
        None => ("", None),
    };
    let Some(original) = text.filter(|t| !t.trim().is_empty()) else {
        let mut root = JsonIn::Object(Vec::new());
        let mut changed = false;
        for edit in edits {
            changed |= tree::apply(&mut root, edit)?;
        }
        let empty = matches!(&root, JsonIn::Object(m) if m.is_empty());
        return Ok((changed && !empty).then(|| render_new(&root)));
    };
    // A parse error here is the person's file; past this point the file is
    // known to be valid, so a parse error can only come from a splice.
    parse(original)?;
    let mut current = original.to_string();
    for edit in edits {
        let next = splice::apply(&current, edit).map_err(|e| match e {
            JsoncError::Parse { .. } => JsoncError::Unsafe {
                at: dotted(edit_path(edit)),
            },
            other => other,
        })?;
        if let Some(next) = next {
            verify(&current, &next, edit)?;
            current = next;
        }
    }
    Ok((current != original).then(|| format!("{bom}{current}")))
}

/// Split off a leading UTF-8 byte order mark, which some editors write.
fn without_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

fn edit_path(edit: &Edit) -> &[SegBuf] {
    match edit {
        Edit::Set { path, .. } | Edit::Push { path, .. } | Edit::Remove { path, .. } => path,
    }
}

/// What an edit may do to the containers on its path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Set or Push: missing parents may be created.
    Write,
    /// Remove: with `Prune::Emptied` a parent left empty may go.
    Remove(Prune),
}

/// The post-condition of the splice editor (spec 3.3), checked after every
/// single edit. The result must parse to an object; along the edited path
/// every other key and every other array element must be equal; a parent
/// may appear only for a Set or Push and vanish only for a pruning Remove,
/// and then only when nothing else was in it; and the edit itself must have
/// taken effect (the set value reads back, the pushed element is last, the
/// removed key is gone). Anything else means a splice rule went wrong or
/// the file holds duplicate keys, and the edit is refused.
fn verify(before_text: &str, after_text: &str, edit: &Edit) -> Result<(), JsoncError> {
    let refuse = || JsoncError::Unsafe {
        at: dotted(edit_path(edit)),
    };
    let before = parse_value(before_text).map_err(|_| refuse())?;
    let after = match parse(after_text) {
        Ok(Some(root @ ast::Value::Object(_))) => serde_json::Value::from(root),
        _ => return Err(refuse()),
    };
    let ok = match edit {
        Edit::Set { path, value } => {
            outside_equal(Some(&before), Some(&after), path, Change::Write)
                && lookup(&after, path) == Some(&value.to_value())
        }
        Edit::Push { path, value } => {
            outside_equal(Some(&before), Some(&after), path, Change::Write)
                && pushed(lookup(&before, path), lookup(&after, path), value)
        }
        Edit::Remove { path, prune } => {
            outside_equal(Some(&before), Some(&after), path, Change::Remove(*prune))
                && removed(&after, path)
        }
    };
    if ok { Ok(()) } else { Err(refuse()) }
}

fn lookup<'a>(value: &'a serde_json::Value, path: &[SegBuf]) -> Option<&'a serde_json::Value> {
    path.iter().try_fold(value, |node, seg| match seg {
        SegBuf::Key(k) => node.as_object()?.get(k),
        SegBuf::Index(i) => node.as_array()?.get(*i),
    })
}

/// Everything beside `path` is equal in `before` and `after`, level by level.
fn outside_equal(
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
    path: &[SegBuf],
    change: Change,
) -> bool {
    let Some((seg, rest)) = path.split_first() else {
        return true;
    };
    if before.is_none() && after.is_none() {
        return true;
    }
    if before.is_none() && change != Change::Write {
        return false;
    }
    if after.is_none() && change != Change::Remove(Prune::Emptied) {
        return false;
    }
    match seg {
        SegBuf::Key(key) => {
            type Map = serde_json::Map<String, serde_json::Value>;
            fn as_map(v: Option<&serde_json::Value>) -> Option<Option<&Map>> {
                match v {
                    None => Some(None),
                    Some(serde_json::Value::Object(map)) => Some(Some(map)),
                    Some(_) => None,
                }
            }
            fn others<'m>(
                m: Option<&'m Map>,
                key: &str,
            ) -> Vec<(&'m String, &'m serde_json::Value)> {
                m.map(|m| m.iter().filter(|(k, _)| *k != key).collect())
                    .unwrap_or_default()
            }
            let (Some(b), Some(a)) = (as_map(before), as_map(after)) else {
                return false;
            };
            others(b, key) == others(a, key)
                && outside_equal(
                    b.and_then(|m| m.get(key)),
                    a.and_then(|m| m.get(key)),
                    rest,
                    change,
                )
        }
        SegBuf::Index(i) => {
            fn as_items(v: Option<&serde_json::Value>) -> Option<&[serde_json::Value]> {
                match v {
                    None => Some(&[]),
                    Some(serde_json::Value::Array(items)) => Some(items.as_slice()),
                    Some(_) => None,
                }
            }
            let (Some(b), Some(a)) = (as_items(before), as_items(after)) else {
                return false;
            };
            if rest.is_empty() && matches!(change, Change::Remove(_)) {
                // The element at the index goes; the rest keeps its order.
                if *i >= b.len() {
                    return false;
                }
                let mut expected = b.to_vec();
                expected.remove(*i);
                return expected == a;
            }
            b.len() == a.len()
                && b.iter()
                    .zip(a)
                    .enumerate()
                    .all(|(j, (x, y))| j == *i || x == y)
                && outside_equal(b.get(*i), a.get(*i), rest, change)
        }
    }
}

/// The array after a Push is the array before plus the pushed element.
fn pushed(
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
    value: &JsonIn,
) -> bool {
    let before: &[serde_json::Value] = match before {
        None => &[],
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => return false,
    };
    match after {
        Some(serde_json::Value::Array(items)) => {
            items.len() == before.len() + 1
                && items[..before.len()] == *before
                && items.last() == Some(&value.to_value())
        }
        _ => false,
    }
}

/// A removed key no longer reads back (a duplicate key would still).
fn removed(after: &serde_json::Value, path: &[SegBuf]) -> bool {
    match path.split_last() {
        Some((SegBuf::Key(key), parent)) => lookup(after, parent)
            .and_then(serde_json::Value::as_object)
            .is_none_or(|map| !map.contains_key(key)),
        _ => true,
    }
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
            // A container that still holds the person's comments is not
            // empty: it stays, comments and all.
            if let Ok((d, node)) = descend(&root, parent_path)
                && d == parent_path.len()
                && items(node).is_empty()
                && !scan_gap(&after, node.start() + 1, node.end() - 1).has_comment
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
        let prev_bound = match index {
            0 => container.start() + 1,
            i => present[i - 1].1,
        };
        let gap_before = scan_gap(text, prev_bound, start);
        let before = ws_back(text, start);
        // The item sits right after a comment. The line break after that
        // comment is the comment's own (a `//` comment ends there), so it is
        // never deleted; the item's line goes with the break after it.
        let after_comment = gap_before.last_comment_end == Some(before);
        let first_break_after = |from: usize| line_break_end(&text[from..start]).map(|i| from + i);
        let deletions: Vec<(usize, usize)> = if present.len() == 1 {
            // The only item: the whitespace before it from its first line
            // break, and after it one line break when more than one follows
            // (the container was split over lines before), else all of it.
            let end = own_comma.map(|c| c + 1).unwrap_or(end);
            let ws_end = ws_forward(text, end);
            let run = &text[end..ws_end];
            let through_first_break = run.find('\n').map(|i| end + i + 1);
            match first_break_after(before) {
                Some(from) if after_comment => vec![(from, through_first_break.unwrap_or(ws_end))],
                _ => {
                    let from = match text[before..start].find('\n') {
                        Some(i) if i > 0 && text.as_bytes()[before + i - 1] == b'\r' => {
                            before + i - 1
                        }
                        Some(i) => before + i,
                        None => before,
                    };
                    let to = if run.matches('\n').count() >= 2 {
                        through_first_break.expect("counted")
                    } else {
                        ws_end
                    };
                    vec![(from, to)]
                }
            }
        } else if index + 1 == present.len() {
            let prev_end = present[index - 1].1;
            if !gap_before.has_comment {
                vec![(prev_end, end)]
            } else {
                let item_end = own_comma.map(|c| c + 1).unwrap_or(end);
                let line_end = past_blank_line_end(text, item_end);
                // A block comment with the close after the item on the same
                // line (`"a": 1 /* x */ }`) had our line break put after it by
                // `insert`: that break is ours and goes again. A `//` comment's
                // break, or one before an item that ends its own line, stays.
                let keep_break =
                    after_comment && (gap_before.last_comment_is_line || line_end > item_end);
                let (from, to) = match first_break_after(before) {
                    Some(from) if keep_break => (from, line_end),
                    _ => (before, item_end),
                };
                match (own_comma, gap_before.comma) {
                    (None, Some(comma)) => vec![(comma, comma + 1), (from, to)],
                    _ => vec![(from, to)],
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
        /// Where the last comment in the gap ends, when nothing but
        /// whitespace follows it: a `//` comment ends before its line
        /// break (before the `\r` of a CRLF), a block comment after `*/`.
        last_comment_end: Option<usize>,
        /// The last comment is a `//` comment.
        last_comment_is_line: bool,
    }

    /// The offset just past the first line break in `s`: LF, CRLF or a lone
    /// CR, which also ends a `//` comment for the parser.
    fn line_break_end(s: &str) -> Option<usize> {
        let bytes = s.as_bytes();
        bytes.iter().enumerate().find_map(|(i, b)| match b {
            b'\n' => Some(i + 1),
            b'\r' if bytes.get(i + 1) != Some(&b'\n') => Some(i + 1),
            _ => None,
        })
    }

    /// Past the blanks and the line break that end the line at `at`, when
    /// nothing else follows on that line; else `at` itself.
    fn past_blank_line_end(text: &str, at: usize) -> usize {
        let rest = &text[at..];
        let blank = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let after = &rest[blank..];
        if after.starts_with("\r\n") {
            at + blank + 2
        } else if after.starts_with(['\n', '\r']) {
            at + blank + 1
        } else {
            at
        }
    }

    /// Scan text between two values, which holds only whitespace, comments
    /// and at most one comma.
    fn scan_gap(text: &str, from: usize, to: usize) -> Gap {
        let bytes = text.as_bytes();
        let mut gap = Gap {
            comma: None,
            has_comment: false,
            last_comment_end: None,
            last_comment_is_line: false,
        };
        let mut i = from;
        while i < to {
            match bytes[i] {
                b',' => {
                    gap.comma.get_or_insert(i);
                    gap.last_comment_end = None;
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    gap.has_comment = true;
                    // A `//` comment ends at LF, at the CR of a CRLF or at a
                    // lone CR, as the parser reads it.
                    i = text[i..]
                        .find(['\n', '\r'])
                        .map(|n| i + n)
                        .unwrap_or(text.len());
                    gap.last_comment_end = Some(i);
                    gap.last_comment_is_line = true;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    gap.has_comment = true;
                    i = text[i + 2..]
                        .find("*/")
                        .map(|n| i + 2 + n + 2)
                        .unwrap_or(text.len());
                    gap.last_comment_end = Some(i);
                    gap.last_comment_is_line = false;
                }
                b if is_ws(b) => i += 1,
                _ => {
                    gap.last_comment_end = None;
                    i += 1;
                }
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
                    i = text[i..]
                        .find(['\n', '\r'])
                        .map(|n| i + n)
                        .unwrap_or(text.len());
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

    fn remove_never(text: &str, path: Vec<SegBuf>) -> String {
        apply_edits(
            Some(text),
            &[Edit::Remove {
                path,
                prune: Prune::Never,
            }],
        )
        .unwrap()
        .expect("a change")
    }

    fn key(k: &str) -> SegBuf {
        SegBuf::Key(k.into())
    }

    /// Review t2, critical 1: the person deletes the other server, and
    /// uninstall must not pull the closing brace into the comment above.
    #[test]
    fn the_reviews_cursor_repro_uninstalls_to_valid_json() {
        let installed = install(Some(include_str!(
            "../tests/fixtures/harness/cursor-mcp.json"
        )));
        let edited = remove_never(&installed, vec![key("mcpServers"), key("other")]);
        assert!(
            edited.starts_with(
                "{\n    // servers this person set up by hand\n    \"mcpServers\": {\n        \"crystalline\": {"
            ),
            "{edited:?}"
        );
        let back = uninstall(&edited);
        assert_eq!(back, "{\n    // servers this person set up by hand\n}\n");
        assert_eq!(parse_value(&back).unwrap(), serde_json::json!({}));
    }

    /// The only item and the last item after a line or a block comment, in
    /// LF and in CRLF: the comment keeps its line and the result parses.
    #[test]
    fn a_removal_after_a_comment_keeps_the_comment_on_its_own_line() {
        let cases: &[(&str, Vec<SegBuf>, &str)] = &[
            (
                "{\n  // c\n  \"crystalline\": {}\n}\n",
                vec![key("crystalline")],
                "{\n  // c\n}\n",
            ),
            (
                "{\r\n  // c\r\n  \"crystalline\": {}\r\n}\r\n",
                vec![key("crystalline")],
                "{\r\n  // c\r\n}\r\n",
            ),
            (
                "{\n  /* c */\n  \"crystalline\": {}\n}\n",
                vec![key("crystalline")],
                "{\n  /* c */\n}\n",
            ),
            (
                "{\r\n  /* c */\r\n  \"crystalline\": {}\r\n}\r\n",
                vec![key("crystalline")],
                "{\r\n  /* c */\r\n}\r\n",
            ),
            (
                "{\n  \"m\": {\n    // c\n    \"crystalline\": {}\n  }\n}\n",
                vec![key("m"), key("crystalline")],
                "{\n  \"m\": {\n    // c\n  }\n}\n",
            ),
            (
                "{\"h\": [\n  // mine\n  {}\n]}",
                vec![key("h"), SegBuf::Index(0)],
                "{\"h\": [\n  // mine\n]}",
            ),
            (
                "{\"a\": 1, // note\n    \"crystalline\": {} }",
                vec![key("crystalline")],
                "{\"a\": 1 // note\n }",
            ),
            (
                "{\"a\": 1, // note\r\n    \"crystalline\": {} }",
                vec![key("crystalline")],
                "{\"a\": 1 // note\r\n }",
            ),
            (
                "{\"a\": 1, /* note */\n    \"crystalline\": {} }",
                vec![key("crystalline")],
                "{\"a\": 1 /* note */ }",
            ),
            (
                "{\n  \"a\": 1, // note\n  \"crystalline\": {},\n}\n",
                vec![key("crystalline")],
                "{\n  \"a\": 1, // note\n}\n",
            ),
        ];
        for (text, path, want) in cases {
            let got = remove_never(text, path.clone());
            assert_eq!(&got, want, "{text:?}");
            parse_value(&got).unwrap_or_else(|e| panic!("{got:?}: {e}"));
        }
    }

    /// The documented exception with a comment beside the empty container:
    /// the container goes, the comment and valid JSON stay.
    #[test]
    fn the_exception_under_a_comment_leaves_the_comment_and_valid_json() {
        for (original, want) in [
            (
                "{\n  // MCP servers for Cursor\n  \"mcpServers\": {}\n}\n",
                "{\n  // MCP servers for Cursor\n}\n",
            ),
            (
                "{\r\n  // MCP servers for Cursor\r\n  \"mcpServers\": {}\r\n}\r\n",
                "{\r\n  // MCP servers for Cursor\r\n}\r\n",
            ),
        ] {
            assert_eq!(uninstall(&install(Some(original))), want);
        }
    }

    /// Review t2, important 1: a container that holds the person's comments
    /// is not empty, so the prune keeps it and the file comes back.
    #[test]
    fn a_container_with_only_comments_is_not_pruned() {
        for original in [
            "{\n  \"mcpServers\": {\n    // mine go here\n  }\n}\n",
            "{\r\n  \"mcpServers\": {\r\n    // mine go here\r\n  }\r\n}\r\n",
            "{\n  \"mcpServers\": { /* mine */ }\n}\n",
            "{\n  \"theme\": 1,\n  \"mcpServers\": {\n    /* later */\n  }\n}",
        ] {
            assert_eq!(
                uninstall(&install(Some(original))),
                original,
                "{original:?}"
            );
        }
    }

    /// Spec 3.3: the result is parsed again and every key outside the edit
    /// must be unchanged, or nothing is written.
    #[test]
    fn a_result_that_changes_anything_outside_the_edit_is_refused() {
        let set = [Edit::Set {
            path: key_path(),
            value: ours("gemini"),
        }];
        let original = "{\n  \"theme\": \"Default\"\n}\n";
        let good = install(Some(original));
        assert_eq!(verify(original, &good, &set[0]), Ok(()));
        for bad in [
            good.replace("Default", "Dracula"),
            good.replace("\"theme\": \"Default\",\n", ""),
            good.replace("}\n}\n", "}\n"),
        ] {
            assert!(
                matches!(
                    verify(original, &bad, &set[0]),
                    Err(JsoncError::Unsafe { .. })
                ),
                "{bad:?}"
            );
        }
        let message = JsoncError::Unsafe {
            at: "mcpServers.crystalline".into(),
        }
        .to_string();
        assert!(message.contains("nothing was written"), "{message}");
        assert!(!message.contains("column"), "{message}");
    }

    /// Review t2, minor 1: a leading byte order mark is kept and is not
    /// reported as broken JSON.
    #[test]
    fn a_byte_order_mark_is_kept_and_not_read_as_broken_json() {
        let original = "\u{feff}{\n  \"theme\": \"Default\"\n}\n";
        let installed = install(Some(original));
        assert!(
            installed.starts_with("\u{feff}{\n  \"theme\""),
            "{installed:?}"
        );
        assert_eq!(
            get(&installed, &[Seg::Key("theme")]).unwrap(),
            Some(serde_json::json!("Default"))
        );
        assert_eq!(uninstall(&installed), original);
    }

    /// Every node's raw text by dotted path.
    fn raw_nodes(text: &str) -> std::collections::BTreeMap<String, String> {
        fn walk(
            text: &str,
            node: &ast::Value<'_>,
            at: &str,
            out: &mut std::collections::BTreeMap<String, String>,
        ) {
            let join = |seg: &str| {
                if at.is_empty() {
                    seg.to_string()
                } else {
                    format!("{at}.{seg}")
                }
            };
            match node {
                ast::Value::Object(obj) => {
                    for p in &obj.properties {
                        let path = join(p.name.as_str());
                        out.insert(
                            path.clone(),
                            text[p.value.start()..p.value.end()].to_string(),
                        );
                        walk(text, &p.value, &path, out);
                    }
                }
                ast::Value::Array(arr) => {
                    for (i, e) in arr.elements.iter().enumerate() {
                        let path = join(&i.to_string());
                        out.insert(path.clone(), text[e.start()..e.end()].to_string());
                        walk(text, e, &path, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = std::collections::BTreeMap::new();
        let root = parse(text).unwrap().expect("a value");
        walk(text, &root, "", &mut out);
        out
    }

    /// Every node of `original` that is not on the way to a touched path and
    /// not under one has the same bytes in `output`.
    fn assert_foreign_kept(name: &str, original: &str, output: &str, touched: &[String]) {
        let after = raw_nodes(output);
        for (path, raw) in raw_nodes(original) {
            let related = touched.iter().any(|t| {
                t == &path
                    || t.starts_with(&format!("{path}."))
                    || path.starts_with(&format!("{t}."))
            });
            if !related {
                assert_eq!(after.get(&path), Some(&raw), "{name}: {path}");
            }
        }
    }

    /// Property-style: for every fixture and every install and uninstall
    /// sequence, the output parses and every node outside our entries keeps
    /// its bytes; the person deleting their own entries in between still
    /// leaves an uninstall that parses.
    #[test]
    fn every_fixture_survives_every_install_sequence() {
        let fixtures: &[(&str, &str, &str)] = &[
            (
                "cursor hooks",
                include_str!("../tests/fixtures/harness/cursor-hooks.json"),
                "sessionStart",
            ),
            (
                "cursor hooks commented",
                include_str!("../tests/fixtures/harness/cursor-hooks-commented.jsonc"),
                "sessionStart",
            ),
            (
                "cursor mcp",
                include_str!("../tests/fixtures/harness/cursor-mcp.json"),
                "sessionStart",
            ),
            (
                "gemini",
                include_str!("../tests/fixtures/harness/gemini-settings.json"),
                "SessionStart",
            ),
            (
                "gemini commented",
                include_str!("../tests/fixtures/harness/gemini-settings-commented.jsonc"),
                "SessionStart",
            ),
            (
                "kiro mcp",
                include_str!("../tests/fixtures/harness/kiro-mcp.json"),
                "SessionStart",
            ),
            (
                "qwen crlf",
                include_str!("../tests/fixtures/harness/qwen-settings-crlf.json"),
                "SessionStart",
            ),
            (
                "comments everywhere",
                "{\n  // a\n  \"theme\": 1, // b\n  /* c */\n  \"mcpServers\": { // d\n    \"other\": {} /* e */\n  },\n  \"hooks\": {\n    // f\n  }\n}\n",
                "SessionStart",
            ),
            (
                "block comment before the close",
                "{\n  \"a\": 1 /* x */ }\n",
                "SessionStart",
            ),
            (
                "block comment before the close, crlf",
                "{\r\n  \"a\": 1 /* x */ }\r\n",
                "SessionStart",
            ),
            (
                "block comment before the close, trailing comma",
                "{\n  \"a\": 1, /* x */ }\n",
                "SessionStart",
            ),
            (
                "block comment before the close, nested",
                "{\n  \"mcpServers\": {\n    \"o\": 1 /* x */ }\n}\n",
                "SessionStart",
            ),
            (
                "multi-line block comment before the close",
                "{\n  \"a\": 1 /* x\n */ }\n",
                "SessionStart",
            ),
            (
                "hooks array with a block comment before the close",
                "{\"hooks\": {\"SessionStart\": [\n  {\"x\": 1} /* mine */ ]}}",
                "SessionStart",
            ),
            (
                "hooks array with three groups",
                "{\n  \"hooks\": {\n    \"SessionStart\": [\n      {\"a\": 1},\n      {\"b\": 2}, // two\n      {\"c\": 3}\n    ]\n  }\n}\n",
                "SessionStart",
            ),
        ];
        let mcp = key_path();
        for (name, original, event) in fixtures {
            let hook_path = vec![key("hooks"), key(event)];
            let existing = parse_value(original).unwrap()["hooks"][event]
                .as_array()
                .map_or(0, Vec::len);
            let mut hook_item = hook_path.clone();
            hook_item.push(SegBuf::Index(existing));
            let hook_touched = format!("hooks.{event}.{existing}");
            let group = JsonIn::Object(vec![("command".into(), JsonIn::Str("crystalline".into()))]);
            let set_mcp = Edit::Set {
                path: mcp.clone(),
                value: ours("gemini"),
            };
            let push_hook = Edit::Push {
                path: hook_path.clone(),
                value: group,
            };
            let remove_mcp = Edit::Remove {
                path: mcp.clone(),
                prune: Prune::Emptied,
            };
            let remove_hook = Edit::Remove {
                path: hook_item.clone(),
                prune: Prune::Emptied,
            };
            let touched = vec!["mcpServers.crystalline".to_string(), hook_touched];

            for installs in [
                vec![set_mcp.clone()],
                vec![push_hook.clone()],
                vec![set_mcp.clone(), push_hook.clone()],
                vec![push_hook.clone(), set_mcp.clone()],
            ] {
                let installed = apply_edits(Some(original), &installs)
                    .unwrap_or_else(|e| panic!("{name}: {e}"))
                    .expect("a change");
                parse_value(&installed).unwrap();
                assert_foreign_kept(name, original, &installed, &touched);
                if installs.contains(&set_mcp) {
                    assert_eq!(
                        apply_edits(Some(&installed), std::slice::from_ref(&set_mcp)).unwrap(),
                        None,
                        "{name}: a second install is no change"
                    );
                }
                for uninstalls in [
                    vec![remove_mcp.clone(), remove_hook.clone()],
                    vec![remove_hook.clone(), remove_mcp.clone()],
                ] {
                    let back = apply_edits(Some(&installed), &uninstalls)
                        .unwrap_or_else(|e| panic!("{name}: {e}"))
                        .expect("a change");
                    assert_eq!(&back, original, "{name}: {installs:?} then {uninstalls:?}");
                }
            }

            // The person deletes their own entries while ours stay, then
            // uninstall runs: whatever is left must still parse.
            let installed = apply_edits(Some(original), &[set_mcp.clone(), push_hook.clone()])
                .unwrap()
                .unwrap();
            let value = parse_value(&installed).unwrap();
            let mut theirs: Vec<Vec<SegBuf>> = Vec::new();
            for (k, v) in value.as_object().unwrap() {
                if k == "mcpServers" || k == "hooks" {
                    for inner in v
                        .as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                    {
                        if inner != "crystalline" && inner != *event {
                            theirs.push(vec![key(k), key(&inner)]);
                        }
                    }
                    if k == "hooks" {
                        for i in (0..existing).rev() {
                            let mut p = hook_path.clone();
                            p.push(SegBuf::Index(i));
                            theirs.push(p);
                        }
                    }
                } else {
                    theirs.push(vec![key(k)]);
                }
            }
            let mut text = installed;
            for path in theirs {
                if let Some(next) = apply_edits(
                    Some(&text),
                    &[Edit::Remove {
                        path,
                        prune: Prune::Never,
                    }],
                )
                .unwrap_or_else(|e| panic!("{name}: {e}"))
                {
                    text = next;
                }
            }
            parse_value(&text).unwrap_or_else(|e| panic!("{name}: {text:?}: {e}"));
            let back = apply_edits(Some(&text), &[remove_hook.clone(), remove_mcp.clone()])
                .unwrap_or_else(|e| panic!("{name}: {text:?}: {e}"));
            let back = back.unwrap_or(text);
            let value = parse_value(&back).unwrap_or_else(|e| panic!("{name}: {back:?}: {e}"));
            assert!(
                value
                    .get("mcpServers")
                    .is_none_or(|m| m.get("crystalline").is_none()),
                "{name}"
            );
        }
    }

    /// Re-review N1: a block comment after the last member with the close on
    /// the same line belongs to that member; our line break after it is
    /// ours and goes again on uninstall.
    #[test]
    fn a_block_comment_before_the_close_round_trips() {
        for original in [
            "{\n  \"a\": 1 /* x */ }\n",
            "{\r\n  \"a\": 1 /* x */ }\r\n",
            "{\n  \"a\": 1, /* x */ }\n",
            "{\n  \"mcpServers\": {\n    \"o\": 1 /* x */ }\n}\n",
            "{\n  \"a\": 1 /* x\n */ }\n",
        ] {
            assert_eq!(
                uninstall(&install(Some(original))),
                original,
                "{original:?}"
            );
        }
        let original = "{\"h\": [\n  {\"x\": 1} /* mine */ ]}";
        let pushed = apply_edits(
            Some(original),
            &[Edit::Push {
                path: vec![key("h")],
                value: JsonIn::Object(vec![]),
            }],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            remove_never(&pushed, vec![key("h"), SegBuf::Index(1)]),
            original
        );
    }

    /// Re-review N2: inside an array only the element the edit names may
    /// change; every other element is compared.
    #[test]
    fn verify_compares_every_array_element_outside_the_edit() {
        let original = "{\"h\": {\"S\": [1, 2, 3]}}";
        let remove = Edit::Remove {
            path: vec![key("h"), key("S"), SegBuf::Index(0)],
            prune: Prune::Never,
        };
        assert_eq!(
            verify(original, "{\"h\": {\"S\": [2, 3]}}", &remove),
            Ok(())
        );
        for bad in [
            "{\"h\": {\"S\": [9]}}",
            "{\"h\": {\"S\": [3, 2]}}",
            "{\"h\": {\"S\": [1, 2, 3]}}",
        ] {
            assert!(
                matches!(
                    verify(original, bad, &remove),
                    Err(JsoncError::Unsafe { .. })
                ),
                "{bad}"
            );
        }
        let push = Edit::Push {
            path: vec![key("h"), key("S")],
            value: JsonIn::Int(4),
        };
        assert_eq!(
            verify(original, "{\"h\": {\"S\": [1, 2, 3, 4]}}", &push),
            Ok(())
        );
        for bad in [
            "{\"h\": {\"S\": [1, 2, 4]}}",
            "{\"h\": {\"S\": [9, 2, 3, 4]}}",
            "{\"h\": {\"S\": [1, 2, 3, 5]}}",
        ] {
            assert!(
                matches!(verify(original, bad, &push), Err(JsoncError::Unsafe { .. })),
                "{bad}"
            );
        }
        let set = Edit::Set {
            path: vec![key("h"), key("S"), SegBuf::Index(1)],
            value: JsonIn::Int(7),
        };
        assert_eq!(
            verify(original, "{\"h\": {\"S\": [1, 7, 3]}}", &set),
            Ok(())
        );
        assert!(matches!(
            verify(original, "{\"h\": {\"S\": [0, 7, 3]}}", &set),
            Err(JsoncError::Unsafe { .. })
        ));
    }

    /// Re-review N3: the edit itself must have taken effect, and a prune
    /// may only take containers when the removal asked for it.
    #[test]
    fn verify_checks_that_the_edit_took_effect() {
        let set = Edit::Set {
            path: key_path(),
            value: JsonIn::Int(1),
        };
        assert!(matches!(
            verify("{}", "{\"mcpServers\": {\"crystalline\": 2}}", &set),
            Err(JsoncError::Unsafe { .. })
        ));
        assert!(matches!(
            verify(
                "{\"m\": {}, \"x\": 1}",
                "{\"x\": 1, \"mcpServers\": {\"crystalline\": 1}}",
                &set
            ),
            Err(JsoncError::Unsafe { .. })
        ));
        let never = Edit::Remove {
            path: vec![key("m"), key("c")],
            prune: Prune::Never,
        };
        assert!(matches!(
            verify("{\"m\": {\"c\": 1}}", "{}", &never),
            Err(JsoncError::Unsafe { .. })
        ));
        assert_eq!(verify("{\"m\": {\"c\": 1}}", "{\"m\": {}}", &never), Ok(()));
        let remove = Edit::Remove {
            path: key_path(),
            prune: Prune::Emptied,
        };
        for duplicated in [
            "{\"mcpServers\": {\"crystalline\": {}}, \"mcpServers\": {\"crystalline\": {}}}",
            "{\"mcpServers\": {\"crystalline\": {\"command\": \"x\"}, \"crystalline\": {}}}",
        ] {
            assert!(
                matches!(
                    apply_edits(Some(duplicated), std::slice::from_ref(&remove)),
                    Err(JsoncError::Unsafe { .. })
                ),
                "{duplicated}"
            );
        }
    }

    /// Re-review N4: a `//` comment also ends at a lone CR, as the parser
    /// reads it.
    #[test]
    fn a_line_comment_ended_by_a_lone_cr_is_handled() {
        let text = "{\n  // c\r  \"mcpServers\": {\"crystalline\": {}}\n}\n";
        let back = uninstall(text);
        assert_eq!(parse_value(&back).unwrap(), serde_json::json!({}));
        assert!(back.contains("// c\r"), "{back:?}");
    }
}
