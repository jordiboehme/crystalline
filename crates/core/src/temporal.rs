//! The temporal write contract.
//!
//! A handful of frontmatter fields carry a date an agent or tool may set:
//! `recorded_at`, `valid_from`, `valid_to`, `source_date`, `stale_after` (with
//! its legacy spelling `review_after`) and the legacy `last_verified`. Every
//! one is a plain ISO date (`YYYY-MM-DD`) and day-granular; there is no
//! time-of-day component. Open-ended validity is
//! never spelled out with a distant date, it is expressed by absence: an
//! absent `valid_from` means the knowledge has always been valid and an
//! absent `valid_to` means it is valid forever.
//!
//! [`normalize_temporal_fields`] enforces that contract on the way in. It
//! rejects a value that is not a plain ISO date with a helpful
//! [`DateFieldError`], drops an explicit null bound - other than on the
//! required `recorded_at` - and a sentinel bound written by a foreign source,
//! and promotes a valid date the parser parked in `extra` into its typed
//! field. [`normalize_verified`] does the same for the shape of a `verified`
//! entry, which is a trust record rather than a date.

use chrono::{Datelike, NaiveDate};

use crate::engram::{Frontmatter, Verified};
use crate::import::{SENTINEL_FUTURE_YEAR, SENTINEL_PAST_YEAR};
use crate::yaml::YamlValue;

/// Frontmatter keys holding a plain ISO date. Single source of truth for both
/// the write contract here and the `T003` verify rule. When a raw value fails
/// to parse as `YYYY-MM-DD`, [`crate::parse`] preserves it in
/// `frontmatter.extra` instead of the typed field.
pub const DATE_FIELDS: &[&str] = &[
    "recorded_at",
    "valid_from",
    "valid_to",
    "source_date",
    "last_verified",
    "stale_after",
    "review_after",
];

/// A temporal field was set to something other than a plain ISO date. The
/// `value` field carries the offending value already rendered for display.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{field} must be a plain ISO date (YYYY-MM-DD), got {value}; temporal fields are day-granular"
)]
pub struct DateFieldError {
    /// The frontmatter key that was set, one of [`DATE_FIELDS`].
    pub field: &'static str,
    /// The offending value, rendered as [`describe`] renders it.
    pub value: String,
}

/// Normalize the temporal date fields of `fm` in place, returning the names of
/// any bounds that were dropped (empty when nothing was dropped).
///
/// The contract, applied in order:
/// 1. For each date field the parser parked in `extra`: a string that parses
///    as `YYYY-MM-DD` is promoted into its typed field and removed from
///    `extra`; an explicit null is removed and reported as a dropped bound
///    (a null bound means "no bound"), except on the required `recorded_at`,
///    whose null falls through to the error case instead; anything else - a
///    timestamp string, an int, a bool, a list or a map - is a
///    [`DateFieldError`].
/// 2. Sentinel bounds on the typed fields are dropped, since absence is how
///    open-ended validity is expressed: a `valid_to` at or above the sentinel
///    future year and a `valid_from` at or below the sentinel past year are
///    cleared and reported. Only these two bounds get sentinel treatment.
pub fn normalize_temporal_fields(
    fm: &mut Frontmatter,
) -> Result<Vec<&'static str>, DateFieldError> {
    let mut dropped: Vec<&'static str> = Vec::new();

    // 1. Reconcile each date field the parser parked in `extra`. Classify the
    // value while `extra` is only borrowed, then mutate `fm` once the borrow
    // of the value has ended.
    for &field in DATE_FIELDS {
        let Some(value) = fm.extra.get(field) else {
            continue;
        };
        let parsed = match value {
            // An explicit null bound means "no bound": drop it. recorded_at is
            // required (T001), so a null there cannot unset it and falls
            // through to the error arm below instead.
            YamlValue::Null if field != "recorded_at" => None,
            YamlValue::String(s) => Some(NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(
                |_| DateFieldError {
                    field,
                    value: describe(value),
                },
            )?),
            other => {
                return Err(DateFieldError {
                    field,
                    value: describe(other),
                });
            }
        };
        fm.extra.shift_remove(field);
        match parsed {
            Some(date) => assign_date_field(fm, field, date),
            None => dropped.push(field),
        }
    }

    // 2. Drop sentinel bounds foreign sources write literally, since absence is
    // how open-ended validity is expressed. A far-future `valid_from` or a
    // far-past `valid_to` is left alone, matching the importer.
    if let Some(to) = fm.valid_to
        && to.year() >= SENTINEL_FUTURE_YEAR
    {
        fm.valid_to = None;
        dropped.push("valid_to");
    }
    if let Some(from) = fm.valid_from
        && from.year() <= SENTINEL_PAST_YEAR
    {
        fm.valid_from = None;
        dropped.push("valid_from");
    }

    Ok(dropped)
}

