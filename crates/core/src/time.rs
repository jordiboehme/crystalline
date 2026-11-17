//! Dates and instants for every crate: the one place that knows which date
//! library sits underneath.
//!
//! Every other crate talks to dates through this module only: the newtypes
//! [`Stamp`] (a UTC instant), [`OffsetStamp`] (an instant plus the offset it
//! was written with), [`Day`] (a calendar date) and [`Delta`] (an exact
//! span of seconds and nanoseconds), the clock ([`now`], [`today_utc`]), the
//! duration helpers ([`days`], [`hours`], [`minutes`], [`seconds`],
//! [`millis`]) and the named emitters and parsers. None of the newtypes
//! implements `Display` or serde's traits, so every byte that leaves
//! Crystalline goes through a helper named here.
//!
//! The parsers ([`parse_rfc3339`], [`parse_lenient_stamp`],
//! [`parse_lenient_offset`], [`parse_date`], [`parse_lenient_date`],
//! [`parse_kept_stamp`], [`parse_rfc2822`]) and the RFC 3339 emitter are
//! written by hand on plain numbers: each grammar reads text into numbers,
//! and one conversion below turns the numbers into a value. That conversion
//! is the only place a parser meets the library.
//!
//! The values are still backed by chrono in this stage; the names are what
//! the rest of the workspace moves onto first.

mod calendar;
mod emit;
mod parse;
pub mod serde;

use chrono::{
    DateTime, Days, FixedOffset, NaiveDate, NaiveTime, SecondsFormat, TimeDelta, TimeZone, Utc,
};
use parse::Parts;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A UTC instant with nanosecond precision.
///
/// `Debug` prints what chrono's `Debug` printed: `2026-01-05T10:17:19Z`.
/// `Default` is the Unix epoch, as for chrono's `DateTime<Utc>`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp(DateTime<Utc>);

/// An instant together with the UTC offset it was written with.
///
/// Equality, ordering and hashing look at the instant only, so
/// `10:00+02:00` and `08:00+00:00` are the same value; the offset matters
/// only when the value is written out again.
///
/// `Debug` prints what chrono's `Debug` printed: the local date and time
/// and the exact offset, `2026-01-05T12:17:19+02:00`.
#[derive(Clone, Copy)]
pub struct OffsetStamp(DateTime<FixedOffset>);

/// A calendar date with no time of day and no offset.
///
/// `Debug` prints what chrono's `Debug` printed: `2026-01-05`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day(NaiveDate);

/// An exact span of time: whole seconds plus nanoseconds, where a day is
/// always 86400 seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Delta(TimeDelta);

/// The current instant from the system clock.
pub fn now() -> Stamp {
    Stamp(Utc::now())
}

/// Today's date in UTC.
pub fn today_utc() -> Day {
    now().day_utc()
}

/// `n` days of 86400 seconds each. Panics when the span is out of range,
/// as chrono's `Duration::days` does.
pub const fn days(n: i64) -> Delta {
    Delta(TimeDelta::days(n))
}

/// `n` hours. Panics when the span is out of range.
pub const fn hours(n: i64) -> Delta {
    Delta(TimeDelta::hours(n))
}

/// `n` minutes. Panics when the span is out of range.
pub const fn minutes(n: i64) -> Delta {
    Delta(TimeDelta::minutes(n))
}

/// `n` seconds. Panics when the span is out of range.
pub const fn seconds(n: i64) -> Delta {
    Delta(TimeDelta::seconds(n))
}

/// `n` milliseconds.
pub const fn millis(n: i64) -> Delta {
    Delta(TimeDelta::milliseconds(n))
}

/// `n` seconds, or `None` when the span is out of range. For a number from
/// outside input, where [`seconds`] would panic.
pub fn try_seconds(n: i64) -> Option<Delta> {
    TimeDelta::try_seconds(n).map(Delta)
}

impl Delta {
    /// The empty span.
    pub const ZERO: Delta = Delta(TimeDelta::zero());

    /// Whole days in the span, truncated toward zero.
    pub fn num_days(&self) -> i64 {
        self.0.num_days()
    }

    /// Whole hours in the span, truncated toward zero.
    pub fn num_hours(&self) -> i64 {
        self.0.num_hours()
    }

    /// Whole minutes in the span, truncated toward zero.
    pub fn num_minutes(&self) -> i64 {
        self.0.num_minutes()
    }

    /// Whole seconds in the span, truncated toward zero.
    pub fn num_seconds(&self) -> i64 {
        self.0.num_seconds()
    }

    /// Whole milliseconds in the span, truncated toward zero.
    pub fn num_millis(&self) -> i64 {
        self.0.num_milliseconds()
    }

    /// The span as a `std::time::Duration`, or `None` when it is negative.
    pub fn to_std(&self) -> Option<Duration> {
        self.0.to_std().ok()
    }

    /// Wraps a chrono span. Removed in stage 3.
    #[doc(hidden)]
    pub fn from_chrono(value: TimeDelta) -> Self {
        Delta(value)
    }

    /// The chrono span behind this value. Removed in stage 3.
    #[doc(hidden)]
    pub fn into_chrono(self) -> TimeDelta {
        self.0
    }
}

impl Neg for Delta {
    type Output = Delta;

    fn neg(self) -> Delta {
        Delta(-self.0)
    }
}

impl Add for Delta {
    type Output = Delta;

    fn add(self, rhs: Delta) -> Delta {
        Delta(self.0 + rhs.0)
    }
}

impl Sub for Delta {
    type Output = Delta;

    fn sub(self, rhs: Delta) -> Delta {
        Delta(self.0 - rhs.0)
    }
}

impl Mul<i32> for Delta {
    type Output = Delta;

    fn mul(self, rhs: i32) -> Delta {
        Delta(self.0 * rhs)
    }
}

impl Stamp {
    /// 1970-01-01T00:00:00Z.
    pub const UNIX_EPOCH: Stamp = Stamp(DateTime::UNIX_EPOCH);

