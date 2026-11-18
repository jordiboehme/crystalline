//! The date grammars, written by hand: text in, plain numbers ([`Parts`])
//! out. Nothing here knows a date library; `time.rs` turns the numbers into
//! a value.
//!
//! Each grammar reproduces one chrono parser of 0.33 exactly, quirks
//! included. A differential test held them to that while chrono was in the
//! build; the frozen corpus (`corpus.rs`) holds them to its answers now:
//!
//! - [`rfc3339`] for `DateTime::parse_from_rfc3339`
//! - [`lenient`] for `FromStr` (and so serde's `Deserialize`) for
//!   `DateTime<FixedOffset>` and `DateTime<Utc>`
//! - [`date`] for `NaiveDate::parse_from_str(s, "%Y-%m-%d")`
//! - [`lenient_date`] for `FromStr` (and `Deserialize`) for `NaiveDate`
//! - [`kept`] for `NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ")`
//! - [`rfc2822`] for `DateTime::parse_from_rfc2822`
//!
//! chrono builds all but the strict and the RFC 2822 grammar from the same
//! few scanning steps, and so do these: every number skips leading white
//! space first (`str::trim_start`, so any Unicode white space), a year
//! without a sign takes one to four digits and one with a sign takes any
//! number, and the other fields take one or two digits.

use super::calendar::{days_from_civil, days_in_month, weekday_from_monday};

/// A date and time of day as written, with the offset it was written in.
///
/// A leap second (`:60`) is held as `second: 59` with `leap` set, so the
/// value needs no second beyond 59.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Parts {
    pub(super) year: i32,
    pub(super) month: u8,
    pub(super) day: u8,
    pub(super) hour: u8,
    pub(super) minute: u8,
    pub(super) second: u8,
    pub(super) nanos: u32,
    pub(super) leap: bool,
    pub(super) offset_seconds: i32,
}

impl Parts {
    /// Midnight at the start of a date, offset zero.
    fn date(year: i32, month: u8, day: u8) -> Parts {
        Parts {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            nanos: 0,
            leap: false,
            offset_seconds: 0,
        }
    }

    /// These parts when every field is in range, with second 60 turned into
    /// a leap second; `None` otherwise. The checks are chrono's: an existing
    /// date, hour 0 to 23, minute 0 to 59, second 0 to 60 (at any minute)
    /// and an offset of less than 24 hours either way. Whether the year is
    /// one the library can hold is left to the conversion.
    fn checked(mut self) -> Option<Parts> {
        let date = (1..=days_in_month(self.year, self.month)).contains(&self.day);
        let time = self.hour < 24 && self.minute < 60 && self.second <= 60;
        let offset = self.offset_seconds.unsigned_abs() < 86_400;
        if !(date && time && offset && self.nanos < 1_000_000_000) {
            return None;
        }
        if self.second == 60 {
            self.second = 59;
            self.leap = true;
        }
        Some(self)
    }
}

/// `DateTime::parse_from_rfc3339`: `YYYY-MM-DD`, then `T`, `t` or a space,
/// then `hh:mm:ss` at fixed positions, then an optional `.` with one or more
/// digits (nine kept, the rest dropped), then `Z`, `z` or a sign (`+`, `-`
/// or U+2212) with `hh:mm`, and nothing after it.
pub(super) fn rfc3339(s: &str) -> Option<Parts> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let digit = |i: usize| b[i].is_ascii_digit().then(|| b[i] - b'0');
    let pair = |i: usize| Some(digit(i)? * 10 + digit(i + 1)?);
    let separators = b[4] == b'-'
        && b[7] == b'-'
        && matches!(b[10], b'T' | b't' | b' ')
        && b[13] == b':'
        && b[16] == b':';
    if !separators {
        return None;
    }
    let fields = Parts {
        year: i32::from(pair(0)?) * 100 + i32::from(pair(2)?),
        month: pair(5)?,
        day: pair(8)?,
        hour: pair(11)?,
        minute: pair(14)?,
        second: pair(17)?,
        nanos: 0,
        leap: false,
        offset_seconds: 0,
    };
    // The first 19 bytes are ASCII now, so both cuts fall on a character
    // edge.
    let (rest, nanos) = if b[19..].first() == Some(&b'.') {
        fraction(&s[20..])?
    } else {
        (&s[19..], 0)
    };
    let (rest, offset_seconds) = offset(rest, Colon::One)?;
    if !rest.is_empty() {
        return None;
    }
    Parts {
        nanos,
        offset_seconds,
        ..fields
    }
    .checked()
}

