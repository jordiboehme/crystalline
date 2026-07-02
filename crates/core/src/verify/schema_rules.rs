//! S-family rules: Picoschema definition well-formedness (`S001`-`S004`,
//! always errors), dangling schema references (`S010`) and conformance
//! (`S020`-`S033`, delegated to [`crate::schema::validate`]).
//!
//! Conformance severity already reflects a schema's own
//! `settings.validation` (warn/strict/off, computed by
//! [`crate::schema::validate`]); `--strict` and per-rule config overrides
//! then apply on top through the normal [`Sink::emit`] path, so a Warning
//! becomes an Error under either a strict schema or a strict verify run.

use crate::engram::Engram;
use crate::schema::{IssueSeverity, Schema, SchemaIssueKind, select_schema, validate};
use crate::yaml::YamlValue;

use super::scanner::{Domain, ScannedFile};
use super::{Severity, Sink};

pub(crate) fn check(domain: &Domain, sink: &mut Sink) {
    let mut schemas: Vec<Schema> = Vec::new();

    for file in &domain.files {
        let Ok(engram) = &file.parsed else { continue };
        if engram.frontmatter.schema_def.is_some() {
            check_schema_definition(file, engram, sink);
        }
        if engram.frontmatter.engram_type == "schema"
            && let Some(def) = &engram.frontmatter.schema_def
        {
            schemas.push(Schema::from_schema_def(def));
        }
    }

    for file in &domain.files {
        let Ok(engram) = &file.parsed else { continue };
        check_dangling_reference(file, engram, &schemas, sink);
        if engram.frontmatter.engram_type == "schema" {
            continue;
        }
        if let Some(schema) = select_schema(engram, &schemas) {
            for issue in validate(engram, &schema) {
                let default = match issue.severity {
                    IssueSeverity::Error => Severity::Error,
                    IssueSeverity::Warning => Severity::Warning,
                };
                sink.emit(
                    &file.path,
                    issue.line,
                    rule_id(issue.kind),
                    default,
                    issue.message.clone(),
                    None,
                );
            }
        }
    }
}

fn rule_id(kind: SchemaIssueKind) -> &'static str {
    match kind {
        SchemaIssueKind::MissingRequiredRelation => "S020",
        SchemaIssueKind::MissingRequiredObservation => "S021",
        SchemaIssueKind::ObservationEnumViolation => "S022",
        SchemaIssueKind::ObservationTypeMismatch => "S023",
        SchemaIssueKind::MissingRequiredFrontmatter => "S030",
        SchemaIssueKind::FrontmatterEnumViolation => "S031",
        SchemaIssueKind::FrontmatterNotArray => "S032",
        SchemaIssueKind::FrontmatterTypeMismatch => "S033",
    }
}

fn check_schema_definition(file: &ScannedFile, engram: &Engram, sink: &mut Sink) {
    // `frontmatter.schema_def` is only ever populated when `type: schema`
    // (see `parse::parse_frontmatter`); this guard is defensive but keeps
    // the intent explicit at the call site.
    if engram.frontmatter.engram_type != "schema" {
        return;
    }
    let Some(def) = &engram.frontmatter.schema_def else {
        return;
    };

    if def.entity.as_deref().unwrap_or("").trim().is_empty() {
        sink.emit(
            &file.path,
            None,
            "S001",
            Severity::Error,
            "schema engram is missing `entity`",
            None,
        );
    }

    if def.schema.is_empty() {
        sink.emit(
            &file.path,
            None,
            "S002",
            Severity::Error,
            "schema engram has no field declarations under `schema:`",
            None,
        );
    }

    for (key, value) in &def.schema {
        if let Err(reason) = check_declaration(key, value) {
            sink.emit(
                &file.path,
                None,
                "S003",
                Severity::Error,
                format!("malformed field declaration `{key}`: {reason}"),
                None,
            );
        }
    }
    if let Some(fm_decls) = def
        .settings
        .get("frontmatter")
        .and_then(YamlValue::as_mapping)
    {
        for (key, value) in fm_decls {
            if let Err(reason) = check_declaration(key, value) {
                sink.emit(
                    &file.path,
                    None,
                    "S003",
                    Severity::Error,
                    format!("malformed frontmatter field declaration `{key}`: {reason}"),
                    None,
                );
            }
        }
    }

    if let Some(v) = def.settings.get("validation") {
        let ok = v
            .as_str()
            .map(|s| matches!(s.trim().to_lowercase().as_str(), "warn" | "strict" | "off"))
            .unwrap_or(false);
        if !ok {
            sink.emit(
                &file.path,
                None,
                "S004",
                Severity::Error,
                "settings.validation must be one of `warn`, `strict` or `off`",
                None,
            );
        }
    }
}

fn check_dangling_reference(
    file: &ScannedFile,
    engram: &Engram,
    schemas: &[Schema],
    sink: &mut Sink,
) {
    if engram.frontmatter.engram_type == "schema" {
        return;
    }
    if let Some(name) = engram
        .frontmatter
        .extra
        .get("schema")
        .and_then(YamlValue::as_str)
        && !schemas.iter().any(|s| s.entity.as_deref() == Some(name))
    {
        sink.emit(
            &file.path,
            None,
            "S010",
            Severity::Warning,
            format!("schema reference `{name}` does not match any schema in this domain"),
            None,
        );
    }
}

/// Re-derive well-formedness of a single Picoschema declaration
/// (`name?(modifier): value`) independently of [`crate::schema`]'s parser,
/// which silently falls back to `Scalar(Any)` for anything it does not
/// recognize (a reasonable default for validation, but it means a typo like
/// `sting` or an unknown `(fooarray)` modifier would otherwise pass
/// silently). Returns the reason a declaration is malformed, if any.
fn check_declaration(key: &str, value: &YamlValue) -> Result<(), String> {
    let key = key.trim();
    let (head, modifier) = match key.split_once('(') {
        Some((h, rest)) => {
            let modi = rest
                .strip_suffix(')')
                .ok_or_else(|| "unclosed modifier parenthesis".to_string())?;
            (h, Some(modi.trim()))
        }
        None => (key, None),
    };
    let head = head.trim();
    let name = head.strip_suffix('?').unwrap_or(head).trim();
    if name.is_empty() {
        return Err("empty field name".into());
    }
    if let Some(m) = modifier
        && !matches!(m, "array" | "enum" | "object")
    {
        return Err(format!("unknown modifier `{m}`"));
    }

    match modifier {
        Some("enum") => {
            if value.as_sequence().is_some_and(|s| !s.is_empty()) {
                Ok(())
            } else {
                Err("enum declaration must be a non-empty list".into())
            }
        }
        Some("object") => {
            if value.as_mapping().is_some() {
                Ok(())
            } else {
                Err("object declaration must be a mapping".into())
            }
        }
        Some("array") => check_type_token(value),
        _ => check_type_token(value),
    }
}

fn check_type_token(value: &YamlValue) -> Result<(), String> {
    let s = value
        .as_str()
        .ok_or_else(|| "declaration value must be a string type name".to_string())?;
    let type_name = s.split_once(',').map(|(t, _)| t.trim()).unwrap_or(s.trim());
    if type_name.is_empty() {
        return Err("empty type name".into());
    }
    let is_scalar = matches!(
        type_name,
        "string" | "integer" | "number" | "boolean" | "any"
    );
    let is_entity = type_name.chars().next().is_some_and(|c| c.is_uppercase());
    if is_scalar || is_entity {
        Ok(())
    } else {
        Err(format!("unknown type `{type_name}`"))
    }
}