    /// The instant at this UTC date and time of day, or `None` when there is
    /// no such date or time (second 0 to 59) or it is out of range.
    pub fn from_ymd_hms(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Option<Stamp> {
        let date = NaiveDate::from_ymd_opt(year, month, day)?;
        date.and_hms_opt(hour, minute, second)
            .map(|t| Stamp(t.and_utc()))
    }

    /// The instant a `SystemTime` names, or `None` when it is out of range
    /// (where chrono's `From<SystemTime>` panicked).
    pub fn from_system_time(time: SystemTime) -> Option<Stamp> {
        match time.duration_since(UNIX_EPOCH) {
            Ok(after) => {
                Stamp::from_unix(i64::try_from(after.as_secs()).ok()?, after.subsec_nanos())
            }
            Err(before) => {
                let before = before.duration();
                let secs = i64::try_from(before.as_secs()).ok()?;
                match before.subsec_nanos() {
                    0 => Stamp::from_unix(secs.checked_neg()?, 0),
                    nanos => Stamp::from_unix(-secs - 1, 1_000_000_000 - nanos),
                }
            }
        }
    }

    /// The instant `micros` microseconds after the Unix epoch, or `None` when
    /// it is out of range.
    pub fn from_unix_micros(micros: i64) -> Option<Stamp> {
        DateTime::from_timestamp_micros(micros).map(Stamp)
    }

    /// The instant `secs` seconds and `nanos` nanoseconds after the Unix
    /// epoch, or `None` when it is out of range or `nanos` is not below one
    /// second.
    pub fn from_unix(secs: i64, nanos: u32) -> Option<Stamp> {
        if nanos >= 1_000_000_000 {
            return None;
        }
        DateTime::from_timestamp(secs, nanos).map(Stamp)
    }

    /// The instant `ms` milliseconds after the Unix epoch, or `None` when it
    /// is out of range.
    pub fn from_unix_millis(ms: i64) -> Option<Stamp> {
        DateTime::from_timestamp_millis(ms).map(Stamp)
    }

    /// Whole seconds since the Unix epoch, rounded toward negative infinity.
    pub fn unix_seconds(&self) -> i64 {
        self.0.timestamp()
    }

    /// Whole milliseconds since the Unix epoch, rounded toward negative
    /// infinity.
    pub fn unix_millis(&self) -> i64 {
        self.0.timestamp_millis()
    }

    /// Whole microseconds since the Unix epoch, rounded toward negative
    /// infinity.
    pub fn unix_micros(&self) -> i64 {
        self.0.timestamp_micros()
    }

    /// The nanoseconds past the whole second.
    pub fn subsec_nanos(&self) -> u32 {
        self.0.timestamp_subsec_nanos()
    }

    /// This instant moved forward by `delta`, or `None` when out of range.
    pub fn checked_add(self, delta: Delta) -> Option<Stamp> {
        self.0.checked_add_signed(delta.0).map(Stamp)
    }

    /// This instant moved back by `delta`, or `None` when out of range.
    pub fn checked_sub(self, delta: Delta) -> Option<Stamp> {
        self.0.checked_sub_signed(delta.0).map(Stamp)
    }

    /// The span from `earlier` to this instant (negative when `earlier` is
    /// in fact later).
    pub fn since(self, earlier: Stamp) -> Delta {
        Delta(self.0.signed_duration_since(earlier.0))
    }

    /// The UTC calendar date of this instant.
    pub fn day_utc(self) -> Day {
        Day(self.0.date_naive())
    }

    /// This instant with offset zero.
    pub fn to_offset_utc(self) -> OffsetStamp {
        OffsetStamp(self.0.fixed_offset())
    }

    /// RFC 3339 with `+00:00` and the fraction in groups of none, 3, 6 or 9
    /// digits, as chrono's `to_rfc3339` writes it.
    pub fn rfc3339(&self) -> String {
        emit::rfc3339(self.unix_seconds(), self.subsec_nanos(), 0)
    }

    /// RFC 3339 in whole seconds with `Z`: `2026-01-05T10:17:19Z`.
    pub fn rfc3339_secs_z(&self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// RFC 3339 in whole seconds with `+00:00`: `2026-01-05T10:17:19+00:00`.
    pub fn rfc3339_secs(&self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Secs, false)
    }

    /// chrono's `Display` of a UTC instant, for log lines:
    /// `2026-01-05 10:17:19 UTC`, `2026-01-05 10:17:19.120 UTC`.
    pub fn display_utc(&self) -> String {
        emit::display_utc(self.unix_seconds(), self.subsec_nanos())
    }

    /// The HTTP date (RFC 9110's IMF-fixdate): `Mon, 05 Jan 2026 10:17:19
    /// GMT`, as chrono's `%a, %d %b %Y %H:%M:%S GMT` writes it.
    pub fn http_date(&self) -> String {
        emit::http_date(self.unix_seconds(), self.subsec_nanos())
    }

    /// `2026-01-05 10:17 UTC`.
    pub fn ymd_hm_utc(&self) -> String {
        self.0.format("%Y-%m-%d %H:%M UTC").to_string()
    }

    /// `20260105T101719.120Z`, always three fraction digits.
    pub fn compact_millis_z(&self) -> String {
        self.0.format("%Y%m%dT%H%M%S%.3fZ").to_string()
    }

    /// `20260105T101719Z`.
    pub fn compact_z(&self) -> String {
        self.0.format("%Y%m%dT%H%M%SZ").to_string()
    }

    /// `260105101719`: two-digit year, then month, day, hour, minute and
    /// second.
    pub fn yymmddhhmmss(&self) -> String {
        self.0.format("%y%m%d%H%M%S").to_string()
    }

    /// Wraps a chrono instant. Removed in stage 3.
    #[doc(hidden)]
    pub fn from_chrono(value: DateTime<Utc>) -> Self {
        Stamp(value)
    }

    /// The chrono instant behind this value. Removed in stage 3.
    #[doc(hidden)]
    pub fn into_chrono(self) -> DateTime<Utc> {
        self.0
    }
}

impl OffsetStamp {
    /// The instant, without the offset.
    pub fn stamp(&self) -> Stamp {
        Stamp(self.0.to_utc())
    }

    /// The date in the written offset: `2026-01-05T23:30:00-05:00` is the 5th,
    /// though it is the 6th in UTC.
    pub fn day(&self) -> Day {
        Day(self.0.date_naive())
    }

    /// The written offset in seconds east of UTC.
    pub fn offset_seconds(&self) -> i32 {
        self.0.offset().local_minus_utc()
    }

    /// RFC 3339 in the written offset (`+00:00` for offset zero) with the
    /// fraction in groups of none, 3, 6 or 9 digits, as chrono's
    /// `to_rfc3339` writes it.
    pub fn rfc3339(&self) -> String {
        let instant = self.stamp();
        emit::rfc3339(
            instant.unix_seconds(),
            instant.subsec_nanos(),
            self.offset_seconds(),
        )
    }

    /// Wraps a chrono instant with offset. Removed in stage 3.
    #[doc(hidden)]
    pub fn from_chrono(value: DateTime<FixedOffset>) -> Self {
        OffsetStamp(value)
    }

    /// The chrono instant with offset behind this value. Removed in stage 3.
    #[doc(hidden)]
    pub fn into_chrono(self) -> DateTime<FixedOffset> {
        self.0
    }
}

impl Default for Stamp {
    fn default() -> Stamp {
        Stamp::UNIX_EPOCH
    }
}

impl fmt::Debug for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&emit::debug_utc(self.unix_seconds(), self.subsec_nanos()))
    }
}

impl fmt::Debug for OffsetStamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let instant = self.stamp();
        f.write_str(&emit::debug_offset(
            instant.unix_seconds(),
            instant.subsec_nanos(),
            self.offset_seconds(),
        ))
    }
}

impl fmt::Debug for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (month, day) = (self.month() as u8, self.day() as u8);
        f.write_str(&emit::date(i64::from(self.year()), month, day))
    }
}

/// Moves the instant forward. Panics when out of range, as chrono's
/// `DateTime + TimeDelta` does; [`Stamp::checked_add`] does not.
impl Add<Delta> for Stamp {
    type Output = Stamp;

    fn add(self, rhs: Delta) -> Stamp {
        self.checked_add(rhs).expect("`Stamp + Delta` overflowed")
    }
}

/// Moves the instant back. Panics when out of range, as chrono's
/// `DateTime - TimeDelta` does; [`Stamp::checked_sub`] does not.
impl Sub<Delta> for Stamp {
    type Output = Stamp;

    fn sub(self, rhs: Delta) -> Stamp {
        self.checked_sub(rhs).expect("`Stamp - Delta` overflowed")
    }
}

impl AddAssign<Delta> for Stamp {
    fn add_assign(&mut self, rhs: Delta) {
        *self = *self + rhs;
    }
}

impl SubAssign<Delta> for Stamp {
    fn sub_assign(&mut self, rhs: Delta) {
        *self = *self - rhs;
    }
}

/// The span between two instants, as [`Stamp::since`].
impl Sub<Stamp> for Stamp {
    type Output = Delta;

    fn sub(self, rhs: Stamp) -> Delta {
        self.since(rhs)
    }
}

/// Moves the date by the whole days in the span (the rest is dropped).
/// Panics when out of range, as chrono's `NaiveDate + TimeDelta` does.
impl Add<Delta> for Day {
    type Output = Day;

    fn add(self, rhs: Delta) -> Day {
        Day(self.0 + rhs.0)
    }
}

/// Moves the date back by the whole days in the span (the rest is
/// dropped). Panics when out of range, as chrono's `NaiveDate - TimeDelta`
/// does.
impl Sub<Delta> for Day {
    type Output = Day;

    fn sub(self, rhs: Delta) -> Day {
        Day(self.0 - rhs.0)
    }
}

impl PartialEq for OffsetStamp {
    fn eq(&self, other: &Self) -> bool {
        self.stamp() == other.stamp()
    }
}

impl Eq for OffsetStamp {}

impl PartialOrd for OffsetStamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OffsetStamp {
    fn cmp(&self, other: &Self) -> Ordering {
        self.stamp().cmp(&other.stamp())
    }
}

impl Hash for OffsetStamp {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.stamp().hash(state);
    }
}