/// chrono's `FromStr` for `DateTime<FixedOffset>`: a year (signed or not),
/// `-`, month, `-`, day, then exactly one `T`, `t` or space, then hour, `:`,
/// minute, `:`, second, an optional `.` fraction right after the second,
/// then `UTC` (any case), `Z`, `z` or a sign (`+`, `-`, U+2212) with two hour
/// digits, any run of `:` and white space, and two minute digits. White
/// space may stand before every number, before each `-` and `:`, before the
/// offset and at the end.
pub(super) fn lenient(s: &str) -> Option<Parts> {
    let (s, year) = year(s)?;
    let s = s.trim_start().strip_prefix('-')?;
    let (s, month) = field(s)?;
    let s = s.trim_start().strip_prefix('-')?;
    let (s, day) = field(s)?;
    let s = match s.as_bytes().first() {
        Some(b'T' | b't' | b' ') => &s[1..],
        _ => return None,
    };
    let (s, hour) = field(s)?;
    let s = s.trim_start().strip_prefix(':')?;
    let (s, minute) = field(s)?;
    let s = s.trim_start().strip_prefix(':')?;
    let (s, second) = field(s)?;
    let (s, nanos) = match s.strip_prefix('.') {
        Some(digits) => fraction(digits)?,
        None => (s, 0),
    };
    let s = s.trim_start();
    let (s, offset_seconds) = if s.len() >= 3 && s.as_bytes()[..3].eq_ignore_ascii_case(b"UTC") {
        (&s[3..], 0)
    } else {
        offset(s, Colon::AnyRun)?
    };
    if !s.trim_start().is_empty() {
        return None;
    }
    Parts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        nanos,
        leap: false,
        offset_seconds,
    }
    .checked()
}

/// `NaiveDate::parse_from_str(s, "%Y-%m-%d")`: a year (signed or not), `-`,
/// month, `-`, day. White space may stand before each number only.
pub(super) fn date(s: &str) -> Option<Parts> {
    let (s, year) = year(s)?;
    let s = s.strip_prefix('-')?;
    let (s, month) = field(s)?;
    let s = s.strip_prefix('-')?;
    let (s, day) = field(s)?;
    if !s.is_empty() {
        return None;
    }
    Parts::date(year, month, day).checked()
}

/// chrono's `FromStr` for `NaiveDate`: as [`date`], plus white space before
/// each `-` and at the end.
pub(super) fn lenient_date(s: &str) -> Option<Parts> {
    let (s, year) = year(s)?;
    let s = s.trim_start().strip_prefix('-')?;
    let (s, month) = field(s)?;
    let s = s.trim_start().strip_prefix('-')?;
    let (s, day) = field(s)?;
    if !s.trim_start().is_empty() {
        return None;
    }
    Parts::date(year, month, day).checked()
}

/// `NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ")`, read as UTC: a
/// year, month, day, `T`, hour, minute, second, `Z`. A year without a sign
/// takes four digits at most, so `20260105` splits into 2026, 01 and 05; a
/// signed year takes every digit that follows, which is why
/// `+20260105T101719Z` is refused, while `+2026 0105T101719Z` is accepted.
pub(super) fn kept(s: &str) -> Option<Parts> {
    let (s, year) = year(s)?;
    let (s, month) = field(s)?;
    let (s, day) = field(s)?;
    let s = s.strip_prefix('T')?;
    let (s, hour) = field(s)?;
    let (s, minute) = field(s)?;
    let (s, second) = field(s)?;
    if s != "Z" {
        return None;
    }
    Parts {
        hour,
        minute,
        second,
        ..Parts::date(year, month, day)
    }
    .checked()
}

