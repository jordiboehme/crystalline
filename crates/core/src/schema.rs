//! The Picoschema engine: parse, validate, infer and diff.
//!
//! A schema is declared in the frontmatter of a `type: schema` Engram. The
//! body `schema:` mapping declares observation categories and relations; a
//! Capitalized type names a relation target entity, lowercase names a scalar.
//! `settings.frontmatter` declares expected frontmatter fields with the same
//! syntax. Validation never rejects on its own: severity follows
//! `settings.validation` (`warn`, `strict` or `off`).

use std::collections::BTreeSet;

use indexmap::IndexMap;
use serde::Serialize;

use crate::engram::{Engram, Frontmatter, SchemaDef};
use crate::yaml::YamlValue;

/// A lowercase scalar type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScalarType {
    /// `string`
    String,
    /// `integer`
    Integer,
    /// `number`
    Number,
    /// `boolean`
    Boolean,
    /// `any`
    Any,
}

/// The type of a schema field.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FieldType {
    /// A scalar value.
    Scalar(ScalarType),
    /// A closed set of allowed string values.
    Enum(Vec<String>),
    /// A relation target entity (a Capitalized type name).
    Entity(String),
    /// An array of the inner type.
    Array(Box<FieldType>),
    /// A nested object with its own field declarations.
    Object(Vec<FieldDecl>),
}

impl FieldType {
    /// Whether this type denotes a relation (an entity or an array of them).
    pub fn is_relation(&self) -> bool {
        match self {
            FieldType::Entity(_) => true,
            FieldType::Array(inner) => inner.is_relation(),
            _ => false,
        }
    }
}

/// A single field declaration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldDecl {
    /// The field name (observation category, relation type or frontmatter key).
    pub name: String,
    /// Whether the field is optional (a trailing `?`).
    pub optional: bool,
    /// The declared type.
    pub field_type: FieldType,
    /// An optional trailing description.
    pub description: Option<String>,
}

/// How strongly validation issues are surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValidationMode {
    /// Issues are warnings.
    #[default]
    Warn,
    /// Issues are errors.
    Strict,
    /// Validation is disabled.
    Off,
}

/// A parsed Picoschema.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Schema {
    /// The entity type this schema governs.
    pub entity: Option<String>,
    /// Schema version.
    pub version: Option<i64>,
    /// Body field declarations (observations and relations).
    pub fields: Vec<FieldDecl>,
    /// Validation mode.
    pub validation: ValidationMode,
    /// Frontmatter field declarations.
    pub frontmatter: Vec<FieldDecl>,
}

/// Severity of a schema validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    /// A warning.
    Warning,
    /// An error.
    Error,
}

/// The kind of a schema validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SchemaIssueKind {
    /// A required observation category was absent.
    MissingRequiredObservation,
    /// A required relation was absent.
    MissingRequiredRelation,
    /// An observation value was outside its enum.
    ObservationEnumViolation,
    /// An observation value did not match its scalar type.
    ObservationTypeMismatch,
    /// A required frontmatter field was absent.
    MissingRequiredFrontmatter,
    /// A frontmatter value was outside its enum.
    FrontmatterEnumViolation,
    /// A frontmatter value did not match its scalar type.
    FrontmatterTypeMismatch,
}

/// A single schema validation issue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SchemaIssue {
    /// Severity, derived from the schema's validation mode.
    pub severity: IssueSeverity,
    /// The kind of issue.
    pub kind: SchemaIssueKind,
    /// The field, category, relation type or frontmatter path involved.
    pub field: String,
    /// A human-readable message.
    pub message: String,
    /// The source line, when the issue points at a body element.
    pub line: Option<usize>,
}

/// Drift between a schema and a set of Engrams.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SchemaDrift {
    /// Observation categories present in Engrams but not declared.
    pub undeclared_observations: Vec<String>,
    /// Relation types present in Engrams but not declared.
    pub undeclared_relations: Vec<String>,
    /// Declared observation categories never present in any Engram.
    pub unused_observations: Vec<String>,
    /// Declared relations never present in any Engram.
    pub unused_relations: Vec<String>,
}

impl Schema {
    /// Build a schema from a schema Engram's frontmatter block.
    pub fn from_schema_def(def: &SchemaDef) -> Schema {
        let fields = parse_declarations(&def.schema);
        let validation = def
            .settings
            .get("validation")
            .and_then(YamlValue::as_str)
            .map(parse_validation_mode)
            .unwrap_or_default();
        let frontmatter = def
            .settings
            .get("frontmatter")
            .and_then(YamlValue::as_mapping)
            .map(parse_declarations)
            .unwrap_or_default();
        Schema {
            entity: def.entity.clone(),
            version: def.version,
            fields,
            validation,
            frontmatter,
        }
    }

