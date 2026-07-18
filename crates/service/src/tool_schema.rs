//! Sanitizes the JSON Schema every tool advertises over `list_tools` and
//! `get_tool` down to the conservative shape clients actually expect.
//!
//! schemars' draft-2020-12 generator (used for every `Parameters<T>` in
//! [`crate::params`]) is fully spec-compliant but produces constructs most MCP
//! extensions never emit: every `Option<T>` scalar becomes a `"type":
//! ["X","null"]` union rather than a bare `"X"`, `Option<usize>`/`Option<u8>`/
//! `Option<f32>`/`Option<f64>` fields carry non-standard `format` values
//! (`uint`, `uint8`, `float`, `double` are not part of the JSON Schema format
//! vocabulary), and the two `serde_json::Value` params (`write_engram.metadata`,
//! `search_engrams.metadata_filters`) generate a type-less schema. Field
//! reports show Claude Desktop chat mode - the one surface that uploads tool
//! schemas to the claude.ai backend, a system this codebase cannot observe or
//! debug directly - hanging without ever issuing a tool call while other
//! extensions on the same machines keep working (see
//! research/2026-07-18-desktop-chat-mode-deep-research.md). This module
//! normalizes the advertised schemas to the shape well-behaved extensions
//! produce, without touching how arguments are deserialized: serde's `Option`
//! already accepts `null` or an absent key regardless of what the schema
//! advertises, so this is purely a wire-shape fix.
//!
//! Tradeoff: a type-less schema is defaulted to `"type": "object"` (see
//! [`sanitize_schema`] step C). That is correct for the two `Value` params in
//! this codebase today, both documented as maps, but it would misrepresent a
//! future `Value` param meant to hold a bare scalar or array; such a param
//! would need a `schemars` override (`#[schemars(schema_with = ...)]`) to
//! advertise its real shape.

use std::sync::Arc;

use serde_json::{Map, Value};

/// JSON Schema `format` values this codebase advertises on purpose. Anything
/// else - including the non-standard `uint`/`uint8`/`float`/`double` that
/// schemars emits for Rust's numeric types - is stripped rather than passed
/// through, since a client that validates `format` strictly would otherwise
/// reject values schemars itself considers valid.
const FORMAT_ALLOWLIST: &[&str] = &[
    "date-time",
    "date",
    "time",
    "duration",
    "email",
    "idn-email",
    "hostname",
    "idn-hostname",
    "ipv4",
    "ipv6",
    "uri",
    "uri-reference",
    "iri",
    "iri-reference",
    "uuid",
    "uri-template",
    "json-pointer",
    "relative-json-pointer",
    "regex",
];

/// Keys whose presence means a node already carries a type in some form, so
/// step C of [`sanitize_schema`] leaves it alone rather than defaulting to
/// `object`.
const TYPE_BEARING_KEYS: &[&str] = &[
    "type", "$ref", "enum", "const", "anyOf", "oneOf", "allOf", "not",
];

/// Sanitizes both schemas a [`rmcp::model::Tool`] can carry. `input_schema`
/// is always present; `output_schema` is defensive - our tools never set one
/// today, but a future rmcp upgrade or tool could.
///
/// Both fields are `Arc<JsonObject>`, shared with rmcp's thread-local schema
/// cache: mutating through `Arc::make_mut` clones the map only if another
/// handle is live (clone-on-write), so the cached copy the router built at
/// startup is never touched, only the handle this function holds.
pub(crate) fn sanitize_tool(tool: &mut rmcp::model::Tool) {
    sanitize_schema(Arc::make_mut(&mut tool.input_schema));
    if let Some(output_schema) = &mut tool.output_schema {
        sanitize_schema(Arc::make_mut(output_schema));
    }
}

