//! RFC 3339 printed by hand from plain numbers, byte for byte as chrono's
//! `to_rfc3339` writes it.

use super::calendar::civil_from_days;
use std::fmt::Write;

/// The instant `secs` seconds and `nanos` nanoseconds after the Unix epoch,
/// shown at `offset` seconds east of UTC.
///
/// `nanos` may reach into a second second (1_000_000_000 or more) for a
/// leap second, which is then written as `:60`. The year has four digits
/// within 0000 to 9999 and a sign plus at least four digits outside it. The
/// fraction comes in groups of none, 3, 6 or 9 digits. The offset is
/// `+hh:mm` or `-hh:mm`, rounded to the nearest minute, and never `Z`.
pub(super) fn rfc3339(secs: i64, nanos: u32, offset: i32) -> String {
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

    let mut out = String::with_capacity(32);
    if (0..=9999).contains(&year) {
        let _ = write!(out, "{year:04}");
    } else {
        let _ = write!(out, "{year:+05}");
    }
    let _ = write!(
        out,
        "-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
    );
    if nanos == 0 {
        // A whole second writes no fraction.
    } else if nanos.is_multiple_of(1_000_000) {
        let _ = write!(out, ".{:03}", nanos / 1_000_000);
    } else if nanos.is_multiple_of(1_000) {
        let _ = write!(out, ".{:06}", nanos / 1_000);
    } else {
        let _ = write!(out, ".{nanos:09}");
    }
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
