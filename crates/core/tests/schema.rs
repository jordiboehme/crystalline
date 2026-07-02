//! Picoschema parsing, validation, selection, inference and drift.

mod common;

use common::{fixtures_dir, read};
use crystalline_core::parse_engram;
use crystalline_core::schema::{
    FieldType, IssueSeverity, ScalarType, Schema, SchemaIssueKind, ValidationMode, diff, infer,
    select_schema, validate,
};

fn engram(rel: &str) -> crystalline_core::Engram {
    parse_engram(&read(&fixtures_dir().join(rel))).unwrap()
}

fn task_schema() -> Schema {
    Schema::from_engram(&engram("schemas/schema-task.md")).expect("schema engram")
}

#[test]
fn schema_parses_declarations() {
    let s = task_schema();
    assert_eq!(s.entity.as_deref(), Some("task"));
    assert_eq!(s.version, Some(1));
    assert_eq!(s.validation, ValidationMode::Warn);

    let summary = s.fields.iter().find(|f| f.name == "summary").unwrap();
    assert!(!summary.optional);
    assert_eq!(summary.field_type, FieldType::Scalar(ScalarType::String));
    assert_eq!(summary.description.as_deref(), Some("one line summary"));

    let priority = s.fields.iter().find(|f| f.name == "priority").unwrap();
    assert!(!priority.optional);
    assert!(matches!(&priority.field_type, FieldType::Enum(v) if v == &["low", "medium", "high"]));

    let estimate = s.fields.iter().find(|f| f.name == "estimate").unwrap();
    assert!(estimate.optional);
    assert_eq!(estimate.field_type, FieldType::Scalar(ScalarType::Integer));

    // Capitalized types are relations.
    let blocked = s.fields.iter().find(|f| f.name == "blocked_by").unwrap();
    assert!(blocked.field_type.is_relation());

    // Frontmatter declarations parse from settings.frontmatter.
    let owner = s.frontmatter.iter().find(|f| f.name == "owner").unwrap();
    assert!(!owner.optional);
    let status = s.frontmatter.iter().find(|f| f.name == "status").unwrap();
    assert!(matches!(&status.field_type, FieldType::Enum(_)));
}

#[test]
fn conforming_engram_has_no_issues() {
    let schema = task_schema();
    let e = engram("schemas/conforming-engram.md");
    let issues = validate(&e, &schema);
    assert!(issues.is_empty(), "unexpected issues: {issues:?}");
}

#[test]
fn violating_engram_reports_expected_issues() {
    let schema = task_schema();
    let e = engram("schemas/violating-engram.md");
    let issues = validate(&e, &schema);
    let kinds: Vec<SchemaIssueKind> = issues.iter().map(|i| i.kind).collect();

    assert!(kinds.contains(&SchemaIssueKind::MissingRequiredObservation)); // summary
    assert!(kinds.contains(&SchemaIssueKind::ObservationEnumViolation)); // priority urgent
    assert!(kinds.contains(&SchemaIssueKind::MissingRequiredFrontmatter)); // owner
    // Warn mode surfaces warnings, not errors.
    assert!(issues.iter().all(|i| i.severity == IssueSeverity::Warning));
}

#[test]
fn off_mode_suppresses_issues() {
    let mut schema = task_schema();
    schema.validation = ValidationMode::Off;
    let e = engram("schemas/violating-engram.md");
    assert!(validate(&e, &schema).is_empty());
}

#[test]
fn strict_mode_promotes_to_errors() {
    let mut schema = task_schema();
    schema.validation = ValidationMode::Strict;
    let e = engram("schemas/violating-engram.md");
    let issues = validate(&e, &schema);
    assert!(!issues.is_empty());
    assert!(issues.iter().all(|i| i.severity == IssueSeverity::Error));
}

#[test]
fn select_schema_prefers_inline_then_reference_then_implicit() {
    let schema = task_schema();
    let schemas = [schema.clone()];

    // Implicit: engram type matches schema entity.
    let conforming = engram("schemas/conforming-engram.md");
    let selected = select_schema(&conforming, &schemas).unwrap();
    assert_eq!(selected.entity.as_deref(), Some("task"));

    // Inline: a schema engram selects its own inline schema.
    let schema_engram = engram("schemas/schema-task.md");
    let inline = select_schema(&schema_engram, &[]).unwrap();
    assert_eq!(inline.entity.as_deref(), Some("task"));
}

#[test]
fn infer_marks_common_fields_required() {
    let a = engram("schemas/conforming-engram.md");
    let b = engram("schemas/violating-engram.md");
    let inferred = infer(&[a, b], 0.25);
    assert_eq!(inferred.entity.as_deref(), Some("task"));

    // priority appears in both engrams, so it is required (>= 0.95).
    let priority = inferred
        .fields
        .iter()
        .find(|f| f.name == "priority")
        .unwrap();
    assert!(!priority.optional);

    // summary appears in only one engram, so it is optional at threshold 0.25.
    let summary = inferred.fields.iter().find(|f| f.name == "summary");
    assert!(summary.map(|f| f.optional).unwrap_or(false));
}

#[test]
fn diff_reports_undeclared_and_unused() {
    let schema = task_schema();
    // The violating engram uses a `note` observation and a `relates_to` relation
    // that the schema does not declare.
    let e = engram("schemas/violating-engram.md");
    let drift = diff(&schema, &[e]);
    assert!(drift.undeclared_observations.contains(&"note".to_string()));
    assert!(
        drift
            .undeclared_relations
            .contains(&"relates_to".to_string())
    );
    // Declared fields absent from the sample show up as unused.
    assert!(drift.unused_observations.contains(&"summary".to_string()));
}