impl Day {
    /// The date with this year, month (1 to 12) and day of the month, or
    /// `None` when there is no such date.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Option<Day> {
        NaiveDate::from_ymd_opt(year, month, day).map(Day)
    }

    /// The year.
    pub fn year(&self) -> i32 {
        chrono::Datelike::year(&self.0)
    }

    /// The month, 1 to 12.
    pub fn month(&self) -> u32 {
        chrono::Datelike::month(&self.0)
    }

    /// The day of the month, starting at 1.
    pub fn day(&self) -> u32 {
        chrono::Datelike::day(&self.0)
    }

    /// The date `n` calendar days later (earlier for a negative `n`), or
    /// `None` when out of range.
    pub fn checked_add_days(self, n: i64) -> Option<Day> {
        let step = Days::new(n.unsigned_abs());
        if n >= 0 {
            self.0.checked_add_days(step)
        } else {
            self.0.checked_sub_days(step)
        }
        .map(Day)
    }

    /// The date `n` calendar days earlier, held at the earliest date when
    /// that is out of range. A negative `n` moves forward and is held at the
    /// latest date. Never panics.
    pub fn saturating_sub_days(self, n: i64) -> Day {
        let step = Days::new(n.unsigned_abs());
        if n >= 0 {
            self.0
                .checked_sub_days(step)
                .map_or(Day(NaiveDate::MIN), Day)
        } else {
            self.0
                .checked_add_days(step)
                .map_or(Day(NaiveDate::MAX), Day)
        }
    }

    /// Whole days from this date to `later`: `a.days_until(b)` is `b - a`,
    /// negative when `later` is in fact earlier.
    pub fn days_until(self, later: Day) -> i64 {
        later.0.signed_duration_since(self.0).num_days()
    }

    /// Midnight UTC at the start of this date, or `None` when out of range.
    pub fn start_utc(self) -> Option<Stamp> {
        self.0.and_hms_opt(0, 0, 0).map(|t| Stamp(t.and_utc()))
    }

    /// `2026-01-05`.
    pub fn ymd(&self) -> String {
        self.0.format("%Y-%m-%d").to_string()
    }

    /// Wraps a chrono date. Removed in stage 3.
    #[doc(hidden)]
    pub fn from_chrono(value: NaiveDate) -> Self {
        Day(value)
    }

    /// The chrono date behind this value. Removed in stage 3.
    #[doc(hidden)]
    pub fn into_chrono(self) -> NaiveDate {
        self.0
    }
}

/// A strict RFC 3339 instant, as chrono's `DateTime::parse_from_rfc3339`
/// reads it: `2026-01-05T10:17:19Z`, with `t` or a space allowed for `T`,
/// an optional fraction of any length (nine digits kept), and `Z`, `z` or
/// `+hh:mm`/`-hh:mm` (the minus also as U+2212). A leap second `:60` is kept.
pub fn parse_rfc3339(s: &str) -> Option<OffsetStamp> {
    parse::rfc3339(s).and_then(offset_stamp_from_parts)
}

/// An instant as chrono's `FromStr` and `Deserialize` for `DateTime<Utc>`
/// read it: the grammar of [`parse_lenient_offset`], moved to UTC.
pub fn parse_lenient_stamp(s: &str) -> Option<Stamp> {
    parse_lenient_offset(s).map(|value| value.stamp())
}

/// An instant with its offset as chrono's `FromStr` and `Deserialize` for
/// `DateTime<FixedOffset>` read it: RFC 3339 plus white space around the
/// value and its parts, unpadded fields (`2026-1-5T1:7:9Z`), a signed year
/// of any length, `UTC` as an offset, and an offset written `+0200` or
/// `+02 00`.
pub fn parse_lenient_offset(s: &str) -> Option<OffsetStamp> {
    parse::lenient(s).and_then(offset_stamp_from_parts)
}

/// A date as chrono's `NaiveDate::parse_from_str(s, "%Y-%m-%d")` reads it:
/// `2026-01-05`, also `2026-1-5` and a leading space before each number.
pub fn parse_date(s: &str) -> Option<Day> {
    parse::date(s).and_then(day_from_parts)
}

/// A date as chrono's `FromStr` and `Deserialize` for `NaiveDate` read it:
/// the grammar of [`parse_date`] plus white space before each `-` and at the
/// end (`2026 - 01 - 05 `).
pub fn parse_lenient_date(s: &str) -> Option<Day> {
    parse::lenient_date(s).and_then(day_from_parts)
}

/// A kept-file stamp, `20260105T101719Z`, as chrono's
/// `NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ")` reads it, taken as
/// UTC. A leading `+` makes the year swallow every digit, so
/// `+20260105T101719Z` is refused.
pub fn parse_kept_stamp(s: &str) -> Option<Stamp> {
    parse::kept(s)
        .and_then(offset_stamp_from_parts)
        .map(|value| value.stamp())
}

/// An RFC 2822 date (the HTTP `Date` header), as chrono's
/// `DateTime::parse_from_rfc2822` reads it: `Mon, 5 Jan 2026 10:17:19 GMT`.
pub fn parse_rfc2822(s: &str) -> Option<OffsetStamp> {
    parse::rfc2822(s).and_then(offset_stamp_from_parts)
}

// The one place a parser meets the library: numbers from a grammar become a
// value here, and only here. A leap second becomes chrono's leap nanosecond
// (second 59 plus a full second of nanoseconds), so nothing changes; the
// library also decides which years and instants it can hold.

fn offset_stamp_from_parts(parts: Parts) -> Option<OffsetStamp> {
    let date = NaiveDate::from_ymd_opt(parts.year, parts.month.into(), parts.day.into())?;
    let leap = if parts.leap { 1_000_000_000 } else { 0 };
    let time = NaiveTime::from_hms_nano_opt(
        parts.hour.into(),
        parts.minute.into(),
        parts.second.into(),
        parts.nanos + leap,
    )?;
    let offset = FixedOffset::east_opt(parts.offset_seconds)?;
    offset
        .from_local_datetime(&date.and_time(time))
        .single()
        .map(OffsetStamp)
}

