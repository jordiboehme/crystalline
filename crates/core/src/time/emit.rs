//! Dates printed by hand from plain numbers, byte for byte as chrono printed
//! them: RFC 3339 (`to_rfc3339`), the `Debug` forms and chrono's `Display`
//! of a UTC instant.

use super::calendar::civil_from_days;
use std::fmt::Write;

/// A year, month and day as `YYYY-MM-DD`: four digits within 0000 to 9999,
/// a sign and at least four digits outside it.
pub(super) fn date(year: i64, month: u8, day: u8) -> String {
    let mut out = String::with_capacity(12);
    if (0..=9999).contains(&year) {
        let _ = write!(out, "{year:04}");
    } else {
        let _ = write!(out, "{year:+05}");
    }
    let _ = write!(out, "-{month:02}-{day:02}");
    out
}

/// The local date and time of an instant `secs` seconds and `nanos`
/// nanoseconds after the Unix epoch, shown at `offset` seconds east of UTC,
/// with `separator` between date and time.
///
/// `nanos` may reach into a second second (1_000_000_000 or more) for a
/// leap second, which is then written as `:60`. The fraction comes in
/// groups of none, 3, 6 or 9 digits.
fn local(secs: i64, nanos: u32, offset: i32, separator: char) -> String {
    let local = secs + i64::from(offset);
    let (year, month, day) = civil_from_days(local.div_euclid(86_400));
    let second_of_day = local.rem_euclid(86_400);
    let (hour, minute, mut second) = (
        second_of_day / 3600,
        second_of_day / 60 % 60,
        second_of_day % 60,
    );
    let mut nanos = nanos;
    if nanos >= 1_000_000_000 {
        second += 1;
        nanos -= 1_000_000_000;
    }

    let mut out = date(year, month, day);
    let _ = write!(out, "{separator}{hour:02}:{minute:02}:{second:02}");
    if nanos == 0 {
        // A whole second writes no fraction.
    } else if nanos.is_multiple_of(1_000_000) {
        let _ = write!(out, ".{:03}", nanos / 1_000_000);
    } else if nanos.is_multiple_of(1_000) {
        let _ = write!(out, ".{:06}", nanos / 1_000);
    } else {
        let _ = write!(out, ".{nanos:09}");
    }
    out
}

/// RFC 3339 as chrono's `to_rfc3339` writes it: the local date and time,
/// then the offset as `+hh:mm` or `-hh:mm`, rounded to the nearest minute
/// and never `Z`. The sign comes from the offset before rounding.
pub(super) fn rfc3339(secs: i64, nanos: u32, offset: i32) -> String {
    let mut out = local(secs, nanos, offset, 'T');
    let (sign, magnitude) = if offset < 0 {
        ('-', -i64::from(offset))
    } else {
        ('+', i64::from(offset))
    };
    let rounded_minutes = (magnitude + 30) / 60;
    let _ = write!(
        out,
        "{sign}{:02}:{:02}",
        rounded_minutes / 60,
        rounded_minutes % 60
    );
    out
}

/// chrono's `Debug` of a `DateTime<Utc>`: the date and time with `Z`.
pub(super) fn debug_utc(secs: i64, nanos: u32) -> String {
    let mut out = local(secs, nanos, 0, 'T');
    out.push('Z');
    out
}

/// chrono's `Debug` of a `DateTime<FixedOffset>`: the local date and time,
/// then the exact offset as `+hh:mm`, or `+hh:mm:ss` when it has seconds.
pub(super) fn debug_offset(secs: i64, nanos: u32, offset: i32) -> String {
    let mut out = local(secs, nanos, offset, 'T');
    let sign = if offset < 0 { '-' } else { '+' };
    let magnitude = offset.unsigned_abs();
    let (hour, minute, second) = (magnitude / 3600, magnitude / 60 % 60, magnitude % 60);
    let _ = write!(out, "{sign}{hour:02}:{minute:02}");
    if second != 0 {
        let _ = write!(out, ":{second:02}");
    }
    out
}

/// chrono's `Display` of a `DateTime<Utc>`: `2026-01-05 10:17:19 UTC`, with
/// the fraction when there is one.
pub(super) fn display_utc(secs: i64, nanos: u32) -> String {
    let mut out = local(secs, nanos, 0, ' ');
    out.push_str(" UTC");
    out
}
