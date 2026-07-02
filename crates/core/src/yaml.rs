//! An order-preserving YAML value model.
//!
//! Engram frontmatter must round-trip with unknown keys kept in their
//! original order. The chosen YAML crate (`serde_yaml_ng`) already backs its
//! mapping with an [`IndexMap`], so order survives a parse. `YamlValue` is a
//! small owned mirror of that value so the public API of `crystalline-core`
//! does not leak the YAML crate: callers work with `YamlValue`, and if the
//! YAML backend is ever swapped the public surface stays stable.
//!
//! Emission stays deterministic by converting back to the backend value and
//! serializing through it, which reproduces the backend's canonical scalar
//! quoting (dates and RFC 3339 timestamps unquoted, ambiguous scalars like
//! `true` or `42` single-quoted).

use indexmap::IndexMap;
use serde::Serialize;

/// A YAML value with mapping key order preserved.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum YamlValue {
    /// A YAML null.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer scalar.
    Int(i64),
    /// A floating point scalar.
    Float(f64),
    /// A string scalar.
    String(String),
    /// A block or flow sequence.
    Sequence(Vec<YamlValue>),
    /// A mapping, keys in original order.
    Mapping(IndexMap<String, YamlValue>),
}

impl YamlValue {
    /// Borrow the value as a string if it is a string scalar.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            YamlValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow the value as a mapping if it is one.
    pub fn as_mapping(&self) -> Option<&IndexMap<String, YamlValue>> {
        match self {
            YamlValue::Mapping(m) => Some(m),
            _ => None,
        }
    }

    /// Borrow the value as a sequence if it is one.
    pub fn as_sequence(&self) -> Option<&[YamlValue]> {
        match self {
            YamlValue::Sequence(s) => Some(s),
            _ => None,
        }
    }

    /// Look up a nested value by dotted path (`a.b.c`). Traverses mappings.
    pub fn get_path(&self, path: &str) -> Option<&YamlValue> {
        let mut current = self;
        for segment in path.split('.') {
            current = current.as_mapping()?.get(segment)?;
        }
        Some(current)
    }

    /// Convert from the backend YAML value, preserving mapping order.
    pub(crate) fn from_backend(value: serde_yaml_ng::Value) -> YamlValue {
        use serde_yaml_ng::Value as V;
        match value {
            V::Null => YamlValue::Null,
            V::Bool(b) => YamlValue::Bool(b),
            V::Number(n) => {
                if let Some(i) = n.as_i64() {
                    YamlValue::Int(i)
                } else if let Some(u) = n.as_u64() {
                    // Values above i64::MAX are rare in frontmatter; keep them
                    // representable rather than losing the key entirely.
                    i64::try_from(u)
                        .map(YamlValue::Int)
                        .unwrap_or_else(|_| YamlValue::Float(u as f64))
                } else {
                    YamlValue::Float(n.as_f64().unwrap_or(0.0))
                }
            }
            V::String(s) => YamlValue::String(s),
            V::Sequence(seq) => {
                YamlValue::Sequence(seq.into_iter().map(YamlValue::from_backend).collect())
            }
            V::Mapping(map) => {
                let mut out = IndexMap::with_capacity(map.len());
                for (k, v) in map {
                    let key = backend_key_to_string(&k);
                    out.insert(key, YamlValue::from_backend(v));
                }
                YamlValue::Mapping(out)
            }
            V::Tagged(tagged) => YamlValue::from_backend(tagged.value),
        }
    }

    /// Convert back to the backend YAML value for deterministic emission.
    pub(crate) fn to_backend(&self) -> serde_yaml_ng::Value {
        use serde_yaml_ng::Value as V;
        match self {
            YamlValue::Null => V::Null,
            YamlValue::Bool(b) => V::Bool(*b),
            YamlValue::Int(i) => V::Number((*i).into()),
            YamlValue::Float(f) => V::Number((*f).into()),
            YamlValue::String(s) => V::String(s.clone()),
            YamlValue::Sequence(seq) => {
                V::Sequence(seq.iter().map(YamlValue::to_backend).collect())
            }
            YamlValue::Mapping(map) => {
                let mut out = serde_yaml_ng::Mapping::new();
                for (k, v) in map {
                    out.insert(V::String(k.clone()), v.to_backend());
                }
                V::Mapping(out)
            }
        }
    }
}

fn backend_key_to_string(key: &serde_yaml_ng::Value) -> String {
    match key {
        serde_yaml_ng::Value::String(s) => s.clone(),
        serde_yaml_ng::Value::Bool(b) => b.to_string(),
        serde_yaml_ng::Value::Number(n) => n.to_string(),
        serde_yaml_ng::Value::Null => "null".to_string(),
        other => serde_yaml_ng::to_string(other)
            .unwrap_or_default()
            .trim_end()
            .to_string(),
    }
}