/// One recursive pass over a JSON Schema object node, applying four
/// transforms in order and then recursing into the fixed set of positions a
/// schema can nest another schema. Idempotent: running it twice on an
/// already-clean node is a no-op.
///
/// A. Type-union flattening: a `"type"` array loses its `"null"` members. One
///    member remains -> replace the array with that bare member. Several
///    remain -> keep the array, minus `"null"`. The array held only `"null"`
///    -> scalar `"type": "null"`. The array was already empty -> the `"type"`
///    key is removed outright, so step C can supply `"object"`.
/// B. Format stripping: `"format"` is removed unless its value is a string in
///    [`FORMAT_ALLOWLIST`]. Sibling keywords (`"minimum"` and similar) are
///    untouched.
/// C. Type defaulting: if the node now has none of [`TYPE_BEARING_KEYS`],
///    insert `"type": "object"` (see the module doc's tradeoff note).
/// D. Recursion into exactly: every object value under `"properties"`,
///    `"$defs"` and `"definitions"`; `"items"` when it is an object, or each
///    object element when it is an array; each object element of
///    `"prefixItems"`; `"additionalProperties"` only when it is an object
///    (the boolean form is left alone); each object element of `"anyOf"`,
///    `"oneOf"` and `"allOf"`; `"not"` when it is an object. Nothing outside
///    this list is touched, so unrelated keys such as `"description"` or
///    `"required"` pass through untouched.
fn sanitize_schema(schema: &mut Map<String, Value>) {
    // A. Flatten a "type" array: drop "null" members, then collapse to a bare
    // string, a narrowed array, "null" or nothing at all depending on what is
    // left. The key is removed up front so every branch below only has to
    // decide whether, and with what, to reinsert it.
    if matches!(schema.get("type"), Some(Value::Array(_))) {
        let Some(Value::Array(members)) = schema.remove("type") else {
            unreachable!("checked above that \"type\" is an array");
        };
        if !members.is_empty() {
            let mut kept: Vec<Value> = members
                .into_iter()
                .filter(|v| v.as_str() != Some("null"))
                .collect();
            let replacement = match kept.len() {
                0 => Value::String("null".to_string()),
                1 => kept.swap_remove(0),
                _ => Value::Array(kept),
            };
            schema.insert("type".to_string(), replacement);
        }
    }

    // B. Drop non-standard formats; a non-string format value is dropped too.
    let keep_format = schema
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|f| FORMAT_ALLOWLIST.contains(&f));
    if !keep_format {
        schema.remove("format");
    }

    // C. A node with no type-bearing keyword at all defaults to object.
    if !TYPE_BEARING_KEYS.iter().any(|k| schema.contains_key(*k)) {
        schema.insert("type".to_string(), Value::String("object".to_string()));
    }

    // D. Recurse into the fixed set of positions a schema can nest another.
    for key in ["properties", "$defs", "definitions"] {
        if let Some(Value::Object(map)) = schema.get_mut(key) {
            for value in map.values_mut() {
                if let Value::Object(nested) = value {
                    sanitize_schema(nested);
                }
            }
        }
    }
    if let Some(items) = schema.get_mut("items") {
        match items {
            Value::Object(nested) => sanitize_schema(nested),
            Value::Array(elements) => {
                for element in elements.iter_mut() {
                    if let Value::Object(nested) = element {
                        sanitize_schema(nested);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(Value::Array(elements)) = schema.get_mut("prefixItems") {
        for element in elements.iter_mut() {
            if let Value::Object(nested) = element {
                sanitize_schema(nested);
            }
        }
    }
    if let Some(Value::Object(nested)) = schema.get_mut("additionalProperties") {
        sanitize_schema(nested);
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(elements)) = schema.get_mut(key) {
            for element in elements.iter_mut() {
                if let Value::Object(nested) = element {
                    sanitize_schema(nested);
                }
            }
        }
    }
    if let Some(Value::Object(nested)) = schema.get_mut("not") {
        sanitize_schema(nested);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Builds the `Map` a schema node is represented as from a `json!` object
    /// literal, the fixture shape every test below starts from.
    fn obj(value: Value) -> Map<String, Value> {
        value
            .as_object()
            .expect("fixture must be a JSON object")
            .clone()
    }

    #[test]
    fn nullable_union_flattens_to_scalar() {
        let mut schema = obj(json!({"type": ["string", "null"]}));
        sanitize_schema(&mut schema);
        assert_eq!(schema["type"], json!("string"));

        let mut schema = obj(json!({"type": ["boolean", "null"]}));
        sanitize_schema(&mut schema);
        assert_eq!(schema["type"], json!("boolean"));
    }

    #[test]
    fn multi_member_union_keeps_array_minus_null() {
        let mut schema = obj(json!({"type": ["string", "number", "null"]}));
        sanitize_schema(&mut schema);
        assert_eq!(schema["type"], json!(["string", "number"]));
    }

    #[test]
    fn bare_null_array_becomes_scalar_null_and_empty_array_defaults_to_object() {
        let mut only_null = obj(json!({"type": ["null"]}));
        sanitize_schema(&mut only_null);
        assert_eq!(only_null["type"], json!("null"));

        let mut empty = obj(json!({"type": []}));
        sanitize_schema(&mut empty);
        assert_eq!(
            empty["type"],
            json!("object"),
            "an emptied type array is removed, then defaulted to object by step C"
        );
    }

    #[test]
    fn nonstandard_formats_are_stripped_while_standard_formats_and_siblings_survive() {
        for dirty in ["uint", "uint8", "float", "double"] {
            let mut schema = obj(json!({"type": "integer", "format": dirty, "minimum": 0}));
            sanitize_schema(&mut schema);
            assert!(
                !schema.contains_key("format"),
                "format {dirty} must be stripped"
            );
            assert_eq!(
                schema["minimum"],
                json!(0),
                "the sibling minimum keyword must survive stripping format {dirty}"
            );
        }
        for standard in ["date-time", "uuid", "email"] {
            let mut schema = obj(json!({"type": "string", "format": standard}));
            sanitize_schema(&mut schema);
            assert_eq!(
                schema["format"],
                json!(standard),
                "standard format {standard} must survive"
            );
        }
    }

    #[test]
    fn typeless_schema_defaults_to_object_but_combinators_are_exempt() {
        let mut typeless = obj(json!({"description": "x", "default": null}));
        sanitize_schema(&mut typeless);
        assert_eq!(typeless["type"], json!("object"));

        let mut ref_only = obj(json!({"$ref": "#/$defs/Foo"}));
        sanitize_schema(&mut ref_only);
        assert!(
            !ref_only.contains_key("type"),
            "a $ref-only schema must not gain a type"
        );

        let mut enum_only = obj(json!({"enum": ["a", "b"]}));
        sanitize_schema(&mut enum_only);
        assert!(
            !enum_only.contains_key("type"),
            "an enum-only schema must not gain a type"
        );

        let mut any_of_only = obj(json!({"anyOf": [{"type": "string"}, {"type": "integer"}]}));
        sanitize_schema(&mut any_of_only);
        assert!(
            !any_of_only.contains_key("type"),
            "an anyOf-only schema must not gain a type"
        );
    }

    /// One fixture exercising every documented recursion position at once:
    /// `properties`, both forms of `items`, `prefixItems`,
    /// `additionalProperties` in both its object and boolean forms, the three
    /// combinator keywords, `not` and both `$defs` and `definitions`. Each
    /// nested position holds a dirty union or format that must come out
    /// clean; the root's `description` and `required` are plain non-schema
    /// values that must pass through untouched.
    #[test]
    fn recursion_reaches_every_documented_position() {
        let mut schema = obj(json!({
            "type": "object",
            "description": "root schema",
            "required": ["a", "b"],
            "properties": {
                "a": {"type": ["string", "null"]},
                "b": {
                    "type": "array",
                    "items": [
                        {"type": ["integer", "null"]},
                        {"type": ["boolean", "null"]}
                    ]
                },
                "c": {"type": "object", "additionalProperties": false}
            },
            "items": {"type": ["number", "null"]},
            "prefixItems": [{"type": ["boolean", "null"]}],
            "additionalProperties": {"type": ["string", "null"]},
            "anyOf": [{"type": ["string", "null"]}],
            "oneOf": [{"type": ["integer", "null"]}],
            "allOf": [{"type": ["boolean", "null"]}],
            "not": {"type": ["string", "null"]},
            "$defs": {"Foo": {"type": ["string", "null"]}},
            "definitions": {"Bar": {"type": ["integer", "null"]}}
        }));

        sanitize_schema(&mut schema);

        assert_eq!(
            schema["description"],
            json!("root schema"),
            "a non-schema string key must pass through untouched"
        );
        assert_eq!(
            schema["required"],
            json!(["a", "b"]),
            "a non-schema array key must pass through untouched"
        );
        assert_eq!(schema["properties"]["a"]["type"], json!("string"));
        assert_eq!(
            schema["properties"]["b"]["items"][0]["type"],
            json!("integer")
        );
        assert_eq!(
            schema["properties"]["b"]["items"][1]["type"],
            json!("boolean")
        );
        assert_eq!(
            schema["properties"]["c"]["additionalProperties"],
            json!(false),
            "the boolean form of additionalProperties must never turn into a schema object"
        );
        assert_eq!(schema["items"]["type"], json!("number"));
        assert_eq!(schema["prefixItems"][0]["type"], json!("boolean"));
        assert_eq!(schema["additionalProperties"]["type"], json!("string"));
        assert_eq!(schema["anyOf"][0]["type"], json!("string"));
        assert_eq!(schema["oneOf"][0]["type"], json!("integer"));
        assert_eq!(schema["allOf"][0]["type"], json!("boolean"));
        assert_eq!(schema["not"]["type"], json!("string"));
        assert_eq!(schema["$defs"]["Foo"]["type"], json!("string"));
        assert_eq!(schema["definitions"]["Bar"]["type"], json!("integer"));
    }

    #[test]
    fn sanitizing_twice_is_idempotent() {
        let mut schema = obj(json!({
            "type": ["string", "null"],
            "format": "uint8",
            "properties": {
                "child": {"type": ["integer", "null"], "format": "double"}
            }
        }));
        sanitize_schema(&mut schema);
        let once = schema.clone();
        sanitize_schema(&mut schema);
        assert_eq!(
            schema, once,
            "a second pass over an already-clean schema must change nothing"
        );
    }

    #[test]
    fn sanitize_tool_cleans_both_schemas_without_disturbing_a_shared_arc_handle() {
        let dirty = obj(json!({
            "type": "object",
            "properties": {
                "count": {"type": ["integer", "null"], "format": "uint"}
            }
        }));

        let mut tool = rmcp::model::Tool::new("demo", "a demo tool", dirty.clone());
        // Held across the sanitize call: proves make_mut cloned rather than
        // mutated the map this handle still points at.
        let shared_before_sanitize = Arc::clone(&tool.input_schema);
        tool.output_schema = Some(Arc::new(dirty));

        sanitize_tool(&mut tool);

        assert_eq!(
            tool.input_schema["properties"]["count"]["type"],
            json!("integer"),
            "the advertised input schema must be sanitized"
        );
        assert!(
            !tool.input_schema["properties"]["count"]
                .as_object()
                .unwrap()
                .contains_key("format"),
            "the advertised input schema format must be stripped"
        );
        let output = tool
            .output_schema
            .as_ref()
            .expect("output schema was set before sanitizing");
        assert_eq!(output["properties"]["count"]["type"], json!("integer"));

        assert_eq!(
            shared_before_sanitize["properties"]["count"]["type"],
            json!(["integer", "null"]),
            "a handle taken before sanitizing must still see the original dirty JSON"
        );
    }
}
