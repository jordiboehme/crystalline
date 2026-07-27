//! T-family rules: temporal metadata.
//!
//! `status` and `recorded_at` are required (`T001`); `valid_from`/`valid_to`
//! stay optional and open-ended (absence means unbounded, per the format
//! spec), so the only hard checks on them are that a present value parses as
//! an ISO date (`T003`) and that `valid_from <= valid_to` when both are
//! present (`T004`). A sentinel far-future `valid_to` (`T008`) is flagged
//! rather than treated as valid input, since Crystalline expresses "valid
//! forever" by omitting the field, never by writing a distant date.

use std::path::Path;

use chrono::Datelike;

use crate::engram::{Engram, RECOMMENDED_STATUSES};
use crate::import::SENTINEL_FUTURE_YEAR;
use crate::temporal::{DATE_FIELDS, describe};

use super::scanner::ScannedFile;
use super::{Severity, Sink};

pub(crate) fn check(file: &ScannedFile, sink: &mut Sink) {
    let Ok(engram) = &file.parsed else { return };
    check_engram(&file.path, engram, sink);
}

pub(crate) fn check_engram(path: &Path, engram: &Engram, sink: &mut Sink) {
    let fm = &engram.frontmatter;

    if fm.status.as_deref().map(str::trim).unwrap_or("").is_empty() {
        sink.emit(
            path,
            None,
            "T001",
            Severity::Error,
            "required field `status` is missing",
            None,
        );
    }
    if fm.recorded_at.is_none() && !fm.extra.contains_key("recorded_at") {
        sink.emit(
            path,
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
            path,
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
                path,
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
            path,
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
    if let Some(raw) = fm.extra.get("generated") {
        sink.emit(
            path,
            None,
            "T003",
            Severity::Error,
            format!(
                "field `generated` is not a mapping with a `by` actor ({})",
                describe(raw)
            ),
            None,
        );
    }

    if let Some(raw) = fm.extra.get("verified") {
        sink.emit(
            path,
            None,
            "T003",
            Severity::Error,
            format!(
                "field `verified` is not a `{{ by, at }}` entry or a list of them ({})",
                describe(raw)
            ),
            None,
        );
    }

    if let (Some(from), Some(to)) = (fm.valid_from, fm.valid_to)
        && from > to
    {
        sink.emit(
            path,
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
                path,
                None,
                "T005",
                Severity::Warning,
                "status is `superseded` but no `superseded_by` relation is present",
                Some("add `- superseded_by [[Target]]`".into()),
            );
        }
    }

    // Write provenance is `generated` since OKF v0.2; the legacy `timestamp`
    // key still satisfies the rule, so an engram written before the migration
    // is not nagged about until something edits it.
    let has_provenance = fm.generated.is_some()
        || fm.extra.contains_key("generated")
        || fm.timestamp.is_some()
        || fm.extra.contains_key("timestamp");
    if !has_provenance {
        sink.emit(
            path,
            None,
            "T006",
            Severity::Warning,
            "missing `generated`",
            None,
        );
    }

    if let Some(tc) = &fm.temporal_confidence
        && tc != "explicit"
        && tc != "inferred"
    {
        sink.emit(
            path,
            None,
            "T007",
            Severity::Warning,
            format!("temporal_confidence `{tc}` is not `explicit` or `inferred`"),
            None,
        );
    }

    if let Some(to) = fm.valid_to
        && to.year() >= SENTINEL_FUTURE_YEAR
    {
        sink.emit(
            path,
            None,
            "T008",
            Severity::Warning,
            format!("valid_to `{to}` looks like a sentinel far-future date"),
            Some("remove the field; absence means valid forever".into()),
        );
    }
}