    /// Build a schema from an Engram, if it carries a schema definition.
    pub fn from_engram(engram: &Engram) -> Option<Schema> {
        engram
            .frontmatter
            .schema_def
            .as_ref()
            .map(Schema::from_schema_def)
    }
}

fn parse_validation_mode(s: &str) -> ValidationMode {
    match s.trim().to_lowercase().as_str() {
        "strict" => ValidationMode::Strict,
        "off" => ValidationMode::Off,
        _ => ValidationMode::Warn,
    }
}

fn parse_declarations(map: &IndexMap<String, YamlValue>) -> Vec<FieldDecl> {
    map.iter()
        .map(|(key, value)| parse_declaration(key, value))
        .collect()
}

fn parse_declaration(key: &str, value: &YamlValue) -> FieldDecl {
    let (name, optional, modifier) = parse_decl_key(key);
    let (field_type, description) = match modifier.as_deref() {
        Some("array") => {
            let (inner, desc) = type_from_value(value);
            (FieldType::Array(Box::new(inner)), desc)
        }
        Some("enum") => (FieldType::Enum(enum_values(value)), None),
        Some("object") => {
            let nested = value
                .as_mapping()
                .map(parse_declarations)
                .unwrap_or_default();
            (FieldType::Object(nested), None)
        }
        _ => type_from_value(value),
    };
    FieldDecl {
        name,
        optional,
        field_type,
        description,
    }
}

fn parse_decl_key(key: &str) -> (String, bool, Option<String>) {
    let key = key.trim();
    let (head, modifier) = match key.split_once('(') {
        Some((h, rest)) => {
            let modi = rest.trim_end_matches(')').trim().to_string();
            (h, Some(modi))
        }
        None => (key, None),
    };
    let head = head.trim();
    let (name, optional) = match head.strip_suffix('?') {
        Some(n) => (n.trim().to_string(), true),
        None => (head.to_string(), false),
    };
    (name, optional, modifier)
}

fn type_from_value(value: &YamlValue) -> (FieldType, Option<String>) {
    match value {
        YamlValue::String(s) => {
            let (type_name, desc) = match s.split_once(',') {
                Some((t, d)) => (t.trim(), Some(d.trim().to_string())),
                None => (s.trim(), None),
            };
            (type_from_name(type_name), desc)
        }
        _ => (FieldType::Scalar(ScalarType::Any), None),
    }
}

fn type_from_name(name: &str) -> FieldType {
    match name {
        "string" => FieldType::Scalar(ScalarType::String),
        "integer" => FieldType::Scalar(ScalarType::Integer),
        "number" => FieldType::Scalar(ScalarType::Number),
        "boolean" => FieldType::Scalar(ScalarType::Boolean),
        "any" => FieldType::Scalar(ScalarType::Any),
        other if other.chars().next().is_some_and(|c| c.is_uppercase()) => {
            FieldType::Entity(other.to_string())
        }
        _ => FieldType::Scalar(ScalarType::Any),
    }
}