/// Assign a parsed date to the typed frontmatter field named by `field`, which
/// is always one of [`DATE_FIELDS`]. The catch-all is unreachable.
fn assign_date_field(fm: &mut Frontmatter, field: &str, date: NaiveDate) {
    match field {
        "recorded_at" => fm.recorded_at = Some(date),
        "valid_from" => fm.valid_from = Some(date),
        "valid_to" => fm.valid_to = Some(date),
        "source_date" => fm.source_date = Some(date),
        "last_verified" => fm.last_verified = Some(date),
        // The legacy `review_after` spelling feeds the canonical field, so a
        // date promoted out of `extra` lands where every consumer reads it.
        "stale_after" | "review_after" => fm.stale_after = Some(date),
        _ => {}
    }
}

/// A `verified` value was set to something other than an OKF `{ by, at }` entry
/// or a list of them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "verified must be a {{ by, at }} entry or a list of them, got {value}; `by` names the actor and `at` is an RFC 3339 instant"
)]
pub struct VerifiedFieldError {
    /// The offending value, rendered as [`describe`] renders it.
    pub value: String,
}

/// Normalize the `verified` field of `fm` in place: a well-formed value the
/// parser parked in `extra` (a bare `{ by, at }` mapping or a list of them) is
/// promoted into the typed field and removed from `extra`, and anything else is
/// a [`VerifiedFieldError`].
///
/// This is the shape contract for a `verified` a caller supplies as metadata,
/// the counterpart of [`normalize_temporal_fields`] for the date keys: a write
/// carrying a malformed verification is rejected rather than stored as a shape
/// no OKF consumer can read.
pub fn normalize_verified(fm: &mut Frontmatter) -> Result<(), VerifiedFieldError> {
    let Some(value) = fm.extra.get("verified") else {
        return Ok(());
    };
    let entries = Verified::parse_list(value).ok_or_else(|| VerifiedFieldError {
        value: describe(value),
    })?;
    fm.extra.shift_remove("verified");
    fm.verified = entries;
    Ok(())
}