/// `DateTime::parse_from_rfc2822`: an optional weekday name directly
/// followed by `,` (it must match the date), a day of one or two digits, a
/// three-letter month name, a year of two or more digits (two digits below
/// 50 are 20xx, other two- and three-digit years 19xx), then `hh:mm` with an
/// optional `:ss`, then a zone: `+hhmm`/`-hhmm`, `GMT`, `UT`, `Z`, the eight
/// North American names or one military letter (offset zero). Comments in
/// parentheses may follow; nothing else may, not even white space. Names
/// are matched in any case.
pub(super) fn rfc2822(s: &str) -> Option<Parts> {
    let mut s = s.trim_start();
    let mut weekday = None;
    if let Some((rest, day)) = name_of(s, &WEEKDAYS) {
        s = rest.strip_prefix(',')?;
        weekday = Some(day);
    }
    let (s, day) = number(s.trim_start(), 1, 2)?;
    let s = space(s)?;
    let (s, month) = name_of(s, &MONTHS)?;
    let s = space(s)?;
    let (rest, year) = number(s, 2, usize::MAX)?;
    let year = match (s.len() - rest.len(), year) {
        (2, 0..=49) => year + 2000,
        (2, 50..=99) | (3, _) => year + 1900,
        _ => year,
    };
    let year = i32::try_from(year).ok()?;
    let s = space(rest)?;
    let (s, hour) = number(s, 2, 2)?;
    let s = s.trim_start().strip_prefix(':')?.trim_start();
    let (s, minute) = number(s, 2, 2)?;
    let (s, second) = match s.trim_start().strip_prefix(':') {
        Some(rest) => number(rest, 2, 2)?,
        None => (s, 0),
    };
    let s = space(s)?;
    let (mut s, offset_seconds) = zone_2822(s)?;
    while let Some(rest) = comment(s) {
        s = rest;
    }
    if !s.is_empty() {
        return None;
    }
    let parts = Parts {
        year,
        month: month + 1,
        day: u8::try_from(day).ok()?,
        hour: u8::try_from(hour).ok()?,
        minute: u8::try_from(minute).ok()?,
        second: u8::try_from(second).ok()?,
        nanos: 0,
        leap: false,
        offset_seconds,
    }
    .checked()?;
    let on_day = weekday_from_monday(days_from_civil(parts.year, parts.month, parts.day));
    if weekday.is_some_and(|w| w != on_day) {
        return None;
    }
    Some(parts)
}

/// Up to `max` ASCII digits, at least `min`, as a number; `None` when there
/// are too few or the value overflows.
fn number(s: &str, min: usize, max: usize) -> Option<(&str, i64)> {
    let bytes = s.as_bytes();
    let mut value: i64 = 0;
    let mut taken = 0;
    for &c in bytes.iter().take(max) {
        if !c.is_ascii_digit() {
            break;
        }
        value = value.checked_mul(10)?.checked_add(i64::from(c - b'0'))?;
        taken += 1;
    }
    (taken >= min).then(|| (&s[taken..], value))
}

/// A month, day, hour, minute or second: white space, then one or two
/// digits.
fn field(s: &str) -> Option<(&str, u8)> {
    let (rest, value) = number(s.trim_start(), 1, 2)?;
    Some((rest, u8::try_from(value).ok()?))
}

/// A year: white space, then `-` or `+` and any number of digits, or one to
/// four digits with no sign. It must fit an `i32`.
fn year(s: &str) -> Option<(&str, i32)> {
    let s = s.trim_start();
    let (rest, value) = if let Some(digits) = s.strip_prefix('-') {
        let (rest, value) = number(digits, 1, usize::MAX)?;
        (rest, -value)
    } else if let Some(digits) = s.strip_prefix('+') {
        number(digits, 1, usize::MAX)?
    } else {
        number(s, 1, 4)?
    };
    Some((rest, i32::try_from(value).ok()?))
}

/// The digits after a `.`: one or more, the first nine read as a fraction of
/// a second and any further digits skipped.
fn fraction(s: &str) -> Option<(&str, u32)> {
    let (rest, value) = number(s, 1, 9)?;
    let taken = s.len() - rest.len();
    let nanos = u32::try_from(value).ok()? * 10u32.pow(9 - taken as u32);
    Some((rest.trim_start_matches(|c: char| c.is_ascii_digit()), nanos))
}

/// What may stand between the hour and the minute of a numeric offset; it
/// also tells the RFC 3339 forms (`One`, `AnyRun`) from RFC 2822's (`None`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Colon {
    /// Exactly one `:` (RFC 3339).
    One,
    /// Any run of `:` and white space, also none (chrono's `FromStr`).
    AnyRun,
    /// Nothing (RFC 2822's `+hhmm`).
    None,
}