fn day_from_parts(parts: Parts) -> Option<Day> {
    NaiveDate::from_ymd_opt(parts.year, parts.month.into(), parts.day.into()).map(Day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    /// 2026-01-05T10:17:19Z.
    const BASE_SECS: i64 = 1_767_608_239;

    const NANOS: [u32; 5] = [0, 500_000_000, 120_000_000, 123_450_000, 123_456_789];

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn chrono_utc(secs: i64, nanos: u32) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, nanos).unwrap()
    }

    #[test]
    fn base_instant_is_the_documented_one() {
        let stamp = Stamp::from_unix(BASE_SECS, 0).unwrap();
        assert_eq!(stamp.rfc3339_secs_z(), "2026-01-05T10:17:19Z");
    }

    #[test]
    fn rfc3339_matches_chrono() {
        for nanos in NANOS {
            let stamp = Stamp::from_unix(BASE_SECS, nanos).unwrap();
            let reference = chrono_utc(BASE_SECS, nanos);
            assert_eq!(stamp.rfc3339(), reference.to_rfc3339(), "nanos {nanos}");
            assert_eq!(
                stamp.to_offset_utc().rfc3339(),
                reference.to_rfc3339(),
                "nanos {nanos}"
            );
            for offset in [0, 2 * 3600, -(4 * 3600 + 30 * 60)] {
                let zone = FixedOffset::east_opt(offset).unwrap();
                let written = reference.with_timezone(&zone);
                let value = OffsetStamp::from_chrono(written);
                assert_eq!(value.rfc3339(), written.to_rfc3339(), "{nanos} {offset}");
                assert_eq!(value.offset_seconds(), offset);
                assert_eq!(value.stamp(), stamp);
            }
        }
        let fractions: Vec<String> = NANOS
            .iter()
            .map(|n| Stamp::from_unix(BASE_SECS, *n).unwrap().rfc3339())
            .collect();
        assert_eq!(
            fractions,
            [
                "2026-01-05T10:17:19+00:00",
                "2026-01-05T10:17:19.500+00:00",
                "2026-01-05T10:17:19.120+00:00",
                "2026-01-05T10:17:19.123450+00:00",
                "2026-01-05T10:17:19.123456789+00:00",
            ]
        );
        let zone = FixedOffset::west_opt(4 * 3600 + 30 * 60).unwrap();
        let written = OffsetStamp::from_chrono(chrono_utc(BASE_SECS, 0).with_timezone(&zone));
        assert_eq!(written.rfc3339(), "2026-01-05T05:47:19-04:30");
        let reference = chrono_utc(BASE_SECS, 123_450_000);
        let plus_two = FixedOffset::east_opt(2 * 3600).unwrap();
        let written = OffsetStamp::from_chrono(reference.with_timezone(&plus_two));
        assert_eq!(written.rfc3339(), "2026-01-05T12:17:19.123450+02:00");
        let written = OffsetStamp::from_chrono(reference.with_timezone(&zone));
        assert_eq!(written.rfc3339(), "2026-01-05T05:47:19.123450-04:30");
    }

    #[test]
    fn secs_forms_match_chrono() {
        for nanos in NANOS {
            let stamp = Stamp::from_unix(BASE_SECS, nanos).unwrap();
            let reference = chrono_utc(BASE_SECS, nanos);
            assert_eq!(
                stamp.rfc3339_secs_z(),
                reference.to_rfc3339_opts(SecondsFormat::Secs, true)
            );
            assert_eq!(
                stamp.rfc3339_secs(),
                reference.to_rfc3339_opts(SecondsFormat::Secs, false)
            );
        }
        let stamp = Stamp::from_unix(BASE_SECS, 123_456_789).unwrap();
        assert_eq!(stamp.rfc3339_secs_z(), "2026-01-05T10:17:19Z");
        assert_eq!(stamp.rfc3339_secs(), "2026-01-05T10:17:19+00:00");
    }

    #[test]
    fn compact_formats_match_chrono() {
        for nanos in NANOS {
            let stamp = Stamp::from_unix(BASE_SECS, nanos).unwrap();
            let reference = chrono_utc(BASE_SECS, nanos);
            assert_eq!(
                stamp.day_utc().ymd(),
                reference.format("%Y-%m-%d").to_string()
            );
            assert_eq!(
                stamp.ymd_hm_utc(),
                reference.format("%Y-%m-%d %H:%M UTC").to_string()
            );
            assert_eq!(
                stamp.compact_millis_z(),
                reference.format("%Y%m%dT%H%M%S%.3fZ").to_string()
            );
            assert_eq!(
                stamp.compact_z(),
                reference.format("%Y%m%dT%H%M%SZ").to_string()
            );
            assert_eq!(
                stamp.yymmddhhmmss(),
                reference.format("%y%m%d%H%M%S").to_string()
            );
        }
        let whole = Stamp::from_unix(BASE_SECS, 0).unwrap();
        assert_eq!(whole.day_utc().ymd(), "2026-01-05");
        assert_eq!(whole.ymd_hm_utc(), "2026-01-05 10:17 UTC");
        assert_eq!(whole.compact_millis_z(), "20260105T101719.000Z");
        assert_eq!(whole.compact_z(), "20260105T101719Z");
        assert_eq!(whole.yymmddhhmmss(), "260105101719");
        let fine = Stamp::from_unix(BASE_SECS, 123_456_789).unwrap();
        assert_eq!(fine.compact_millis_z(), "20260105T101719.123Z");
    }

    #[test]
    fn offset_stamps_compare_by_instant() {
        let plus_two = FixedOffset::east_opt(2 * 3600).unwrap();
        let at_ten =
            OffsetStamp::from_chrono(plus_two.with_ymd_and_hms(2026, 1, 5, 10, 0, 0).unwrap());
        let at_eight = Day::from_ymd(2026, 1, 5)
            .unwrap()
            .start_utc()
            .unwrap()
            .checked_add(hours(8))
            .unwrap()
            .to_offset_utc();
        assert_eq!(at_ten.offset_seconds(), 7200);
        assert_eq!(at_eight.offset_seconds(), 0);
        assert_eq!(at_ten, at_eight);
        assert_eq!(hash_of(&at_ten), hash_of(&at_eight));
        assert_eq!(at_ten.cmp(&at_eight), Ordering::Equal);
        assert_ne!(at_ten.rfc3339(), at_eight.rfc3339());

        let later = at_eight
            .stamp()
            .checked_add(millis(1))
            .unwrap()
            .to_offset_utc();
        assert!(at_ten < later);
        assert_eq!([later, at_ten].iter().max_by_key(|s| **s), Some(&later));
    }

    #[test]
    fn text_order_is_instant_order() {
        // Each second's fractions cover the four AutoSi widths, including a
        // whole second right before one nanosecond past it, where `+` meets
        // `.`; the seconds cross a minute boundary.
        const FRACTIONS: [u32; 10] = [
            0,
            1,
            999,
            1_000,
            100_000_000,
            100_000_001,
            120_000_000,
            123_450_000,
            500_000_000,
            999_999_999,
        ];
        let start = BASE_SECS + 31; // 10:17:50
        let stamps: Vec<Stamp> = (0..20)
            .flat_map(|s| FRACTIONS.map(move |n| Stamp::from_unix(start + s, n).unwrap()))
            .collect();
        assert_eq!(stamps.len(), 200);
        assert!(stamps.windows(2).all(|w| w[0] < w[1]));
        let in_order: Vec<String> = stamps.iter().map(Stamp::rfc3339).collect();
        let mut as_text = in_order.clone();
        as_text.reverse();
        as_text.sort();
        assert_eq!(as_text, in_order);
        let offset_in_order: Vec<String> =
            stamps.iter().map(|s| s.to_offset_utc().rfc3339()).collect();
        let mut offset_as_text = offset_in_order.clone();
        offset_as_text.reverse();
        offset_as_text.sort();
        assert_eq!(offset_as_text, offset_in_order);
    }

    #[test]
    fn delta_arithmetic_matches_chrono() {
        let cases = [
            (days(3), TimeDelta::days(3)),
            (days(-3), TimeDelta::days(-3)),
            (hours(49), TimeDelta::hours(49)),
            (hours(-49), TimeDelta::hours(-49)),
            (minutes(-90), TimeDelta::minutes(-90)),
            (seconds(3599), TimeDelta::seconds(3599)),
            (seconds(-3599), TimeDelta::seconds(-3599)),
            (millis(1_999), TimeDelta::milliseconds(1_999)),
            (millis(-1_999), TimeDelta::milliseconds(-1_999)),
            (Delta::ZERO, TimeDelta::zero()),
        ];
        for (delta, reference) in cases {
            assert_eq!(delta.num_days(), reference.num_days(), "{reference:?}");
            assert_eq!(delta.num_hours(), reference.num_hours(), "{reference:?}");
            assert_eq!(
                delta.num_minutes(),
                reference.num_minutes(),
                "{reference:?}"
            );
            assert_eq!(
                delta.num_seconds(),
                reference.num_seconds(),
                "{reference:?}"
            );
            assert_eq!(
                delta.num_millis(),
                reference.num_milliseconds(),
                "{reference:?}"
            );
            assert_eq!((-delta).num_millis(), (-reference).num_milliseconds());
            assert_eq!((delta * 3).num_millis(), (reference * 3).num_milliseconds());
        }
        // Truncation toward zero.
        assert_eq!(hours(-49).num_days(), -2);
        assert_eq!(seconds(-3599).num_hours(), 0);
        assert_eq!(millis(-1_999).num_seconds(), -1);
        assert_eq!((days(1) + hours(1)).num_hours(), 25);
        assert_eq!((days(1) - hours(1)).num_hours(), 23);
        assert!(minutes(59) < hours(1));
        assert!(-seconds(1) < Delta::ZERO);

        let base = Stamp::from_unix(BASE_SECS, 120_000_000).unwrap();
        let reference = chrono_utc(BASE_SECS, 120_000_000);
        let later = base.checked_add(days(2) + millis(5)).unwrap();
        assert_eq!(
            later.rfc3339(),
            (reference + TimeDelta::days(2) + TimeDelta::milliseconds(5)).to_rfc3339()
        );
        assert_eq!(later.since(base).num_millis(), 2 * 86_400_000 + 5);
        assert_eq!(base.since(later).num_millis(), -(2 * 86_400_000 + 5));
        assert_eq!(later.checked_sub(days(2) + millis(5)), Some(base));
        assert_eq!(base.unix_seconds(), BASE_SECS);
        assert_eq!(base.unix_millis(), BASE_SECS * 1000 + 120);
        assert_eq!(base.subsec_nanos(), 120_000_000);
        assert_eq!(Stamp::from_unix_millis(BASE_SECS * 1000 + 120), Some(base));
        assert_eq!(Stamp::from_unix(BASE_SECS, 1_000_000_000), None);
        assert_eq!(Stamp::from_unix(i64::MAX, 0), None);
        assert_eq!(base.checked_add(days(400_000_000)), None);
    }

    #[test]
    fn day_arithmetic_and_accessors() {
        let day = Day::from_ymd(2026, 1, 5).unwrap();
        assert_eq!((day.year(), day.month(), day.day()), (2026, 1, 5));
        assert_eq!(Day::from_ymd(2026, 2, 30), None);
        assert_eq!(day.checked_add_days(31).unwrap().ymd(), "2026-02-05");
        assert_eq!(day.checked_add_days(-5).unwrap().ymd(), "2025-12-31");
        assert_eq!(day.checked_add_days(i64::MAX), None);
        assert_eq!(day.saturating_sub_days(5).ymd(), "2025-12-31");
        assert_eq!(day.saturating_sub_days(-27).ymd(), "2026-02-01");
        let later = Day::from_ymd(2026, 3, 1).unwrap();
        assert_eq!(day.days_until(later), 55);
        assert_eq!(later.days_until(day), -55);
        assert_eq!(
            day.days_until(later),
            later
                .into_chrono()
                .signed_duration_since(day.into_chrono())
                .num_days()
        );
        assert_eq!(
            day.start_utc().unwrap().rfc3339(),
            "2026-01-05T00:00:00+00:00"
        );
        assert!(day < later);
        // Two clock reads may straddle UTC midnight, so today may be one day
        // after the first read's date, never anything else.
        let before = now().day_utc();
        let today = today_utc();
        assert!(today == before || Some(today) == before.checked_add_days(1));
    }

    /// Requires `ours` to agree with chrono's `theirs` on `input`: both
    /// refuse, or both accept with the same instant, offset and RFC 3339
    /// bytes. Returns whether the input was accepted.
    fn agree_offset(
        input: &str,
        ours: Option<OffsetStamp>,
        theirs: Option<DateTime<FixedOffset>>,
    ) -> bool {
        match (ours, theirs) {
            (None, None) => false,
            (Some(ours), Some(theirs)) => {
                assert_eq!(ours.into_chrono(), theirs, "{input:?}");
                assert_eq!(
                    ours.offset_seconds(),
                    theirs.offset().local_minus_utc(),
                    "{input:?}"
                );
                assert_eq!(ours.rfc3339(), theirs.to_rfc3339(), "{input:?}");
                true
            }
            (ours, theirs) => panic!("{input:?}: ours {ours:?}, chrono {theirs:?}"),
        }
    }

    /// As [`agree_offset`] for a UTC instant.
    fn agree_stamp(input: &str, ours: Option<Stamp>, theirs: Option<DateTime<Utc>>) -> bool {
        match (ours, theirs) {
            (None, None) => false,
            (Some(ours), Some(theirs)) => {
                assert_eq!(ours.into_chrono(), theirs, "{input:?}");
                assert_eq!(ours.rfc3339(), theirs.to_rfc3339(), "{input:?}");
                true
            }
            (ours, theirs) => panic!("{input:?}: ours {ours:?}, chrono {theirs:?}"),
        }
    }

    /// As [`agree_offset`] for a date.
    fn agree_day(input: &str, ours: Option<Day>, theirs: Option<NaiveDate>) -> bool {
        match (ours, theirs) {
            (None, None) => false,
            (Some(ours), Some(theirs)) => {
                assert_eq!(ours.into_chrono(), theirs, "{input:?}");
                true
            }
            (ours, theirs) => panic!("{input:?}: ours {ours:?}, chrono {theirs:?}"),
        }
    }

    #[test]
    fn grammar_rows_match_chrono() {
        // Every row of the J2 table in the design review, then the forms both
        // libraries read alike. The flag is chrono's answer.
        let rows: &[(&str, bool)] = &[
            ("2026-01-05T10:17:19+0200", false),
            ("2026-01-05T10:17:19+02", false),
            ("2026-01-05T10:17:19+02:00:30", false),
            ("2026-01-05T10:17Z", false),
            ("2026-01-05T10Z", false),
            ("20260105T101719Z", false),
            ("2026-01-05T10:17:19,5Z", false),
            ("2026-01-05T10:17:19Z[UTC]", false),
            ("2026-01-05T10:17:19+02:00[Europe/Berlin]", false),
            ("2026-01-05T10:17:19+24:00", false),
            ("2026-01-05T10:17:19+25:59", false),
            ("+002026-01-05T10:17:19Z", false),
            ("2026-01-05T10:17:19.1234567891Z", true),
            ("2026-01-05T10:17:19\u{2212}02:00", true),
            ("9999-12-31T23:59:59Z", true),
            ("2026-12-31T23:59:60Z", true),
            ("2026-01-05T10:17:60Z", true),
            ("2026-01-05t10:17:19Z", true),
            ("2026-01-05T10:17:19z", true),
            ("2026-01-05 10:17:19Z", true),
            ("2026-01-05T10:17+02:00", false),
            ("2026-01-05T10:17:19-00:00", true),
            ("2026-01-05T10:17:19+00:00", true),
            ("2026-01-05T10:17:19.12+05:30", true),
            ("2026-01-05T10:17:19.123456789-04:30", true),
            ("2026-01-05T10:17:19+23:59", true),
            ("2026-01-05T10:17:19+02:60", false),
            ("2026-01-05T10:17:19.Z", false),
            ("2026-01-05T24:00:00Z", false),
            ("2026-01-05T10:60:00Z", false),
            ("2026-01-05T10:17:61Z", false),
            ("2026-02-29T00:00:00Z", false),
            ("2024-02-29T00:00:00Z", true),
            ("2026-00-05T00:00:00Z", false),
            ("2026-01-00T00:00:00Z", false),
            ("0000-01-01T00:00:00Z", true),
            (" 2026-01-05T10:17:19Z", false),
            ("2026-01-05T10:17:19Z ", false),
            ("2026-01-05T10:17:19 Z", false),
            ("2026-1-05T10:17:19Z", false),
            ("2026-01-05T10:17:19", false),
            ("", false),
        ];
        for (input, accepted) in rows {
            let ours = parse_rfc3339(input);
            let theirs = DateTime::parse_from_rfc3339(input).ok();
            assert_eq!(agree_offset(input, ours, theirs), *accepted, "{input:?}");
        }
        let printed = |s: &str| parse_rfc3339(s).unwrap().rfc3339();
        assert_eq!(printed("2026-12-31T23:59:60Z"), "2026-12-31T23:59:60+00:00");
        assert_eq!(
            printed("2026-01-05T10:17:19.1234567891Z"),
            "2026-01-05T10:17:19.123456789+00:00"
        );
        assert_eq!(
            printed("2026-01-05T10:17:19\u{2212}02:00"),
            "2026-01-05T10:17:19-02:00"
        );
        assert_eq!(
            printed("2026-01-05T10:17:19-00:00"),
            "2026-01-05T10:17:19+00:00"
        );
        assert_eq!(
            printed("2026-01-05t10:17:19.5z"),
            "2026-01-05T10:17:19.500+00:00"
        );
    }

    #[test]
    fn lenient_rows_match_chrono_fromstr() {
        // The J3 forms and their neighbours. The flag is chrono's answer.
        let rows: &[(&str, bool)] = &[
            ("2026-01-05T10:17:19Z", true),
            (" 2026-01-05T10:17:19Z ", true),
            ("\t2026-01-05T10:17:19Z\n", true),
            ("2026-1-5T10:17:19Z", true),
            ("2026-01-05T1:7:9Z", true),
            ("+2026-01-05T10:17:19Z", true),
            ("-0001-01-05T10:17:19Z", true),
            ("-2026-01-05T10:17:19+02:00", true),
            ("2026-01-05T10:17:19 +02:00", true),
            ("2026-01-05T10:17:19+0200", true),
            ("2026-01-05T10:17:19+02 00", true),
            ("2026-01-05T10:17:19+02 :: 00", true),
            ("2026 - 01 - 05T10:17:19Z", true),
            ("2026-01-05T 10 : 17 : 19Z", true),
            ("2026-01-05  10:17:19Z", true),
            ("2026-01-05 T10:17:19Z", false),
            ("2026-01-05T10:17:19UTC", true),
            ("2026-01-05T10:17:19 utc", true),
            ("2026-01-05T10:17:19+02", false),
            ("2026-01-05T10:17Z", false),
            ("2026-01-05T10:17:19 .5Z", false),
            ("2026-01-05T10:17:19.5 Z", true),
            ("2026-01-05T10:17:19.Z", false),
            ("+10000-01-01T00:00:00Z", true),
            ("12026-01-05T10:17:19Z", false),
            ("26-01-05T10:17:19Z", true),
            ("2026-01-05T10:17:60Z", true),
            ("2026-01-05T10:17:19\u{2212}02:00", true),
            ("2026-01-05T10:17:19.1234567891Z", true),
            ("2026-01-05T10:17:19-00:00", true),
            ("2026-01-05T10:17:19+24:00", false),
            ("2026-01-05T10:17:19Z\u{3000}", true),
            ("\u{a0}2026-01-05T10:17:19Z", true),
            ("+262142-12-31T23:59:59Z", true),
            ("+262142-12-31T23:59:59-01:00", false),
            ("+262143-01-01T00:00:00Z", false),
            ("-262143-01-01T00:00:00Z", true),
            ("-262143-01-01T00:00:00+01:00", false),
            ("+2147483648-01-01T00:00:00Z", false),
            ("2026-01-05", false),
            ("2026-01-05T10:17:19Zx", false),
            ("", false),
        ];
        for (input, accepted) in rows {
            let ours = parse_lenient_offset(input);
            let theirs = input.parse::<DateTime<FixedOffset>>().ok();
            assert_eq!(agree_offset(input, ours, theirs), *accepted, "{input:?}");
            let json = serde_json::to_string(input).unwrap();
            let theirs = serde_json::from_str::<DateTime<FixedOffset>>(&json).ok();
            assert_eq!(agree_offset(input, ours, theirs), *accepted, "{input:?}");

            let ours = parse_lenient_stamp(input);
            let theirs = input.parse::<DateTime<Utc>>().ok();
            assert_eq!(agree_stamp(input, ours, theirs), *accepted, "{input:?}");
            let theirs = serde_json::from_str::<DateTime<Utc>>(&json).ok();
            assert_eq!(agree_stamp(input, ours, theirs), *accepted, "{input:?}");
        }
        let printed = |s: &str| parse_lenient_offset(s).unwrap().rfc3339();
        assert_eq!(
            printed("-0001-01-05T10:17:19Z"),
            "-0001-01-05T10:17:19+00:00"
        );
        assert_eq!(
            printed("+10000-01-01T00:00:00Z"),
            "+10000-01-01T00:00:00+00:00"
        );
        assert_eq!(printed("2026-1-5T1:7:9 +0200"), "2026-01-05T01:07:09+02:00");
    }

    #[test]
    fn date_rows_match_chrono() {
        // (input, accepted by parse_from_str "%Y-%m-%d", accepted by FromStr)
        let rows: &[(&str, bool, bool)] = &[
            ("2026-01-05", true, true),
            ("2026-1-5", true, true),
            (" 2026-01-05", true, true),
            ("2026- 01- 05", true, true),
            ("26-01-05", true, true),
            ("2026 - 01 - 05", false, true),
            ("2026-01-05 ", false, true),
            ("+10000-01-01", true, true),
            ("-0001-01-01", true, true),
            ("+2026-01-05", true, true),
            ("12026-01-05", false, false),
            ("20260105", false, false),
            ("2026-01-05T00:00:00", false, false),
            ("2026-01-05[UTC]", false, false),
            ("2026-02-30", false, false),
            ("2024-02-29", true, true),
            ("2026-13-01", false, false),
            ("2026-001-05", false, false),
            ("", false, false),
        ];
        for (input, strict, lenient) in rows {
            let theirs = NaiveDate::parse_from_str(input, "%Y-%m-%d").ok();
            assert_eq!(
                agree_day(input, parse_date(input), theirs),
                *strict,
                "{input:?}"
            );
            let ours = parse_lenient_date(input);
            let theirs = input.parse::<NaiveDate>().ok();
            assert_eq!(agree_day(input, ours, theirs), *lenient, "{input:?}");
            let json = serde_json::to_string(input).unwrap();
            let theirs = serde_json::from_str::<NaiveDate>(&json).ok();
            assert_eq!(agree_day(input, ours, theirs), *lenient, "{input:?}");
        }
    }

    #[test]
    fn kept_stamps_match_chrono() {
        // A signed year takes every digit after the sign, so a leading `+`
        // or `-` is refused unless something stops the digits.
        let rows: &[(&str, bool)] = &[
            ("20260105T101719Z", true),
            ("+20260105T101719Z", false),
            ("-20260105T101719Z", false),
            ("+2026 0105T101719Z", true),
            (" 20260105T101719Z", true),
            ("20260105T101760Z", true),
            ("20260105T241719Z", false),
            ("20260230T101719Z", false),
            ("20260105t101719Z", false),
            ("20260105T101719z", false),
            ("20260105T101719Z ", false),
            ("20260105T101719.5Z", false),
            ("20260105T1017Z", false),
            ("2026-01-05T10:17:19Z", false),
            ("", false),
        ];
        for (input, accepted) in rows {
            let theirs = chrono::NaiveDateTime::parse_from_str(input, "%Y%m%dT%H%M%SZ")
                .ok()
                .map(|t| t.and_utc());
            let ours = parse_kept_stamp(input);
            assert_eq!(agree_stamp(input, ours, theirs), *accepted, "{input:?}");
        }
        assert_eq!(
            parse_kept_stamp("20260105T101719Z")
                .unwrap()
                .rfc3339_secs_z(),
            "2026-01-05T10:17:19Z"
        );
    }

    #[test]
    fn rfc2822_rows_match_chrono() {
        let rows: &[(&str, bool)] = &[
            ("Mon, 05 Jan 2026 10:17:19 +0000", true),
            ("Mon, 5 Jan 2026 10:17:19 GMT", true),
            ("5 Jan 2026 10:17:19 +0200", true),
            ("Tue, 05 Jan 2026 10:17:19 GMT", false),
            ("Mon, 05 Jan 2026 10:17 GMT", true),
            ("Thu, 31 Dec 2026 23:59:60 GMT", true),
            ("Mon, 05 Jan 26 10:17:19 GMT", true),
            ("Mon, 05 Jan 2026 10:17:19 -0500", true),
            ("Mon, 05 Jan 2026 10:17:19 EST", true),
            ("Mon, 05 Jan 2026 10:17:19 +02:00", false),
            ("Mon, 05 Jan 2026 10:17:19 GMT (a (nested) comment)", true),
            ("Mon, 05 Jan 2026 10:17:19 GMT ", false),
            ("Mon, 05 January 2026 10:17:19 GMT", false),
            ("mon, 05 jan 2026 10:17:19 gmt", true),
            ("Mon, 05 Jan 2026 10:17:19 +2400", false),
            ("Monday, 05 Jan 2026 10:17:19 GMT", false),
            ("Mon, 05 Jan 2026 10 : 17:19 GMT", true),
            ("Mon, 05 Jan 2026 10:17: 19 GMT", false),
            ("Mon, 05 Jan 2026 10:17:19 J", false),
            ("Mon, 05 Jan 2026 10:17:19 A", true),
            ("Mon, 05 Jan 2026 10:17:19 XYZ", false),
            ("Mon, 05 Jan 2026 10:17:19 \u{2212}0200", false),
            ("Mon, 05 Jan 2026 10:17:19", false),
            ("", false),
        ];
        for (input, accepted) in rows {
            let ours = parse_rfc2822(input);
            let theirs = DateTime::parse_from_rfc2822(input).ok();
            assert_eq!(agree_offset(input, ours, theirs), *accepted, "{input:?}");
        }
        let leap = parse_rfc2822("Thu, 31 Dec 2026 23:59:60 GMT").unwrap();
        assert_eq!(leap.rfc3339(), "2026-12-31T23:59:60+00:00");
        let single = parse_rfc2822("Mon, 5 Jan 2026 10:17:19 GMT").unwrap();
        assert_eq!(single.rfc3339(), "2026-01-05T10:17:19+00:00");
    }

    #[test]
    fn rfc3339_emitter_corners_match_chrono() {
        let at = |y: i32, m: u32, d: u32, nanos: u32| {
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, nanos)
                .unwrap()
                .and_utc()
        };
        let instants = [
            at(-1, 1, 1, 0),
            at(-10000, 6, 30, 5),
            at(10000, 1, 1, 120_000_000),
            at(262142, 12, 30, 0),
            at(-262143, 1, 2, 0),
            at(9999, 12, 31, 999_999_999),
            at(2026, 12, 31, 1_000_000_000),
            at(2026, 12, 31, 1_123_456_000),
            at(1969, 12, 31, 1),
            at(0, 1, 1, 1_000),
        ];
        let offsets = [0, -29, 29, 30, -30, 86_399, -86_399, 7_200, -16_200, 3_599];
        for instant in instants {
            assert_eq!(Stamp::from_chrono(instant).rfc3339(), instant.to_rfc3339());
            for offset in offsets {
                let written = instant.with_timezone(&FixedOffset::east_opt(offset).unwrap());
                assert_eq!(
                    OffsetStamp::from_chrono(written).rfc3339(),
                    written.to_rfc3339(),
                    "{instant:?} {offset}"
                );
            }
        }
        let written = |offset: i32| {
            let zone = FixedOffset::east_opt(offset).unwrap();
            OffsetStamp::from_chrono(at(2026, 1, 5, 0).with_timezone(&zone)).rfc3339()
        };
        assert_eq!(written(-29), "2026-01-05T23:59:30-00:00");
        assert_eq!(written(86_399), "2026-01-06T23:59:58+24:00");
        assert_eq!(
            Stamp::from_chrono(at(-1, 1, 1, 0)).rfc3339(),
            "-0001-01-01T23:59:59+00:00"
        );
    }

    #[test]
    fn offset_stamp_day_is_the_written_date() {
        let late = parse_rfc3339("2026-01-05T23:30:00-05:00").unwrap();
        assert_eq!(late.day().ymd(), "2026-01-05");
        assert_eq!(late.stamp().day_utc().ymd(), "2026-01-06");
        assert_eq!(late.day().into_chrono(), late.into_chrono().date_naive());
        let early = parse_rfc3339("2026-01-06T00:30:00+02:00").unwrap();
        assert_eq!(early.day().ymd(), "2026-01-06");
        assert_eq!(early.stamp().day_utc().ymd(), "2026-01-05");
    }

    #[test]
    fn duration_helpers_are_const_and_try_seconds_never_panics() {
        const REFRESH: Delta = minutes(5);
        const SPANS: [Delta; 5] = [days(1), hours(2), minutes(3), seconds(4), millis(5)];
        assert_eq!(REFRESH.num_seconds(), 300);
        assert_eq!(
            SPANS.map(|d| d.num_millis()),
            [86_400_000, 7_200_000, 180_000, 4_000, 5]
        );
        assert_eq!(try_seconds(90), Some(seconds(90)));
        assert_eq!(try_seconds(-90), Some(seconds(-90)));
        assert_eq!(try_seconds(i64::MAX), None);
        assert_eq!(try_seconds(i64::MIN), None);
        assert_eq!(
            try_seconds(i64::MAX).is_none(),
            TimeDelta::try_seconds(i64::MAX).is_none()
        );
        assert_eq!(seconds(90).to_std(), Some(Duration::from_secs(90)));
        assert_eq!(millis(1_500).to_std(), Some(Duration::from_millis(1_500)));
        assert_eq!(Delta::ZERO.to_std(), Some(Duration::ZERO));
        assert_eq!(seconds(-1).to_std(), None);
    }

    #[test]
    fn operators_match_chrono() {
        let base = Stamp::from_unix(BASE_SECS, 120_000_000).unwrap();
        let reference = chrono_utc(BASE_SECS, 120_000_000);
        let span = days(2) + millis(5);
        assert_eq!((base + span).into_chrono(), reference + span.into_chrono());
        assert_eq!((base - span).into_chrono(), reference - span.into_chrono());
        assert_eq!((base + span) - base, span);
        assert_eq!(base - (base + span), -span);
        let mut moving = base;
        moving += seconds(1);
        assert_eq!(moving.unix_seconds(), BASE_SECS + 1);
        moving -= seconds(2);
        assert_eq!(moving.unix_seconds(), BASE_SECS - 1);
        let day = Day::from_ymd(2026, 1, 5).unwrap();
        assert_eq!((day + days(1)).ymd(), "2026-01-06");
        assert_eq!((day - days(5)).ymd(), "2025-12-31");
        // Only whole days move a date, as with chrono.
        assert_eq!((day + hours(47)).ymd(), "2026-01-06");
        assert_eq!(
            (day - hours(47)).into_chrono(),
            day.into_chrono() - TimeDelta::hours(47)
        );
    }

    #[test]
    #[should_panic(expected = "`Stamp + Delta` overflowed")]
    fn stamp_plus_delta_panics_out_of_range_as_chrono_does() {
        let _ = Stamp::UNIX_EPOCH + days(400_000_000);
    }

    #[test]
    fn constructors_match_chrono() {
        assert_eq!(Stamp::UNIX_EPOCH.into_chrono(), DateTime::UNIX_EPOCH);
        assert_eq!(Stamp::default(), Stamp::UNIX_EPOCH);
        assert_eq!(Stamp::default().into_chrono(), DateTime::<Utc>::default());

        let built = Stamp::from_ymd_hms(2026, 1, 5, 10, 17, 19).unwrap();
        assert_eq!(built.rfc3339_secs_z(), "2026-01-05T10:17:19Z");
        assert_eq!(
            built.into_chrono(),
            Utc.with_ymd_and_hms(2026, 1, 5, 10, 17, 19).unwrap()
        );
        assert_eq!(Stamp::from_ymd_hms(2026, 2, 30, 0, 0, 0), None);
        assert_eq!(Stamp::from_ymd_hms(2026, 1, 5, 24, 0, 0), None);
        assert_eq!(Stamp::from_ymd_hms(2026, 1, 5, 10, 17, 60), None);

        let times = [
            UNIX_EPOCH + Duration::new(1_767_608_239, 120_000_000),
            UNIX_EPOCH,
            // 100 ns: Windows keeps SystemTime in 100 ns ticks, so 1 ns
            // before the epoch would round to the epoch itself there.
            UNIX_EPOCH - Duration::new(0, 100),
            UNIX_EPOCH - Duration::new(86_400, 0),
            UNIX_EPOCH - Duration::new(1, 500_000_000),
        ];
        for time in times {
            assert_eq!(
                Stamp::from_system_time(time).unwrap().into_chrono(),
                DateTime::<Utc>::from(time),
                "{time:?}"
            );
        }
        let one_before = Stamp::from_system_time(UNIX_EPOCH - Duration::new(0, 100)).unwrap();
        assert_eq!(
            (one_before.unix_seconds(), one_before.subsec_nanos()),
            (-1, 999_999_900)
        );

        for micros in [0, 1, -1, 1_767_608_239_120_001, -1_500_001] {
            let ours = Stamp::from_unix_micros(micros).unwrap();
            let theirs = DateTime::from_timestamp_micros(micros).unwrap();
            assert_eq!(ours.into_chrono(), theirs, "{micros}");
            assert_eq!(ours.unix_micros(), theirs.timestamp_micros(), "{micros}");
            assert_eq!(ours.unix_micros(), micros);
        }
        let before = Stamp::from_unix_micros(-1).unwrap();
        assert_eq!(
            (before.unix_seconds(), before.subsec_nanos()),
            (-1, 999_999_000)
        );
        assert_eq!(Stamp::from_unix_micros(i64::MAX), None);
    }

    #[test]
    fn display_utc_matches_chrono_display() {
        for nanos in NANOS {
            let reference = chrono_utc(BASE_SECS, nanos);
            let stamp = Stamp::from_chrono(reference);
            assert_eq!(stamp.display_utc(), reference.to_string(), "{nanos}");
        }
        assert_eq!(
            Stamp::from_unix(BASE_SECS, 0).unwrap().display_utc(),
            "2026-01-05 10:17:19 UTC"
        );
        assert_eq!(
            Stamp::from_unix(BASE_SECS, 120_000_000)
                .unwrap()
                .display_utc(),
            "2026-01-05 10:17:19.120 UTC"
        );
    }

    #[test]
    fn instants_before_1970_floor_their_seconds() {
        let last = Stamp::from_unix(-1, 999_999_999).unwrap();
        assert_eq!(last.unix_seconds(), -1);
        assert_eq!(last.unix_millis(), -1);
        assert_eq!(last.unix_micros(), -1);
        assert_eq!(last.subsec_nanos(), 999_999_999);
        assert_eq!(last.rfc3339(), "1969-12-31T23:59:59.999999999+00:00");
        let millis_before = Stamp::from_unix_millis(-1).unwrap();
        assert_eq!(millis_before, Stamp::from_unix(-1, 999_000_000).unwrap());
        assert_eq!(millis_before.rfc3339(), "1969-12-31T23:59:59.999+00:00");
        let after = Stamp::from_unix(0, 1).unwrap();
        assert_eq!(after.since(last).into_chrono(), TimeDelta::nanoseconds(2));
        assert_eq!(last.since(after).num_millis(), 0);
        assert_eq!(
            last.since(Stamp::from_unix(-2, 0).unwrap()).num_millis(),
            1_999
        );
        assert_eq!(
            Stamp::from_unix(-86_401, 500_000_000).unwrap().rfc3339(),
            "1969-12-30T23:59:59.500+00:00"
        );
    }

    #[test]
    fn debug_forms_match_chrono() {
        let instants = [
            chrono_utc(BASE_SECS, 0),
            chrono_utc(BASE_SECS, 120_000_000),
            chrono_utc(BASE_SECS, 123_456_789),
            chrono_utc(-1, 999_999_999),
            NaiveDate::from_ymd_opt(2026, 12, 31)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, 1_250_000_000)
                .unwrap()
                .and_utc(),
            NaiveDate::from_ymd_opt(10000, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
            NaiveDate::from_ymd_opt(-1, 6, 30)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc(),
        ];
        for instant in instants {
            assert_eq!(
                format!("{:?}", Stamp::from_chrono(instant)),
                format!("{instant:?}")
            );
            assert_eq!(
                format!("{:?}", Day::from_chrono(instant.date_naive())),
                format!("{:?}", instant.date_naive())
            );
            for offset in [0, 7_200, -16_200, 5_405, -29, 86_399] {
                let written = instant.with_timezone(&FixedOffset::east_opt(offset).unwrap());
                assert_eq!(
                    format!("{:?}", OffsetStamp::from_chrono(written)),
                    format!("{written:?}"),
                    "{offset}"
                );
            }
        }
        let at = Stamp::from_unix(BASE_SECS, 120_000_000).unwrap();
        assert_eq!(format!("{at:?}"), "2026-01-05T10:17:19.120Z");
        assert_eq!(
            format!("{:?}", parse_rfc3339("2026-01-05T12:17:19+02:00").unwrap()),
            "2026-01-05T12:17:19+02:00"
        );
        assert_eq!(
            format!("{:?}", at.to_offset_utc()),
            "2026-01-05T10:17:19.120+00:00"
        );
        assert_eq!(format!("{:?}", at.day_utc()), "2026-01-05");
        assert_eq!(
            format!("{:?}", Some(at.day_utc())),
            format!("{:?}", Some(at.into_chrono().date_naive()))
        );
    }

    #[test]
    fn compact_formats_hold_at_the_edges() {
        let at = |y, m, d, h, mi, s, nanos| {
            Stamp::from_chrono(
                NaiveDate::from_ymd_opt(y, m, d)
                    .unwrap()
                    .and_hms_nano_opt(h, mi, s, nanos)
                    .unwrap()
                    .and_utc(),
            )
        };
        let rows = [
            (
                at(0, 1, 1, 0, 0, 0, 0),
                "0000-01-01",
                "0000-01-01 00:00 UTC",
                "00000101T000000.000Z",
                "00000101T000000Z",
                "000101000000",
            ),
            (
                at(9999, 12, 31, 23, 59, 59, 999_999_999),
                "9999-12-31",
                "9999-12-31 23:59 UTC",
                "99991231T235959.999Z",
                "99991231T235959Z",
                "991231235959",
            ),
            (
                at(1969, 7, 20, 20, 17, 40, 500_000_000),
                "1969-07-20",
                "1969-07-20 20:17 UTC",
                "19690720T201740.500Z",
                "19690720T201740Z",
                "690720201740",
            ),
            (
                at(2026, 1, 5, 10, 17, 19, 123_999_999),
                "2026-01-05",
                "2026-01-05 10:17 UTC",
                "20260105T101719.123Z",
                "20260105T101719Z",
                "260105101719",
            ),
        ];
        for (stamp, ymd, hm, millis_z, z, short) in rows {
            let reference = stamp.into_chrono();
            assert_eq!(stamp.day_utc().ymd(), ymd);
            assert_eq!(stamp.ymd_hm_utc(), hm);
            assert_eq!(stamp.compact_millis_z(), millis_z);
            assert_eq!(stamp.compact_z(), z);
            assert_eq!(stamp.yymmddhhmmss(), short);
            assert_eq!(ymd, reference.format("%Y-%m-%d").to_string());
            assert_eq!(hm, reference.format("%Y-%m-%d %H:%M UTC").to_string());
            assert_eq!(millis_z, reference.format("%Y%m%dT%H%M%S%.3fZ").to_string());
            assert_eq!(z, reference.format("%Y%m%dT%H%M%SZ").to_string());
            assert_eq!(short, reference.format("%y%m%d%H%M%S").to_string());
        }
    }

    #[test]
    fn http_date_matches_chrono() {
        assert_eq!(
            Stamp::from_unix(BASE_SECS, 0).unwrap().http_date(),
            "Mon, 05 Jan 2026 10:17:19 GMT"
        );
        assert_eq!(
            Stamp::from_ymd_hms(2026, 12, 31, 23, 59, 59)
                .unwrap()
                .http_date(),
            "Thu, 31 Dec 2026 23:59:59 GMT"
        );
        assert_eq!(
            Stamp::UNIX_EPOCH.http_date(),
            "Thu, 01 Jan 1970 00:00:00 GMT"
        );
        let mut instants: Vec<DateTime<Utc>> = (0..400)
            .map(|i| {
                chrono_utc(
                    -2_208_988_800 + i * 31_622_401 + i * i * 7_919,
                    (i as u32) * 2_500_001,
                )
            })
            .collect();
        instants.extend([
            chrono_utc(-1, 999_999_999),
            NaiveDate::from_ymd_opt(2026, 12, 31)
                .unwrap()
                .and_hms_nano_opt(23, 59, 59, 1_000_000_000)
                .unwrap()
                .and_utc(),
            NaiveDate::from_ymd_opt(0, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
            NaiveDate::from_ymd_opt(9999, 12, 31)
                .unwrap()
                .and_hms_opt(23, 59, 59)
                .unwrap()
                .and_utc(),
            NaiveDate::from_ymd_opt(10000, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
            NaiveDate::from_ymd_opt(-1, 6, 30)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc(),
        ]);
        for instant in instants {
            assert_eq!(
                Stamp::from_chrono(instant).http_date(),
                instant.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
                "{instant:?}"
            );
        }
    }

    #[test]
    fn saturating_sub_days_never_panics() {
        let day = Day::from_ymd(2026, 1, 5).unwrap();
        let far = day.saturating_sub_days(20000 * 366);
        assert!(far < day);
        assert!(day.saturating_sub_days(i64::MAX) <= far);
        assert!(day.saturating_sub_days(i64::MIN) > day);
        assert!(day.saturating_sub_days(-(20000 * 366)) > day);
    }
}