/// Render a YAML value for a human-readable message: a string is quoted, a
/// scalar is rendered as itself, a null reads `null` and any composite value
/// reads `a non-scalar value`.
pub(crate) fn describe(value: &YamlValue) -> String {
    match value {
        YamlValue::String(s) => format!("'{s}'"),
        YamlValue::Int(i) => i.to_string(),
        YamlValue::Float(f) => f.to_string(),
        YamlValue::Bool(b) => b.to_string(),
        YamlValue::Null => "null".to_string(),
        YamlValue::Sequence(_) | YamlValue::Mapping(_) => "a non-scalar value".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;

    fn fm_with_extra(key: &str, value: YamlValue) -> Frontmatter {
        let mut fm = Frontmatter::default();
        fm.extra.insert(key.to_string(), value);
        fm
    }

    #[test]
    fn timestamp_string_in_a_date_field_is_rejected() {
        let mut fm = fm_with_extra(
            "valid_to",
            YamlValue::String("2026-07-15T10:30:00Z".to_string()),
        );
        let err = normalize_temporal_fields(&mut fm).unwrap_err();
        assert_eq!(
            err.to_string(),
            "valid_to must be a plain ISO date (YYYY-MM-DD), got '2026-07-15T10:30:00Z'; temporal fields are day-granular"
        );
        assert_eq!(err.field, "valid_to");
    }

    #[test]
    fn valid_date_string_is_promoted_out_of_extra() {
        let mut fm = fm_with_extra("valid_from", YamlValue::String("2026-01-01".to_string()));
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert!(dropped.is_empty());
        assert_eq!(
            fm.valid_from,
            Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap())
        );
        assert!(!fm.extra.contains_key("valid_from"));
    }

    #[test]
    fn sentinel_bounds_are_dropped_at_the_boundary() {
        // valid_to: year 9000 is a sentinel and dropped, 8999 is kept.
        let mut fm = Frontmatter {
            valid_to: NaiveDate::from_ymd_opt(9000, 1, 1),
            ..Default::default()
        };
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert_eq!(dropped, vec!["valid_to"]);
        assert_eq!(fm.valid_to, None);

        let mut fm = Frontmatter {
            valid_to: NaiveDate::from_ymd_opt(8999, 12, 31),
            ..Default::default()
        };
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert!(dropped.is_empty());
        assert_eq!(fm.valid_to, NaiveDate::from_ymd_opt(8999, 12, 31));

        // valid_from: year 1 is a sentinel and dropped, year 2 is kept.
        let mut fm = Frontmatter {
            valid_from: NaiveDate::from_ymd_opt(1, 6, 1),
            ..Default::default()
        };
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert_eq!(dropped, vec!["valid_from"]);
        assert_eq!(fm.valid_from, None);

        let mut fm = Frontmatter {
            valid_from: NaiveDate::from_ymd_opt(2, 6, 1),
            ..Default::default()
        };
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert!(dropped.is_empty());
        assert_eq!(fm.valid_from, NaiveDate::from_ymd_opt(2, 6, 1));
    }

    #[test]
    fn null_drops_the_bound_and_scalars_are_rejected() {
        // An explicit null bound is dropped and named in the return.
        let mut fm = fm_with_extra("valid_to", YamlValue::Null);
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert_eq!(dropped, vec!["valid_to"]);
        assert!(!fm.extra.contains_key("valid_to"));

        // An int is not a date.
        let mut fm = fm_with_extra("recorded_at", YamlValue::Int(2026));
        let err = normalize_temporal_fields(&mut fm).unwrap_err();
        assert_eq!(err.field, "recorded_at");
        assert_eq!(err.value, "2026");

        // A bool is not a date.
        let mut fm = fm_with_extra("review_after", YamlValue::Bool(true));
        let err = normalize_temporal_fields(&mut fm).unwrap_err();
        assert_eq!(err.field, "review_after");
        assert_eq!(err.value, "true");

        // Unrelated extra keys survive normalization untouched.
        let mut fm = Frontmatter::default();
        fm.extra.insert("valid_from".to_string(), YamlValue::Null);
        fm.extra
            .insert("author".to_string(), YamlValue::String("Ada".to_string()));
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert_eq!(dropped, vec!["valid_from"]);
        assert_eq!(
            fm.extra.get("author"),
            Some(&YamlValue::String("Ada".to_string()))
        );
    }

    #[test]
    fn a_legacy_review_after_is_promoted_into_stale_after() {
        // The legacy spelling supplied as metadata lands in the canonical
        // field, so the write emits `stale_after` and every consumer sees it.
        let mut fm = fm_with_extra("review_after", YamlValue::String("2026-12-01".to_string()));
        let dropped = normalize_temporal_fields(&mut fm).unwrap();
        assert!(dropped.is_empty());
        assert_eq!(fm.stale_after, NaiveDate::from_ymd_opt(2026, 12, 1));
        assert!(fm.review_after.is_none());
        assert!(!fm.extra.contains_key("review_after"));
    }

    #[test]
    fn a_well_formed_verified_is_promoted_and_a_malformed_one_is_rejected() {
        let entry = |by: &str| {
            let mut m = IndexMap::new();
            m.insert("by".to_string(), YamlValue::String(by.to_string()));
            m.insert(
                "at".to_string(),
                YamlValue::String("2026-07-27T09:15:00+00:00".to_string()),
            );
            YamlValue::Mapping(m)
        };

        // A bare mapping is one entry, a list is many.
        let mut fm = fm_with_extra("verified", entry("human:jordi"));
        normalize_verified(&mut fm).unwrap();
        assert_eq!(fm.verified.len(), 1);
        assert_eq!(fm.verified[0].by, "human:jordi");
        assert!(!fm.extra.contains_key("verified"));

        let mut fm = fm_with_extra(
            "verified",
            YamlValue::Sequence(vec![entry("human:jordi"), entry("claude-code/1.0.5")]),
        );
        normalize_verified(&mut fm).unwrap();
        assert_eq!(fm.verified.len(), 2);

        // A bare date is the legacy shape, not a verification entry.
        let mut fm = fm_with_extra("verified", YamlValue::String("2026-07-27".to_string()));
        let err = normalize_verified(&mut fm).unwrap_err();
        assert_eq!(err.value, "'2026-07-27'");
        assert!(fm.verified.is_empty());
    }

    #[test]
    fn null_recorded_at_is_rejected_not_dropped() {
        // recorded_at is required (T001): a null cannot unset it, so it must
        // error instead of being silently dropped like the other bounds.
        let mut fm = fm_with_extra("recorded_at", YamlValue::Null);
        let err = normalize_temporal_fields(&mut fm).unwrap_err();
        assert_eq!(err.field, "recorded_at");
        assert_eq!(
            err.to_string(),
            "recorded_at must be a plain ISO date (YYYY-MM-DD), got null; temporal fields are day-granular"
        );
    }
}