fn enum_values(value: &YamlValue) -> Vec<String> {
    match value {
        YamlValue::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

// --- validation --------------------------------------------------------------

/// Validate an Engram against a schema. Returns an empty vector when the schema
/// disables validation.
pub fn validate(engram: &Engram, schema: &Schema) -> Vec<SchemaIssue> {
    if schema.validation == ValidationMode::Off {
        return Vec::new();
    }
    let severity = if schema.validation == ValidationMode::Strict {
        IssueSeverity::Error
    } else {
        IssueSeverity::Warning
    };
    let mut issues = Vec::new();

    for field in &schema.fields {
        if field.field_type.is_relation() {
            validate_relation(engram, field, severity, &mut issues);
        } else {
            validate_observation(engram, field, severity, &mut issues);
        }
    }

    let fm_map = frontmatter_map(&engram.frontmatter);
    for decl in &schema.frontmatter {
        validate_frontmatter(&fm_map, decl, "", severity, &mut issues);
    }

    issues
}

fn validate_relation(
    engram: &Engram,
    field: &FieldDecl,
    severity: IssueSeverity,
    issues: &mut Vec<SchemaIssue>,
) {
    if field.optional {
        return;
    }
    let present = engram.relations.iter().any(|r| r.rel_type == field.name);
    if !present {
        issues.push(SchemaIssue {
            severity,
            kind: SchemaIssueKind::MissingRequiredRelation,
            field: field.name.clone(),
            message: format!("required relation `{}` is missing", field.name),
            line: None,
        });
    }
}

fn validate_observation(
    engram: &Engram,
    field: &FieldDecl,
    severity: IssueSeverity,
    issues: &mut Vec<SchemaIssue>,
) {
    let matching: Vec<_> = engram
        .observations
        .iter()
        .filter(|o| o.category == field.name)
        .collect();

    if !field.optional && matching.is_empty() {
        issues.push(SchemaIssue {
            severity,
            kind: SchemaIssueKind::MissingRequiredObservation,
            field: field.name.clone(),
            message: format!("required observation `{}` is missing", field.name),
            line: None,
        });
        return;
    }

    let element_type = match &field.field_type {
        FieldType::Array(inner) => inner.as_ref(),
        other => other,
    };

    for obs in matching {
        match element_type {
            FieldType::Enum(values) => {
                if !values.iter().any(|v| v == &obs.content) {
                    issues.push(SchemaIssue {
                        severity,
                        kind: SchemaIssueKind::ObservationEnumViolation,
                        field: field.name.clone(),
                        message: format!(
                            "observation `{}` value `{}` is not one of [{}]",
                            field.name,
                            obs.content,
                            values.join(", ")
                        ),
                        line: Some(obs.line),
                    });
                }
            }
            FieldType::Scalar(scalar) if !str_matches_scalar(&obs.content, *scalar) => {
                issues.push(SchemaIssue {
                    severity,
                    kind: SchemaIssueKind::ObservationTypeMismatch,
                    field: field.name.clone(),
                    message: format!(
                        "observation `{}` value `{}` is not a valid {:?}",
                        field.name, obs.content, scalar
                    ),
                    line: Some(obs.line),
                });
            }
            _ => {}
        }
    }
}

fn validate_frontmatter(
    map: &IndexMap<String, YamlValue>,
    decl: &FieldDecl,
    prefix: &str,
    severity: IssueSeverity,
    issues: &mut Vec<SchemaIssue>,
) {
    let path = if prefix.is_empty() {
        decl.name.clone()
    } else {
        format!("{prefix}.{}", decl.name)
    };
    let value = map.get(&decl.name);

    let Some(value) = value else {
        if !decl.optional {
            issues.push(SchemaIssue {
                severity,
                kind: SchemaIssueKind::MissingRequiredFrontmatter,
                field: path.clone(),
                message: format!("required frontmatter field `{path}` is missing"),
                line: None,
            });
        }
        return;
    };

    match &decl.field_type {
        FieldType::Enum(values) => {
            let as_str = value.as_str().map(str::to_string).unwrap_or_default();
            if !values.iter().any(|v| v == &as_str) {
                issues.push(SchemaIssue {
                    severity,
                    kind: SchemaIssueKind::FrontmatterEnumViolation,
                    field: path.clone(),
                    message: format!(
                        "frontmatter `{path}` value `{as_str}` is not one of [{}]",
                        values.join(", ")
                    ),
                    line: None,
                });
            }
        }
        FieldType::Scalar(scalar) => {
            if !value_matches_scalar(value, *scalar) {
                issues.push(SchemaIssue {
                    severity,
                    kind: SchemaIssueKind::FrontmatterTypeMismatch,
                    field: path.clone(),
                    message: format!("frontmatter `{path}` is not a valid {scalar:?}"),
                    line: None,
                });
            }
        }
        FieldType::Object(nested) => {
            if let Some(inner) = value.as_mapping() {
                for child in nested {
                    validate_frontmatter(inner, child, &path, severity, issues);
                }
            }
        }
        FieldType::Array(inner) => {
            if let (Some(seq), FieldType::Scalar(scalar)) = (value.as_sequence(), inner.as_ref()) {
                for item in seq {
                    if !value_matches_scalar(item, *scalar) {
                        issues.push(SchemaIssue {
                            severity,
                            kind: SchemaIssueKind::FrontmatterTypeMismatch,
                            field: path.clone(),
                            message: format!(
                                "frontmatter array `{path}` has a non-{scalar:?} item"
                            ),
                            line: None,
                        });
                    }
                }
            }
        }
        FieldType::Entity(_) => {}
    }
}

fn str_matches_scalar(s: &str, scalar: ScalarType) -> bool {
    match scalar {
        ScalarType::String | ScalarType::Any => true,
        ScalarType::Integer => s.trim().parse::<i64>().is_ok(),
        ScalarType::Number => s.trim().parse::<f64>().is_ok(),
        ScalarType::Boolean => matches!(s.trim(), "true" | "false"),
    }
}

fn value_matches_scalar(v: &YamlValue, scalar: ScalarType) -> bool {
    match scalar {
        ScalarType::String => matches!(v, YamlValue::String(_)),
        ScalarType::Any => true,
        ScalarType::Integer => match v {
            YamlValue::Int(_) => true,
            YamlValue::String(s) => s.trim().parse::<i64>().is_ok(),
            _ => false,
        },
        ScalarType::Number => match v {
            YamlValue::Int(_) | YamlValue::Float(_) => true,
            YamlValue::String(s) => s.trim().parse::<f64>().is_ok(),
            _ => false,
        },
        ScalarType::Boolean => match v {
            YamlValue::Bool(_) => true,
            YamlValue::String(s) => matches!(s.trim(), "true" | "false"),
            _ => false,
        },
    }
}

/// Represent an Engram's frontmatter as a YAML mapping for validation lookups.
fn frontmatter_map(fm: &Frontmatter) -> IndexMap<String, YamlValue> {
    let mut m = IndexMap::new();
    if !fm.engram_type.is_empty() {
        m.insert("type".into(), YamlValue::String(fm.engram_type.clone()));
    }
    if !fm.title.is_empty() {
        m.insert("title".into(), YamlValue::String(fm.title.clone()));
    }
    if let Some(v) = &fm.permalink {
        m.insert("permalink".into(), YamlValue::String(v.clone()));
    }
    if let Some(v) = &fm.description {
        m.insert("description".into(), YamlValue::String(v.clone()));
    }
    if let Some(v) = &fm.resource {
        m.insert("resource".into(), YamlValue::String(v.clone()));
    }
    if let Some(v) = &fm.status {
        m.insert("status".into(), YamlValue::String(v.clone()));
    }
    if let Some(v) = &fm.temporal_confidence {
        m.insert("temporal_confidence".into(), YamlValue::String(v.clone()));
    }
    if !fm.tags.is_empty() {
        m.insert(
            "tags".into(),
            YamlValue::Sequence(fm.tags.iter().cloned().map(YamlValue::String).collect()),
        );
    }
    let date = |d: chrono::NaiveDate| YamlValue::String(d.format("%Y-%m-%d").to_string());
    if let Some(d) = fm.recorded_at {
        m.insert("recorded_at".into(), date(d));
    }
    if let Some(d) = fm.valid_from {
        m.insert("valid_from".into(), date(d));
    }
    if let Some(d) = fm.valid_to {
        m.insert("valid_to".into(), date(d));
    }
    if let Some(d) = fm.source_date {
        m.insert("source_date".into(), date(d));
    }
    if let Some(d) = fm.last_verified {
        m.insert("last_verified".into(), date(d));
    }
    if let Some(d) = fm.review_after {
        m.insert("review_after".into(), date(d));
    }
    if let Some(ts) = fm.timestamp {
        m.insert("timestamp".into(), YamlValue::String(ts.to_rfc3339()));
    }
    if let Some(def) = &fm.schema_def {
        if let Some(entity) = &def.entity {
            m.insert("entity".into(), YamlValue::String(entity.clone()));
        }
        if let Some(version) = def.version {
            m.insert("version".into(), YamlValue::Int(version));
        }
    }
    for (k, v) in &fm.extra {
        m.entry(k.clone()).or_insert_with(|| v.clone());
    }
    m
}

// --- selection ---------------------------------------------------------------

/// Select the schema that applies to an Engram.
///
/// Order: an inline `schema:` mapping in the Engram's own frontmatter, then an
/// explicit `schema: Name` reference, then an implicit match where a schema's
/// `entity` equals the Engram's type.
pub fn select_schema(engram: &Engram, schemas: &[Schema]) -> Option<Schema> {
    if let Some(inline) = Schema::from_engram(engram) {
        return Some(inline);
    }
    if let Some(name) = engram
        .frontmatter
        .extra
        .get("schema")
        .and_then(YamlValue::as_str)
        && let Some(found) = schemas.iter().find(|s| s.entity.as_deref() == Some(name))
    {
        return Some(found.clone());
    }
    schemas
        .iter()
        .find(|s| s.entity.as_deref() == Some(engram.frontmatter.engram_type.as_str()))
        .cloned()
}

// --- inference and drift -----------------------------------------------------

/// Infer a schema from a set of Engrams. Fields present in at least `threshold`
/// of the Engrams become optional; at least 0.95 makes them required.
pub fn infer(engrams: &[Engram], threshold: f64) -> Schema {
    let total = engrams.len();
    let entity = common_type(engrams);

    // First-seen order for determinism.
    let mut obs_order: Vec<String> = Vec::new();
    let mut rel_order: Vec<String> = Vec::new();
    let mut fm_order: Vec<String> = Vec::new();

    let mut obs_counts: IndexMap<String, usize> = IndexMap::new();
    let mut rel_counts: IndexMap<String, usize> = IndexMap::new();
    let mut fm_counts: IndexMap<String, usize> = IndexMap::new();

    for engram in engrams {
        let mut seen_obs = BTreeSet::new();
        for obs in &engram.observations {
            if seen_obs.insert(obs.category.clone()) {
                remember(&mut obs_order, &obs.category);
                *obs_counts.entry(obs.category.clone()).or_default() += 1;
            }
        }
        let mut seen_rel = BTreeSet::new();
        for rel in &engram.relations {
            if seen_rel.insert(rel.rel_type.clone()) {
                remember(&mut rel_order, &rel.rel_type);
                *rel_counts.entry(rel.rel_type.clone()).or_default() += 1;
            }
        }
        for key in frontmatter_map(&engram.frontmatter).keys() {
            if key == "type" {
                continue;
            }
            remember(&mut fm_order, key);
            *fm_counts.entry(key.clone()).or_default() += 1;
        }
    }

    let mut fields = Vec::new();
    for name in obs_order {
        if let Some(decl) = infer_decl(
            &name,
            obs_counts[&name],
            total,
            threshold,
            FieldType::Array(Box::new(FieldType::Scalar(ScalarType::String))),
        ) {
            fields.push(decl);
        }
    }
    for name in rel_order {
        if let Some(decl) = infer_decl(
            &name,
            rel_counts[&name],
            total,
            threshold,
            FieldType::Array(Box::new(FieldType::Entity("Note".into()))),
        ) {
            fields.push(decl);
        }
    }

    let mut frontmatter = Vec::new();
    for name in fm_order {
        if let Some(decl) = infer_decl(
            &name,
            fm_counts[&name],
            total,
            threshold,
            FieldType::Scalar(ScalarType::String),
        ) {
            frontmatter.push(decl);
        }
    }

    Schema {
        entity,
        version: Some(1),
        fields,
        validation: ValidationMode::Warn,
        frontmatter,
    }
}

fn infer_decl(
    name: &str,
    count: usize,
    total: usize,
    threshold: f64,
    field_type: FieldType,
) -> Option<FieldDecl> {
    if total == 0 {
        return None;
    }
    let freq = count as f64 / total as f64;
    if freq < threshold {
        return None;
    }
    Some(FieldDecl {
        name: name.to_string(),
        optional: freq < 0.95,
        field_type,
        description: None,
    })
}

fn remember(order: &mut Vec<String>, name: &str) {
    if !order.iter().any(|n| n == name) {
        order.push(name.to_string());
    }
}

fn common_type(engrams: &[Engram]) -> Option<String> {
    let mut ty: Option<&str> = None;
    for engram in engrams {
        let t = engram.frontmatter.engram_type.as_str();
        if t.is_empty() {
            continue;
        }
        match ty {
            None => ty = Some(t),
            Some(existing) if existing == t => {}
            Some(_) => return None,
        }
    }
    ty.map(str::to_string)
}

/// Compare a schema against a set of Engrams and report drift.
pub fn diff(schema: &Schema, engrams: &[Engram]) -> SchemaDrift {
    let declared_obs: BTreeSet<String> = schema
        .fields
        .iter()
        .filter(|f| !f.field_type.is_relation())
        .map(|f| f.name.clone())
        .collect();
    let declared_rel: BTreeSet<String> = schema
        .fields
        .iter()
        .filter(|f| f.field_type.is_relation())
        .map(|f| f.name.clone())
        .collect();

    let mut observed_obs = BTreeSet::new();
    let mut observed_rel = BTreeSet::new();
    for engram in engrams {
        for obs in &engram.observations {
            observed_obs.insert(obs.category.clone());
        }
        for rel in &engram.relations {
            observed_rel.insert(rel.rel_type.clone());
        }
    }

    SchemaDrift {
        undeclared_observations: observed_obs.difference(&declared_obs).cloned().collect(),
        undeclared_relations: observed_rel.difference(&declared_rel).cloned().collect(),
        unused_observations: declared_obs.difference(&observed_obs).cloned().collect(),
        unused_relations: declared_rel.difference(&observed_rel).cloned().collect(),
    }
}