/// An offset in seconds. The RFC 3339 forms take `Z` or `z` for zero, and a
/// sign of `+`, `-` or U+2212; RFC 2822 takes only `+` or `-`. Then two hour
/// digits (00 to 99), what `colon` allows, and two minute digits (00 to 59).
fn offset(s: &str, colon: Colon) -> Option<(&str, i32)> {
    let rfc3339 = colon != Colon::None;
    if rfc3339 && matches!(s.as_bytes().first(), Some(b'Z' | b'z')) {
        return Some((&s[1..], 0));
    }
    let (negative, s) = match s.chars().next() {
        Some('+') => (false, &s[1..]),
        Some('-') => (true, &s[1..]),
        Some('\u{2212}') if rfc3339 => (true, &s['\u{2212}'.len_utf8()..]),
        _ => return None,
    };
    let b = s.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_digit() || !b[1].is_ascii_digit() {
        return None;
    }
    let hours = i32::from((b[0] - b'0') * 10 + (b[1] - b'0'));
    let s = &s[2..];
    let s = match colon {
        Colon::One => s.strip_prefix(':')?,
        Colon::AnyRun => s.trim_start_matches(|c: char| c == ':' || c.is_whitespace()),
        Colon::None => s,
    };
    let b = s.as_bytes();
    if b.len() < 2 || !(b'0'..=b'5').contains(&b[0]) || !b[1].is_ascii_digit() {
        return None;
    }
    let minutes = i32::from((b[0] - b'0') * 10 + (b[1] - b'0'));
    let seconds = hours * 3600 + minutes * 60;
    Some((&s[2..], if negative { -seconds } else { seconds }))
}

const WEEKDAYS: [&[u8; 3]; 7] = [b"mon", b"tue", b"wed", b"thu", b"fri", b"sat", b"sun"];

const MONTHS: [&[u8; 3]; 12] = [
    b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec",
];

/// A three-letter name from `names` in any case, as its index.
fn name_of<'a>(s: &'a str, names: &[&[u8; 3]]) -> Option<(&'a str, u8)> {
    let head = s.as_bytes().get(..3)?;
    let index = names.iter().position(|n| head.eq_ignore_ascii_case(*n))?;
    Some((&s[3..], index as u8))
}

/// One or more white space characters, skipped.
fn space(s: &str) -> Option<&str> {
    let rest = s.trim_start();
    (rest.len() < s.len()).then_some(rest)
}

/// An RFC 2822 zone: a name (letters only) or `+hhmm`/`-hhmm`.
fn zone_2822(s: &str) -> Option<(&str, i32)> {
    let letters = s
        .bytes()
        .position(|c| !c.is_ascii_alphabetic())
        .unwrap_or(s.len());
    if letters == 0 {
        return offset(s, Colon::None);
    }
    let (name, rest) = s.split_at(letters);
    let is = |n: &str| name.eq_ignore_ascii_case(n);
    let hours = if is("gmt") || is("ut") || is("z") {
        0
    } else if is("edt") {
        -4
    } else if is("est") || is("cdt") {
        -5
    } else if is("cst") || is("mdt") {
        -6
    } else if is("mst") || is("pdt") {
        -7
    } else if is("pst") {
        -8
    } else if name.len() == 1 && !is("j") {
        0
    } else {
        return None;
    };
    Some((rest, hours * 3600))
}

/// One comment in parentheses after optional white space; comments nest and
/// `\` escapes the next byte.
fn comment(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let b = s.as_bytes();
    if b.first() != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, &c) in b.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            b'\\' => escaped = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[i + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grammars' own range checks, seen without the library: chrono
    /// checks these again behind them, jiff checks fewer (its offsets reach
    /// 25:59:59), so they must hold at the `Parts` level.
    #[test]
    fn grammars_refuse_out_of_range_fields_on_their_own() {
        let out_of_range = [
            "2026-01-05T10:17:19+24:00",
            "2026-01-05T10:17:19-24:00",
            "2026-01-05T10:17:19+25:59",
            "2026-01-05T24:00:00Z",
            "2026-01-05T10:60:00Z",
            "2026-01-05T10:17:61Z",
            "2026-02-29T10:17:19Z",
            "2026-13-05T10:17:19Z",
            "2026-01-00T10:17:19Z",
        ];
        for input in out_of_range {
            assert_eq!(rfc3339(input), None, "{input:?}");
            assert_eq!(lenient(input), None, "{input:?}");
        }
        for input in [
            "Mon, 05 Jan 2026 10:17:19 +2400",
            "Mon, 05 Jan 2026 24:17:19 GMT",
            "Tue, 05 Jan 2026 10:17:19 GMT",
        ] {
            assert_eq!(rfc2822(input), None, "{input:?}");
        }
        assert_eq!(kept("20260105T241719Z"), None);
        assert_eq!(date("2026-02-30"), None);
        assert_eq!(lenient_date("2026-02-30"), None);

        let edge = rfc3339("2026-12-31T23:59:60.5+23:59").unwrap();
        assert_eq!(
            (edge.second, edge.leap, edge.nanos, edge.offset_seconds),
            (59, true, 500_000_000, 86_340)
        );
        let minus = lenient("2026-01-05T10:17:19 -23:59").unwrap();
        assert_eq!(minus.offset_seconds, -86_340);
    }
}
