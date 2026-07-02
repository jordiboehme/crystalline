//! T-family rules: temporal metadata.
//!
//! `status` and `recorded_at` are required (`T001`); `valid_from`/`valid_to`
//! stay optional and open-ended (absence means unbounded, per the format
//! spec), so the only hard checks on them are that a present value parses as
//! an ISO date (`T003`) and that `valid_from <= valid_to` when both are
//! present (`T004`). A sentinel far-future `valid_to` (`T008`) is flagged
//! rather than treated as valid input, since Crystalline expresses "valid
//! forever" by omitting the field, never by writing a distant date.

use chrono::Datelike;

use crate::engram::RECOMMENDED_STATUSES;
use crate::yaml::YamlValue;

use super::scanner::ScannedFile;
use super::{Severity, Sink};

/// Frontmatter keys holding a plain ISO date. When the raw value fails to
/// parse, [`crate::parse`] preserves it in `frontmatter.extra` instead of
/// the typed field (see the M1 parser note), which is what T003 reads.
const DATE_FIELDS: &[&str] = &[
    "recorded_at",
    "valid_from",
    "valid_to",
    "source_date",
    "last_verified",
    "review_after",
];

pub(crate) fn check(file: &ScannedFile, sink: &mut Sink) {
    let Ok(engram) = &file.parsed else { return };
    let fm = &engram.frontmatter;

    if fm.status.as_deref().map(str::trim).unwrap_or("").is_empty() {
        sink.emit(
            &file.path,
            None,
            "T001",
            Severity::Error,
            "required field `status` is missing",
            None,
        );
    }
    if fm.recorded_at.is_none() && !fm.extra.contains_key("recorded_at") {
        sink.emit(
            &file.path,
            None,
            "T001",
            Severity::Error,
            "required field `recorded_at` is missing",
            None,
        );
    }

    if let Some(status) = &fm.status
        && !status.trim().is_empty()
        && !RECOMMENDED_STATUSES.contains(&status.as_str())
    {
        sink.emit(
            &file.path,
            None,
            "T002",
            Severity::Info,
            format!("status `{status}` is outside the recommended set"),
            None,
        );
    }

    for field in DATE_FIELDS {
        if let Some(raw) = fm.extra.get(*field) {
            sink.emit(
                &file.path,
                None,
                "T003",
                Severity::Error,
                format!(
                    "field `{field}` is not a valid ISO date ({})",
                    describe(raw)
                ),
                None,
            );
        }
    }
    if let Some(raw) = fm.extra.get("timestamp") {
        sink.emit(
            &file.path,
            None,
            "T003",
            Severity::Error,
            format!(
                "field `timestamp` is not a valid RFC 3339 timestamp ({})",
                describe(raw)
            ),
            None,
        );
    }

    if let (Some(from), Some(to)) = (fm.valid_from, fm.valid_to)
        && from > to
    {
        sink.emit(
            &file.path,
            None,
            "T004",
            Severity::Error,
            format!("valid_from ({from}) is after valid_to ({to})"),
            None,
        );
    }

    if fm
        .status
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("superseded"))
    {
        let has_relation = engram
            .relations
            .iter()
            .any(|r| r.rel_type.eq_ignore_ascii_case("superseded_by"));
        if !has_relation {
            sink.emit(
                &file.path,
                None,
                "T005",
                Severity::Warning,
                "status is `superseded` but no `superseded_by` relation is present",
                Some("add `- superseded_by [[Target]]`".into()),
            );
        }
    }

    if fm.timestamp.is_none() && !fm.extra.contains_key("timestamp") {
        sink.emit(
            &file.path,
            None,
            "T006",
            Severity::Warning,
            "missing `timestamp`",
            None,
        );
    }

    if let Some(tc) = &fm.temporal_confidence
        && tc != "explicit"
        && tc != "inferred"
    {
        sink.emit(
            &file.path,
            None,
            "T007",
            Severity::Warning,
            format!("temporal_confidence `{tc}` is not `explicit` or `inferred`"),
            None,
        );
    }

    if let Some(to) = fm.valid_to
        && to.year() >= 9000
    {
        sink.emit(
            &file.path,
            None,
            "T008",
            Severity::Warning,
            format!("valid_to `{to}` looks like a sentinel far-future date"),
            Some("remove the field; absence means valid forever".into()),
        );
    }
}

fn describe(v: &YamlValue) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| "non-scalar value".to_string())
}
