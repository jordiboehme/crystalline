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
//! The values are backed by jiff. Two things differ from the chrono-backed
//! module of 0.33, both on purpose: a leap second `:60` is read as `:59`,
//! and a parsed year outside 0000 to 9999 (or an instant outside jiff's
//! range) is refused.

mod calendar;
#[cfg(test)]
mod corpus;
mod emit;
mod parse;
pub mod serde;

use jiff::civil::{Date, Time};
use jiff::tz::Offset;
use jiff::{SignedDuration, Span, Timestamp, Unit};
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
pub struct Stamp(Timestamp);

/// An instant together with the UTC offset it was written with.
///
/// Equality, ordering and hashing look at the instant only, so
/// `10:00+02:00` and `08:00+00:00` are the same value; the offset matters
/// only when the value is written out again.
///
/// `Debug` prints what chrono's `Debug` printed: the local date and time
/// and the exact offset, `2026-01-05T12:17:19+02:00`.
#[derive(Clone, Copy)]
pub struct OffsetStamp {
    at: Timestamp,
    offset: Offset,
}

/// A calendar date with no time of day and no offset.
///
/// `Debug` prints what chrono's `Debug` printed: `2026-01-05`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day(Date);

/// An exact span of time: whole seconds plus nanoseconds, where a day is
/// always 86400 seconds.
///
/// The range is chrono's: at most `i64::MAX` milliseconds either way. A
/// span beyond it makes the helpers and operators panic, and
/// [`try_seconds`] and [`Delta::from_std`] give `None`, as chrono did.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Delta(SignedDuration);

/// The largest span, `i64::MAX` milliseconds, as whole seconds and the
/// nanoseconds past them (chrono's `TimeDelta::MAX`).
const DELTA_MAX_SECS: i64 = i64::MAX / 1000;
const DELTA_MAX_NANOS: i32 = (i64::MAX % 1000) as i32 * 1_000_000;

impl Delta {
    /// The empty span.
    pub const ZERO: Delta = Delta(SignedDuration::ZERO);

    /// The longest span, `i64::MAX` milliseconds (chrono's `TimeDelta::MAX`).
    pub const MAX: Delta = Delta(SignedDuration::new(DELTA_MAX_SECS, DELTA_MAX_NANOS));

    /// The longest negative span, `-Delta::MAX`.
    pub const MIN: Delta = Delta(SignedDuration::new(-DELTA_MAX_SECS, -DELTA_MAX_NANOS));

    /// `value` when it is within chrono's range, else `None`. jiff keeps
    /// the seconds and the nanoseconds with the same sign.
    const fn bounded(value: SignedDuration) -> Option<Delta> {
        let (secs, nanos) = (value.as_secs(), value.subsec_nanos());
        let within = if secs >= 0 && nanos >= 0 {
            secs < DELTA_MAX_SECS || (secs == DELTA_MAX_SECS && nanos <= DELTA_MAX_NANOS)
        } else {
            secs > -DELTA_MAX_SECS || (secs == -DELTA_MAX_SECS && nanos >= -DELTA_MAX_NANOS)
        };
        if within { Some(Delta(value)) } else { None }
    }

    /// `units` of `unit_secs` seconds each, panicking with `what` when the
    /// span is out of range.
    const fn whole(units: i64, unit_secs: i64, what: &'static str) -> Delta {
        let Some(secs) = units.checked_mul(unit_secs) else {
            panic!("{}", what);
        };
        match Delta::bounded(SignedDuration::from_secs(secs)) {
            Some(delta) => delta,
            None => panic!("{}", what),
        }
    }
}

/// The current instant from the system clock.
pub fn now() -> Stamp {
    Stamp(Timestamp::now())
}

/// Today's date in UTC.
pub fn today_utc() -> Day {
    now().day_utc()
}

/// `n` days of 86400 seconds each. Panics when the span is out of range,
/// as chrono's `Duration::days` does.
pub const fn days(n: i64) -> Delta {
    Delta::whole(n, 86_400, "`time::days` out of range")
}

/// `n` hours. Panics when the span is out of range.
pub const fn hours(n: i64) -> Delta {
    Delta::whole(n, 3_600, "`time::hours` out of range")
}

/// `n` minutes. Panics when the span is out of range.
pub const fn minutes(n: i64) -> Delta {
    Delta::whole(n, 60, "`time::minutes` out of range")
}

/// `n` seconds. Panics when the span is out of range.
pub const fn seconds(n: i64) -> Delta {
    Delta::whole(n, 1, "`time::seconds` out of range")
}

/// `n` milliseconds. Panics only for `i64::MIN`, as chrono's
/// `Duration::milliseconds` does.
pub const fn millis(n: i64) -> Delta {
    match Delta::bounded(SignedDuration::from_millis(n)) {
        Some(delta) => delta,
        None => panic!("`time::millis` out of range"),
    }
}

/// `n` seconds, or `None` when the span is out of range. For a number from
/// outside input, where [`seconds`] would panic.
pub fn try_seconds(n: i64) -> Option<Delta> {
    Delta::bounded(SignedDuration::from_secs(n))
}

/// `n` seconds, held at the longest span either way when `n` is out of
/// range. For a number from outside input that only moves an instant: with
/// [`Stamp::saturating_add`] it never panics.
pub fn saturating_seconds(n: i64) -> Delta {
    try_seconds(n).unwrap_or(if n < 0 { Delta::MIN } else { Delta::MAX })
}

/// The span a `timeframe` spells, in seconds: a whole number, then a unit
/// (`h` hours, `d` or none days, `w` weeks, `m` months of 30 days, `y`
/// years of 365 days), with spaces around either part allowed: `24h`, `7d`,
/// `7`, `2w`, `3m`, `1y`. `None` when the number or the unit does not read.
/// A span too long for the seconds is held at the longest one. One grammar
/// for every window a person types: `build_context`'s and
/// `recent_activity`'s `timeframe` and a Reflection page's `freshness`.
pub fn timeframe_seconds(spec: &str) -> Option<i64> {
    let spec = spec.trim();
    let (num, unit) = spec.split_at(spec.find(|c: char| c.is_alphabetic()).unwrap_or(spec.len()));
    let n: i64 = num.trim().parse().ok()?;
    let per_unit: i64 = match unit.trim() {
        "h" => 3600,
        "d" | "" => 86_400,
        "w" => 7 * 86_400,
        "m" => 30 * 86_400,
        "y" => 365 * 86_400,
        _ => return None,
    };
    Some(n.saturating_mul(per_unit))
}

impl Delta {
    /// Whole days in the span, truncated toward zero.
    pub fn num_days(&self) -> i64 {
        self.0.as_secs() / 86_400
    }

    /// Whole hours in the span, truncated toward zero.
    pub fn num_hours(&self) -> i64 {
        self.0.as_hours()
    }

    /// Whole minutes in the span, truncated toward zero.
    pub fn num_minutes(&self) -> i64 {
        self.0.as_mins()
    }

    /// Whole seconds in the span, truncated toward zero.
    pub fn num_seconds(&self) -> i64 {
        self.0.as_secs()
    }

    /// Whole milliseconds in the span, truncated toward zero.
    pub fn num_millis(&self) -> i64 {
        // In range by construction: at most `i64::MAX` milliseconds.
        self.0.as_millis() as i64
    }

    /// The span as a `std::time::Duration`, or `None` when it is negative.
    pub fn to_std(&self) -> Option<Duration> {
        Duration::try_from(self.0).ok()
    }

    /// A `std::time::Duration` as a span, nanoseconds kept, or `None` when
    /// it is out of range, as chrono's `TimeDelta::from_std` refused it.
    pub fn from_std(value: Duration) -> Option<Delta> {
        SignedDuration::try_from(value)
            .ok()
            .and_then(Delta::bounded)
    }
}

/// chrono's derived `Debug` of a `TimeDelta`: whole seconds rounded toward
/// negative infinity and the nanoseconds past them,
/// `TimeDelta { secs: -2, nanos: 500000000 }`.
impl fmt::Debug for Delta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.0.as_nanos();
        f.debug_struct("TimeDelta")
            .field("secs", &(total.div_euclid(1_000_000_000) as i64))
            .field("nanos", &(total.rem_euclid(1_000_000_000) as i32))
            .finish()
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
        self.0
            .checked_add(rhs.0)
            .and_then(Delta::bounded)
            .expect("`Delta + Delta` overflowed")
    }
}

impl Sub for Delta {
    type Output = Delta;

    fn sub(self, rhs: Delta) -> Delta {
        self.0
            .checked_sub(rhs.0)
            .and_then(Delta::bounded)
            .expect("`Delta - Delta` overflowed")
    }
}

impl Mul<i32> for Delta {
    type Output = Delta;

    fn mul(self, rhs: i32) -> Delta {
        self.0
            .checked_mul(rhs)
            .and_then(Delta::bounded)
            .expect("`Delta * i32` overflowed")
    }
}

impl Stamp {
    /// 1970-01-01T00:00:00Z.
    pub const UNIX_EPOCH: Stamp = Stamp(Timestamp::UNIX_EPOCH);

    /// The latest whole second an instant holds: 9999-12-30T22:00:00Z.
    pub const LATEST: Stamp = Stamp(Timestamp::constant(253_402_207_200, 0));

    /// The earliest whole second an instant holds: -009999-01-02T01:59:59Z.
    pub const EARLIEST: Stamp = Stamp(Timestamp::MIN);

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
        let date = Day::from_ymd(year, month, day)?.0;
        let time = Time::new(
            i8::try_from(hour).ok()?,
            i8::try_from(minute).ok()?,
            i8::try_from(second).ok()?,
            0,
        )
        .ok()?;
        Offset::UTC
            .to_timestamp(date.to_datetime(time))
            .ok()
            .map(Stamp)
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
        Timestamp::from_microsecond(micros).ok().map(Stamp)
    }

    /// The instant `secs` seconds and `nanos` nanoseconds after the Unix
    /// epoch, or `None` when it is out of range or `nanos` is not below one
    /// second.
    pub fn from_unix(secs: i64, nanos: u32) -> Option<Stamp> {
        if nanos >= 1_000_000_000 {
            return None;
        }
        // Below one second, so it fits; jiff moves a positive fraction of a
        // negative second onto the second itself.
        Timestamp::new(secs, nanos as i32).ok().map(Stamp)
    }

    /// The instant `ms` milliseconds after the Unix epoch, or `None` when it
    /// is out of range.
    pub fn from_unix_millis(ms: i64) -> Option<Stamp> {
        Timestamp::from_millisecond(ms).ok().map(Stamp)
    }

    /// Nanoseconds since the Unix epoch. jiff truncates its seconds toward
    /// zero and gives a negative fraction before 1970; every accessor below
    /// floors from this instead, as chrono did.
    fn nanos_since_epoch(&self) -> i128 {
        self.0.as_nanosecond()
    }

    /// Whole seconds since the Unix epoch, rounded toward negative infinity.
    pub fn unix_seconds(&self) -> i64 {
        self.nanos_since_epoch().div_euclid(1_000_000_000) as i64
    }

    /// Whole milliseconds since the Unix epoch, rounded toward negative
    /// infinity.
    pub fn unix_millis(&self) -> i64 {
        self.nanos_since_epoch().div_euclid(1_000_000) as i64
    }

    /// Whole microseconds since the Unix epoch, rounded toward negative
    /// infinity.
    pub fn unix_micros(&self) -> i64 {
        self.nanos_since_epoch().div_euclid(1_000) as i64
    }

    /// The nanoseconds past the whole second.
    pub fn subsec_nanos(&self) -> u32 {
        self.nanos_since_epoch().rem_euclid(1_000_000_000) as u32
    }

    /// This instant moved forward by `delta`, or `None` when out of range.
    pub fn checked_add(self, delta: Delta) -> Option<Stamp> {
        self.0.checked_add(delta.0).ok().map(Stamp)
    }

    /// This instant moved back by `delta`, or `None` when out of range.
    pub fn checked_sub(self, delta: Delta) -> Option<Stamp> {
        self.0.checked_sub(delta.0).ok().map(Stamp)
    }

    /// This instant moved forward by `delta`, held at [`Stamp::LATEST`] or
    /// [`Stamp::EARLIEST`] when that is out of range. For a span from
    /// outside input (a token lifetime, a `Retry-After`, a setting), where
    /// `+` would panic.
    pub fn saturating_add(self, delta: Delta) -> Stamp {
        self.checked_add(delta).unwrap_or(if delta.0.is_negative() {
            Stamp::EARLIEST
        } else {
            Stamp::LATEST
        })
    }

    /// This instant moved back by `delta`, held at [`Stamp::EARLIEST`] or
    /// [`Stamp::LATEST`] when that is out of range.
    pub fn saturating_sub(self, delta: Delta) -> Stamp {
        self.checked_sub(delta).unwrap_or(if delta.0.is_negative() {
            Stamp::LATEST
        } else {
            Stamp::EARLIEST
        })
    }

    /// The span from `earlier` to this instant (negative when `earlier` is
    /// in fact later).
    pub fn since(self, earlier: Stamp) -> Delta {
        // Any two instants jiff holds are well within chrono's span range.
        Delta(self.0.duration_since(earlier.0))
    }

    /// The UTC calendar date of this instant.
    pub fn day_utc(self) -> Day {
        Day(Offset::UTC.to_datetime(self.0).date())
    }

    /// This instant with offset zero.
    pub fn to_offset_utc(self) -> OffsetStamp {
        OffsetStamp {
            at: self.0,
            offset: Offset::UTC,
        }
    }

    /// RFC 3339 with `+00:00` and the fraction in groups of none, 3, 6 or 9
    /// digits, as chrono's `to_rfc3339` writes it.
    pub fn rfc3339(&self) -> String {
        emit::rfc3339(self.unix_seconds(), self.subsec_nanos(), 0)
    }

    /// RFC 3339 in whole seconds with `Z`: `2026-01-05T10:17:19Z`.
    pub fn rfc3339_secs_z(&self) -> String {
        let mut text = self.rfc3339_secs();
        text.truncate(text.len() - "+00:00".len());
        text.push('Z');
        text
    }

    /// RFC 3339 in whole seconds with `+00:00`: `2026-01-05T10:17:19+00:00`.
    pub fn rfc3339_secs(&self) -> String {
        emit::rfc3339(self.unix_seconds(), 0, 0)
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

    // The compact forms below print the year by hand and the rest with
    // jiff's `strftime`: jiff writes a year before 0000 as `-001` and its
    // `%y` as `-1`, where chrono wrote `-0001` and `99`.

    /// The UTC year of this instant.
    fn utc_year(&self) -> i64 {
        i64::from(Offset::UTC.to_datetime(self.0).year())
    }

    /// `2026-01-05 10:17 UTC`.
    pub fn ymd_hm_utc(&self) -> String {
        let rest = self.0.strftime("-%m-%d %H:%M UTC");
        format!("{}{rest}", emit::year(self.utc_year()))
    }

    /// `20260105T101719.120Z`, always three fraction digits.
    pub fn compact_millis_z(&self) -> String {
        let rest = self.0.strftime("%m%dT%H%M%S%.3fZ");
        format!("{}{rest}", emit::year(self.utc_year()))
    }

    /// `20260105T101719Z`.
    pub fn compact_z(&self) -> String {
        let rest = self.0.strftime("%m%dT%H%M%SZ");
        format!("{}{rest}", emit::year(self.utc_year()))
    }

    /// `260105101719`: two-digit year, then month, day, hour, minute and
    /// second.
    pub fn yymmddhhmmss(&self) -> String {
        let rest = self.0.strftime("%m%d%H%M%S");
        format!("{:02}{rest}", self.utc_year().rem_euclid(100))
    }
}

impl OffsetStamp {
    /// The instant at `offset_seconds` east of UTC, or `None` when the
    /// offset is out of range. Only the tests build one this way; parsed
    /// values come from the grammars.
    #[cfg(test)]
    fn at_offset(stamp: Stamp, offset_seconds: i32) -> Option<OffsetStamp> {
        Some(OffsetStamp {
            at: stamp.0,
            offset: Offset::from_seconds(offset_seconds).ok()?,
        })
    }

    /// The instant, without the offset.
    pub fn stamp(&self) -> Stamp {
        Stamp(self.at)
    }

    /// The date in the written offset: `2026-01-05T23:30:00-05:00` is the 5th,
    /// though it is the 6th in UTC.
    pub fn day(&self) -> Day {
        Day(self.offset.to_datetime(self.at).date())
    }

    /// The written offset in seconds east of UTC.
    pub fn offset_seconds(&self) -> i32 {
        self.offset.seconds()
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
        self.checked_add_days(rhs.num_days())
            .expect("`Day + Delta` overflowed")
    }
}

/// Moves the date back by the whole days in the span (the rest is
/// dropped). Panics when out of range, as chrono's `NaiveDate - TimeDelta`
/// does.
impl Sub<Delta> for Day {
    type Output = Day;

    fn sub(self, rhs: Delta) -> Day {
        self.checked_add_days(-rhs.num_days())
            .expect("`Day - Delta` overflowed")
    }
}

/// Moves the instant and keeps the written offset. Panics when out of
/// range, as chrono's `DateTime<FixedOffset> + TimeDelta` does.
impl Add<Delta> for OffsetStamp {
    type Output = OffsetStamp;

    fn add(self, rhs: Delta) -> OffsetStamp {
        OffsetStamp {
            at: self
                .stamp()
                .checked_add(rhs)
                .expect("`OffsetStamp + Delta` overflowed")
                .0,
            offset: self.offset,
        }
    }
}

/// Moves the instant back and keeps the written offset. Panics when out of
/// range, as chrono's `DateTime<FixedOffset> - TimeDelta` does.
impl Sub<Delta> for OffsetStamp {
    type Output = OffsetStamp;

    fn sub(self, rhs: Delta) -> OffsetStamp {
        OffsetStamp {
            at: self
                .stamp()
                .checked_sub(rhs)
                .expect("`OffsetStamp - Delta` overflowed")
                .0,
            offset: self.offset,
        }
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
    /// `None` when there is no such date or the year is outside -9999 to
    /// 9999.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Option<Day> {
        Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()
        .map(Day)
    }

    /// The year.
    pub fn year(&self) -> i32 {
        i32::from(self.0.year())
    }

    /// The month, 1 to 12.
    pub fn month(&self) -> u32 {
        self.0.month() as u32
    }

    /// The day of the month, starting at 1.
    pub fn day(&self) -> u32 {
        self.0.day() as u32
    }

    /// The date `n` calendar days later (earlier for a negative `n`), or
    /// `None` when out of range.
    pub fn checked_add_days(self, n: i64) -> Option<Day> {
        let span = Span::new().try_days(n).ok()?;
        self.0.checked_add(span).ok().map(Day)
    }

    /// The date `n` calendar days earlier, held at the earliest date when
    /// that is out of range. A negative `n` moves forward and is held at the
    /// latest date. Never panics.
    pub fn saturating_sub_days(self, n: i64) -> Day {
        n.checked_neg()
            .and_then(|back| self.checked_add_days(back))
            .unwrap_or(if n >= 0 {
                Day(Date::MIN)
            } else {
                Day(Date::MAX)
            })
    }

    /// Whole days from this date to `later`: `a.days_until(b)` is `b - a`,
    /// negative when `later` is in fact earlier.
    pub fn days_until(self, later: Day) -> i64 {
        let span = self
            .0
            .until((Unit::Day, later.0))
            .expect("two dates are a whole number of days apart");
        i64::from(span.get_days())
    }

    /// Midnight UTC at the start of this date, or `None` when out of range.
    pub fn start_utc(self) -> Option<Stamp> {
        Offset::UTC
            .to_timestamp(self.0.to_datetime(Time::midnight()))
            .ok()
            .map(Stamp)
    }

    /// `2026-01-05`, and `-0001-01-01` before the year 0000.
    pub fn ymd(&self) -> String {
        emit::date(i64::from(self.year()), self.month() as u8, self.day() as u8)
    }
}

/// A strict RFC 3339 instant, as chrono's `DateTime::parse_from_rfc3339`
/// reads it: `2026-01-05T10:17:19Z`, with `t` or a space allowed for `T`,
/// an optional fraction of any length (nine digits kept), and `Z`, `z` or
/// `+hh:mm`/`-hh:mm` (the minus also as U+2212). A leap second `:60` is read
/// as `:59`.
pub fn parse_rfc3339(s: &str) -> Option<OffsetStamp> {
    parse::rfc3339(s).and_then(offset_stamp_from_parts)
}

/// Whether `s` is an instant by the grammar of [`parse_rfc3339`] that lies
/// past the latest one an instant holds, so `parse_rfc3339` refuses it:
/// `9999-12-31T23:59:59Z`. 0.33.0 read such an instant, so a value stored
/// as "far in the future" then can tell itself apart from one nobody can
/// read.
pub fn rfc3339_past_latest(s: &str) -> bool {
    let Some(parts) = parse::rfc3339(s) else {
        return false;
    };
    date_from_parts(&parts).is_some() && offset_stamp_from_parts(parts).is_none()
}

/// An instant as chrono's `FromStr` and `Deserialize` for `DateTime<Utc>`
/// read it: the grammar of [`parse_lenient_offset`], moved to UTC.
pub fn parse_lenient_stamp(s: &str) -> Option<Stamp> {
    parse_lenient_offset(s).map(|value| value.stamp())
}

/// An instant with its offset as chrono's `FromStr` and `Deserialize` for
/// `DateTime<FixedOffset>` read it: RFC 3339 plus white space around the
/// value and its parts, unpadded fields (`2026-1-5T1:7:9Z`), a signed year
/// (`+2026`), `UTC` as an offset, and an offset written `+0200` or
/// `+02 00`. Only years 0000 to 9999 are read.
pub fn parse_lenient_offset(s: &str) -> Option<OffsetStamp> {
    parse::lenient(s).and_then(offset_stamp_from_parts)
}

/// A date as chrono's `NaiveDate::parse_from_str(s, "%Y-%m-%d")` reads it:
/// `2026-01-05`, also `2026-1-5` and a leading space before each number.
/// Only years 0000 to 9999 are read.
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
// value here, and only here. Two rulings of the jiff move live here (J4):
// a leap second is read as second 59 with its fraction kept, because a jiff
// `Timestamp` cannot hold one, and a year outside 0000 to 9999 is refused,
// as is an instant outside jiff's range.

/// The date in `parts`, when its year is within 0000 to 9999.
fn date_from_parts(parts: &Parts) -> Option<Date> {
    if !(0..=9999).contains(&parts.year) {
        return None;
    }
    Date::new(
        i16::try_from(parts.year).ok()?,
        i8::try_from(parts.month).ok()?,
        i8::try_from(parts.day).ok()?,
    )
    .ok()
}

fn offset_stamp_from_parts(parts: Parts) -> Option<OffsetStamp> {
    let date = date_from_parts(&parts)?;
    // A leap second already arrives as second 59 with `leap` set; dropping
    // the flag is the clamp.
    let time = Time::new(
        i8::try_from(parts.hour).ok()?,
        i8::try_from(parts.minute).ok()?,
        i8::try_from(parts.second).ok()?,
        i32::try_from(parts.nanos).ok()?,
    )
    .ok()?;
    let offset = Offset::from_seconds(parts.offset_seconds).ok()?;
    let at = offset.to_timestamp(date.to_datetime(time)).ok()?;
    Some(OffsetStamp { at, offset })
}

fn day_from_parts(parts: Parts) -> Option<Day> {
    date_from_parts(&parts).map(Day)
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_timeframe_spells_a_span_in_seconds() {
        for (spec, seconds) in [
            ("24h", 24 * 3600),
            ("7d", 7 * 86_400),
            ("7", 7 * 86_400),
            (" 7 d ", 7 * 86_400),
            ("2w", 14 * 86_400),
            ("3m", 90 * 86_400),
            ("1y", 365 * 86_400),
            ("0h", 0),
            ("-2d", -2 * 86_400),
            ("9223372036854775807h", i64::MAX),
        ] {
            assert_eq!(timeframe_seconds(spec), Some(seconds), "{spec}");
        }
        for spec in [
            "",
            "h",
            "1.5d",
            "1H",
            "1mo",
            "soon",
            "99999999999999999999d",
        ] {
            assert_eq!(timeframe_seconds(spec), None, "{spec:?}");
        }
    }
    use super::corpus::{CORPUS, Expect};
    use super::serde::{offset_stamp_json, stamp_json};
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    /// 2026-01-05T10:17:19Z.
    const BASE_SECS: i64 = 1_767_608_239;

    const NANOS: [u32; 5] = [0, 500_000_000, 120_000_000, 123_450_000, 123_456_789];

    /// The fraction chrono's `to_rfc3339` wrote for each of [`NANOS`].
    const FRACTIONS: [&str; 5] = ["", ".500", ".120", ".123450", ".123456789"];

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn base(nanos: u32) -> Stamp {
        Stamp::from_unix(BASE_SECS, nanos).unwrap()
    }

    fn nanos(n: u64) -> Delta {
        Delta::from_std(Duration::from_nanos(n)).unwrap()
    }

    #[test]
    fn base_instant_is_the_documented_one() {
        let stamp = Stamp::from_unix(BASE_SECS, 0).unwrap();
        assert_eq!(stamp.rfc3339_secs_z(), "2026-01-05T10:17:19Z");
    }

    #[test]
    fn rfc3339_writes_what_chrono_wrote() {
        for (nanos, fraction) in NANOS.into_iter().zip(FRACTIONS) {
            let stamp = base(nanos);
            let utc = format!("2026-01-05T10:17:19{fraction}+00:00");
            assert_eq!(stamp.rfc3339(), utc);
            assert_eq!(stamp.to_offset_utc().rfc3339(), utc);
            for (offset, expected) in [
                (0, utc.clone()),
                (7_200, format!("2026-01-05T12:17:19{fraction}+02:00")),
                (-16_200, format!("2026-01-05T05:47:19{fraction}-04:30")),
            ] {
                let value = OffsetStamp::at_offset(stamp, offset).unwrap();
                assert_eq!(value.rfc3339(), expected, "{nanos} {offset}");
                assert_eq!(value.offset_seconds(), offset);
                assert_eq!(value.stamp(), stamp);
            }
        }
    }

    #[test]
    fn secs_forms_drop_the_fraction() {
        for nanos in NANOS {
            let stamp = base(nanos);
            assert_eq!(stamp.rfc3339_secs_z(), "2026-01-05T10:17:19Z");
            assert_eq!(stamp.rfc3339_secs(), "2026-01-05T10:17:19+00:00");
        }
        let before = Stamp::from_unix(-1, 999_999_999).unwrap();
        assert_eq!(before.rfc3339_secs_z(), "1969-12-31T23:59:59Z");
        assert_eq!(before.rfc3339_secs(), "1969-12-31T23:59:59+00:00");
    }

    #[test]
    fn compact_formats_write_what_chrono_wrote() {
        let millis = [".000", ".500", ".120", ".123", ".123"];
        for (nanos, millis) in NANOS.into_iter().zip(millis) {
            let stamp = base(nanos);
            assert_eq!(stamp.day_utc().ymd(), "2026-01-05");
            assert_eq!(stamp.ymd_hm_utc(), "2026-01-05 10:17 UTC");
            assert_eq!(
                stamp.compact_millis_z(),
                format!("20260105T101719{millis}Z")
            );
            assert_eq!(stamp.compact_z(), "20260105T101719Z");
            assert_eq!(stamp.yymmddhhmmss(), "260105101719");
        }
    }

    #[test]
    fn offset_stamps_compare_by_instant() {
        let at_ten = parse_rfc3339("2026-01-05T10:00:00+02:00").unwrap();
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
    fn delta_counts_whole_units_like_chrono() {
        // (span, days, hours, minutes, seconds, milliseconds), chrono's
        // answers.
        let cases = [
            (days(3), 3, 72, 4_320, 259_200, 259_200_000),
            (days(-3), -3, -72, -4_320, -259_200, -259_200_000),
            (hours(49), 2, 49, 2_940, 176_400, 176_400_000),
            (hours(-49), -2, -49, -2_940, -176_400, -176_400_000),
            (minutes(-90), 0, -1, -90, -5_400, -5_400_000),
            (seconds(3599), 0, 0, 59, 3_599, 3_599_000),
            (seconds(-3599), 0, 0, -59, -3_599, -3_599_000),
            (millis(1_999), 0, 0, 0, 1, 1_999),
            (millis(-1_999), 0, 0, 0, -1, -1_999),
            (Delta::ZERO, 0, 0, 0, 0, 0),
        ];
        for (delta, d, h, m, s, ms) in cases {
            assert_eq!(
                (
                    delta.num_days(),
                    delta.num_hours(),
                    delta.num_minutes(),
                    delta.num_seconds(),
                    delta.num_millis()
                ),
                (d, h, m, s, ms),
                "{delta:?}"
            );
            assert_eq!((-delta).num_millis(), -ms);
            assert_eq!((delta * 3).num_millis(), 3 * ms);
        }
        assert_eq!((days(1) + hours(1)).num_hours(), 25);
        assert_eq!((days(1) - hours(1)).num_hours(), 23);
        assert!(minutes(59) < hours(1));
        assert!(-seconds(1) < Delta::ZERO);
        assert_eq!(nanos(1_999_999).num_millis(), 1);
        assert_eq!((-nanos(1_999_999)).num_millis(), -1);

        let start = base(120_000_000);
        let later = start.checked_add(days(2) + millis(5)).unwrap();
        assert_eq!(later.rfc3339(), "2026-01-07T10:17:19.125+00:00");
        assert_eq!(later.since(start).num_millis(), 2 * 86_400_000 + 5);
        assert_eq!(start.since(later).num_millis(), -(2 * 86_400_000 + 5));
        assert_eq!(later.checked_sub(days(2) + millis(5)), Some(start));
        assert_eq!(start.unix_seconds(), BASE_SECS);
        assert_eq!(start.unix_millis(), BASE_SECS * 1000 + 120);
        assert_eq!(start.subsec_nanos(), 120_000_000);
        assert_eq!(Stamp::from_unix_millis(BASE_SECS * 1000 + 120), Some(start));
        assert_eq!(Stamp::from_unix(BASE_SECS, 1_000_000_000), None);
        assert_eq!(Stamp::from_unix(i64::MAX, 0), None);
        assert_eq!(start.checked_add(days(400_000_000)), None);
    }

    #[test]
    fn delta_debug_is_chronos() {
        assert_eq!(
            format!("{:?}", millis(-1_999)),
            "TimeDelta { secs: -2, nanos: 1000000 }"
        );
        assert_eq!(
            format!("{:?}", millis(1_999)),
            "TimeDelta { secs: 1, nanos: 999000000 }"
        );
        assert_eq!(
            format!("{:?}", Delta::ZERO),
            "TimeDelta { secs: 0, nanos: 0 }"
        );
    }

    #[test]
    fn delta_range_is_chronos() {
        // chrono's `TimeDelta::MAX`: `i64::MAX` milliseconds.
        let max = Duration::new(9_223_372_036_854_775, 807_000_000);
        let top = Delta::from_std(max).unwrap();
        assert_eq!(top.num_millis(), i64::MAX);
        assert_eq!((-top).num_millis(), -i64::MAX);
        assert_eq!(Delta::from_std(max + Duration::from_nanos(1)), None);
        assert_eq!(Delta::from_std(Duration::MAX), None);
        assert!(try_seconds(9_223_372_036_854_775).is_some());
        assert!(try_seconds(-9_223_372_036_854_775).is_some());
        assert_eq!(try_seconds(9_223_372_036_854_776), None);
        assert_eq!(try_seconds(-9_223_372_036_854_776), None);
        assert_eq!(days(106_751_991_167).num_days(), 106_751_991_167);
        assert_eq!(millis(i64::MAX).num_millis(), i64::MAX);
        fn panics(f: impl FnOnce() -> Delta + std::panic::UnwindSafe) -> bool {
            std::panic::catch_unwind(f).is_err()
        }
        assert!(panics(|| days(106_751_991_168)));
        assert!(panics(|| hours(i64::MAX)));
        assert!(panics(|| millis(i64::MIN)));
        assert!(panics(|| top + nanos(1)));
        assert!(panics(|| -top - nanos(1)));
        assert!(panics(|| seconds(i64::MAX / 2000) * 3));
    }

    #[test]
    fn day_arithmetic_and_accessors() {
        let day = Day::from_ymd(2026, 1, 5).unwrap();
        assert_eq!((day.year(), day.month(), day.day()), (2026, 1, 5));
        assert_eq!(Day::from_ymd(2026, 2, 30), None);
        assert_eq!(day.checked_add_days(31).unwrap().ymd(), "2026-02-05");
        assert_eq!(day.checked_add_days(-5).unwrap().ymd(), "2025-12-31");
        assert_eq!(day.checked_add_days(i64::MAX), None);
        assert_eq!(day.checked_add_days(i64::MIN), None);
        assert_eq!(day.saturating_sub_days(5).ymd(), "2025-12-31");
        assert_eq!(day.saturating_sub_days(-27).ymd(), "2026-02-01");
        let later = Day::from_ymd(2026, 3, 1).unwrap();
        assert_eq!(day.days_until(later), 55);
        assert_eq!(later.days_until(day), -55);
        let (first, last) = (Day::from_ymd(0, 1, 1).unwrap(), Day(Date::MAX));
        assert_eq!(first.days_until(last), 3_652_424);
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

    /// Every row of the frozen corpus through every parser: the bytes our
    /// side writes for what it read, or `None` for a refusal. The serde
    /// adapters read what the lenient parsers read and write the frozen
    /// `stamp_serde` and `offset_stamp_serde` bytes.
    #[test]
    fn corpus_holds() {
        #[derive(::serde::Deserialize)]
        struct Field {
            #[serde(with = "super::serde::offset_stamp")]
            written: OffsetStamp,
        }
        #[derive(::serde::Deserialize)]
        struct UtcField {
            #[serde(with = "super::serde::stamp")]
            at: Stamp,
        }
        #[derive(::serde::Deserialize)]
        struct DayField {
            #[serde(with = "super::serde::day")]
            on: Day,
        }
        assert!(CORPUS.len() > 100, "{}", CORPUS.len());
        for (input, expect) in CORPUS {
            assert_eq!(&ours(input), expect, "{input:?}");
            let lenient = parse_lenient_offset(input);
            assert_eq!(
                parse_lenient_stamp(input),
                lenient.map(|v| v.stamp()),
                "{input:?}"
            );
            let text = serde_json::to_string(input).unwrap();
            let read = serde_json::from_str::<Field>(&format!(r#"{{"written":{text}}}"#))
                .ok()
                .map(|f| f.written);
            assert_eq!(read.map(|v| v.rfc3339()), lenient.map(|v| v.rfc3339()));
            let read = serde_json::from_str::<UtcField>(&format!(r#"{{"at":{text}}}"#))
                .ok()
                .map(|f| f.at);
            assert_eq!(read, lenient.map(|v| v.stamp()), "{input:?}");
            let read = serde_json::from_str::<DayField>(&format!(r#"{{"on":{text}}}"#))
                .ok()
                .map(|f| f.on);
            assert_eq!(read, parse_lenient_date(input), "{input:?}");
        }
    }

    /// What every parser makes of `input`, in the corpus's shape.
    fn ours(input: &str) -> Expect {
        fn leak(text: String) -> &'static str {
            Box::leak(text.into_boxed_str())
        }
        Expect {
            rfc3339: parse_rfc3339(input).map(|v| leak(v.rfc3339())),
            lenient: parse_lenient_offset(input).map(|v| leak(v.rfc3339())),
            date: parse_date(input).map(|v| leak(v.ymd())),
            lenient_date: parse_lenient_date(input).map(|v| leak(v.ymd())),
            kept: parse_kept_stamp(input).map(|v| leak(v.rfc3339())),
            rfc2822: parse_rfc2822(input).map(|v| leak(v.rfc3339())),
            stamp_serde: parse_lenient_stamp(input).map(|v| leak(adapter_text(stamp_json(v)))),
            offset_stamp_serde: parse_lenient_offset(input)
                .map(|v| leak(adapter_text(offset_stamp_json(v)))),
        }
    }

    /// The text inside a `*_json` value: the bytes the adapter writes.
    fn adapter_text(value: serde_json::Value) -> String {
        match value {
            serde_json::Value::String(text) => text,
            other => panic!("an adapter wrote {other}"),
        }
    }

    /// Prints the corpus table again from what the parsers give now, as
    /// literal Rust for `time/corpus.rs`: add a row with any `Expect`, run
    /// `cargo test -p crystalline-core --lib print_corpus -- --ignored
    /// --nocapture`, check the printed rows and paste them over the table.
    #[test]
    #[ignore = "prints the corpus table; run by hand"]
    fn print_corpus() {
        fn literal(text: &str) -> String {
            let mut out = String::from("\"");
            for c in text.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    ' '..='~' => out.push(c),
                    '\t' => out.push_str("\\t"),
                    '\n' => out.push_str("\\n"),
                    _ => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
                }
            }
            out.push('"');
            out
        }
        let field = |value: Option<&str>| {
            value.map_or("None".to_string(), |t| format!("Some({})", literal(t)))
        };
        for (input, _) in CORPUS {
            let e = ours(input);
            println!(
                "    (\n        {},\n        Expect {{\n            rfc3339: {},\n            lenient: {},\n            date: {},\n            lenient_date: {},\n            kept: {},\n            rfc2822: {},\n            stamp_serde: {},\n            offset_stamp_serde: {},\n        }},\n    ),",
                literal(input),
                field(e.rfc3339),
                field(e.lenient),
                field(e.date),
                field(e.lenient_date),
                field(e.kept),
                field(e.rfc2822),
                field(e.stamp_serde),
                field(e.offset_stamp_serde),
            );
        }
    }

    #[test]
    fn printed_forms_of_parsed_values() {
        let strict = |s: &str| parse_rfc3339(s).unwrap().rfc3339();
        assert_eq!(
            strict("2026-01-05T10:17:19.1234567891Z"),
            "2026-01-05T10:17:19.123456789+00:00"
        );
        assert_eq!(
            strict("2026-01-05T10:17:19\u{2212}02:00"),
            "2026-01-05T10:17:19-02:00"
        );
        assert_eq!(
            strict("2026-01-05T10:17:19-00:00"),
            "2026-01-05T10:17:19+00:00"
        );
        assert_eq!(
            strict("2026-01-05t10:17:19.5z"),
            "2026-01-05T10:17:19.500+00:00"
        );
        let lenient = |s: &str| parse_lenient_offset(s).unwrap().rfc3339();
        assert_eq!(lenient("2026-1-5T1:7:9 +0200"), "2026-01-05T01:07:09+02:00");
        assert_eq!(
            lenient("+2026-01-05T10:17:19Z"),
            "2026-01-05T10:17:19+00:00"
        );
        assert_eq!(
            parse_kept_stamp("20260105T101719Z")
                .unwrap()
                .rfc3339_secs_z(),
            "2026-01-05T10:17:19Z"
        );
        let single = parse_rfc2822("Mon, 5 Jan 2026 10:17:19 GMT").unwrap();
        assert_eq!(single.rfc3339(), "2026-01-05T10:17:19+00:00");
    }

    /// J4: jiff cannot hold a leap second, so `:60` is read as `:59` with
    /// the fraction kept, in every grammar that reads one.
    #[test]
    fn a_leap_second_is_read_as_59() {
        assert_eq!(
            parse_rfc3339("2026-12-31T23:59:60Z").unwrap().rfc3339(),
            "2026-12-31T23:59:59+00:00"
        );
        assert_eq!(
            parse_rfc3339("2026-12-31T23:59:60.5+02:00")
                .unwrap()
                .rfc3339(),
            "2026-12-31T23:59:59.500+02:00"
        );
        assert_eq!(
            parse_lenient_stamp("2026-01-05T10:17:60Z")
                .unwrap()
                .rfc3339(),
            "2026-01-05T10:17:59+00:00"
        );
        assert_eq!(
            parse_kept_stamp("20260105T101760Z").unwrap().compact_z(),
            "20260105T101759Z"
        );
        assert_eq!(
            parse_rfc2822("Thu, 31 Dec 2026 23:59:60 GMT")
                .unwrap()
                .rfc3339(),
            "2026-12-31T23:59:59+00:00"
        );
        assert_eq!(
            parse_rfc3339("2026-12-31T23:59:60Z"),
            parse_rfc3339("2026-12-31T23:59:59Z")
        );
    }

    /// J4: a year outside 0000 to 9999, or an instant past jiff's range, is
    /// refused like text that is no date.
    #[test]
    fn years_outside_0000_to_9999_are_refused() {
        for input in [
            "+10000-01-01T00:00:00Z",
            "-0001-01-05T10:17:19Z",
            "-2026-01-05T10:17:19+02:00",
            "9999-12-31T23:59:59Z",
            "9999-12-31T00:00:00-23:59",
        ] {
            assert_eq!(parse_lenient_offset(input), None, "{input:?}");
            assert_eq!(parse_lenient_stamp(input), None, "{input:?}");
        }
        assert_eq!(parse_rfc3339("9999-12-31T23:59:59Z"), None);
        for input in ["+10000-01-01", "-0001-01-01"] {
            assert_eq!(parse_date(input), None, "{input:?}");
            assert_eq!(parse_lenient_date(input), None, "{input:?}");
        }
        assert_eq!(parse_date("9999-12-31").unwrap().ymd(), "9999-12-31");
        assert_eq!(parse_date("0000-01-01").unwrap().ymd(), "0000-01-01");
        assert_eq!(
            parse_rfc3339("9999-12-30T22:00:00.999999999Z")
                .unwrap()
                .rfc3339(),
            "9999-12-30T22:00:00.999999999+00:00"
        );
        assert_eq!(parse_rfc3339("9999-12-30T22:00:01Z"), None);
        assert_eq!(
            parse_rfc3339("0000-01-01T00:00:00\u{2212}23:59")
                .unwrap()
                .rfc3339(),
            "0000-01-01T00:00:00-23:59"
        );
        // The latest date has no midnight jiff can hold in UTC.
        assert_eq!(parse_date("9999-12-31").unwrap().start_utc(), None);
    }

    /// The hand printers on plain numbers, also where jiff holds no value
    /// (a leap second, years past 9999): chrono's bytes, taken from 0.33.
    #[test]
    fn emitter_corners_write_what_chrono_wrote() {
        // (seconds, nanoseconds, RFC 3339 at offset 0, at -29, at 86399,
        // at -16200, Debug in UTC, the HTTP date, Display in UTC)
        let rows: &[(i64, u32, [&str; 7])] = &[
            (
                -62_198_668_801,
                0,
                [
                    "-0001-01-01T23:59:59+00:00",
                    "-0001-01-01T23:59:30-00:00",
                    "-0001-01-02T23:59:58+24:00",
                    "-0001-01-01T19:29:59-04:30",
                    "-0001-01-01T23:59:59Z",
                    "Fri, 01 Jan -0001 23:59:59 GMT",
                    "-0001-01-01 23:59:59 UTC",
                ],
            ),
            (
                -377_721_014_401,
                5,
                [
                    "-10000-06-30T23:59:59.000000005+00:00",
                    "-10000-06-30T23:59:30.000000005-00:00",
                    "-10000-07-01T23:59:58.000000005+24:00",
                    "-10000-06-30T19:29:59.000000005-04:30",
                    "-10000-06-30T23:59:59.000000005Z",
                    "Fri, 30 Jun -10000 23:59:59 GMT",
                    "-10000-06-30 23:59:59.000000005 UTC",
                ],
            ),
            (
                253_402_387_199,
                120_000_000,
                [
                    "+10000-01-01T23:59:59.120+00:00",
                    "+10000-01-01T23:59:30.120-00:00",
                    "+10000-01-02T23:59:58.120+24:00",
                    "+10000-01-01T19:29:59.120-04:30",
                    "+10000-01-01T23:59:59.120Z",
                    "Sat, 01 Jan +10000 23:59:59 GMT",
                    "+10000-01-01 23:59:59.120 UTC",
                ],
            ),
            (
                8_210_266_790_399,
                0,
                [
                    "+262142-12-30T23:59:59+00:00",
                    "+262142-12-30T23:59:30-00:00",
                    "+262142-12-31T23:59:58+24:00",
                    "+262142-12-30T19:29:59-04:30",
                    "+262142-12-30T23:59:59Z",
                    "Sun, 30 Dec +262142 23:59:59 GMT",
                    "+262142-12-30 23:59:59 UTC",
                ],
            ),
            (
                -8_334_601_056_001,
                0,
                [
                    "-262143-01-02T23:59:59+00:00",
                    "-262143-01-02T23:59:30-00:00",
                    "-262143-01-03T23:59:58+24:00",
                    "-262143-01-02T19:29:59-04:30",
                    "-262143-01-02T23:59:59Z",
                    "Fri, 02 Jan -262143 23:59:59 GMT",
                    "-262143-01-02 23:59:59 UTC",
                ],
            ),
            (
                253_402_300_799,
                999_999_999,
                [
                    "9999-12-31T23:59:59.999999999+00:00",
                    "9999-12-31T23:59:30.999999999-00:00",
                    "+10000-01-01T23:59:58.999999999+24:00",
                    "9999-12-31T19:29:59.999999999-04:30",
                    "9999-12-31T23:59:59.999999999Z",
                    "Fri, 31 Dec 9999 23:59:59 GMT",
                    "9999-12-31 23:59:59.999999999 UTC",
                ],
            ),
            (
                1_798_761_599,
                1_000_000_000,
                [
                    "2026-12-31T23:59:60+00:00",
                    "2026-12-31T23:59:31-00:00",
                    "2027-01-01T23:59:59+24:00",
                    "2026-12-31T19:29:60-04:30",
                    "2026-12-31T23:59:60Z",
                    "Thu, 31 Dec 2026 23:59:60 GMT",
                    "2026-12-31 23:59:60 UTC",
                ],
            ),
            (
                1_798_761_599,
                1_123_456_000,
                [
                    "2026-12-31T23:59:60.123456+00:00",
                    "2026-12-31T23:59:31.123456-00:00",
                    "2027-01-01T23:59:59.123456+24:00",
                    "2026-12-31T19:29:60.123456-04:30",
                    "2026-12-31T23:59:60.123456Z",
                    "Thu, 31 Dec 2026 23:59:60 GMT",
                    "2026-12-31 23:59:60.123456 UTC",
                ],
            ),
            (
                -1,
                1,
                [
                    "1969-12-31T23:59:59.000000001+00:00",
                    "1969-12-31T23:59:30.000000001-00:00",
                    "1970-01-01T23:59:58.000000001+24:00",
                    "1969-12-31T19:29:59.000000001-04:30",
                    "1969-12-31T23:59:59.000000001Z",
                    "Wed, 31 Dec 1969 23:59:59 GMT",
                    "1969-12-31 23:59:59.000000001 UTC",
                ],
            ),
            (
                -62_167_132_801,
                1_000,
                [
                    "0000-01-01T23:59:59.000001+00:00",
                    "0000-01-01T23:59:30.000001-00:00",
                    "0000-01-02T23:59:58.000001+24:00",
                    "0000-01-01T19:29:59.000001-04:30",
                    "0000-01-01T23:59:59.000001Z",
                    "Sat, 01 Jan 0000 23:59:59 GMT",
                    "0000-01-01 23:59:59.000001 UTC",
                ],
            ),
        ];
        for (secs, nanos, [utc, minus_29, plus_day, minus_430, debug, http, display]) in rows {
            let (secs, nanos) = (*secs, *nanos);
            assert_eq!(emit::rfc3339(secs, nanos, 0), *utc);
            assert_eq!(emit::rfc3339(secs, nanos, -29), *minus_29);
            assert_eq!(emit::rfc3339(secs, nanos, 86_399), *plus_day);
            assert_eq!(emit::rfc3339(secs, nanos, -16_200), *minus_430);
            assert_eq!(emit::debug_utc(secs, nanos), *debug);
            assert_eq!(emit::http_date(secs, nanos), *http);
            assert_eq!(emit::display_utc(secs, nanos), *display);
            // Where jiff holds the instant, the values print the same.
            if let Some(stamp) = Stamp::from_unix(secs, nanos) {
                assert_eq!(stamp.rfc3339(), *utc);
                assert_eq!(format!("{stamp:?}"), *debug);
                assert_eq!(stamp.http_date(), *http);
                assert_eq!(stamp.display_utc(), *display);
                let written = OffsetStamp::at_offset(stamp, -16_200).unwrap();
                assert_eq!(written.rfc3339(), *minus_430);
            }
        }
        assert_eq!(
            emit::debug_offset(-62_198_668_801, 0, -29),
            "-0001-01-01T23:59:30-00:00:29"
        );
        assert_eq!(
            emit::debug_offset(1_798_761_599, 1_123_456_000, 86_399),
            "2027-01-01T23:59:59.123456+23:59:59"
        );
        assert_eq!(
            emit::debug_offset(253_402_387_199, 120_000_000, -16_200),
            "+10000-01-01T19:29:59.120-04:30"
        );
    }

    #[test]
    fn offset_stamp_day_is_the_written_date() {
        let late = parse_rfc3339("2026-01-05T23:30:00-05:00").unwrap();
        assert_eq!(late.day().ymd(), "2026-01-05");
        assert_eq!(late.stamp().day_utc().ymd(), "2026-01-06");
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
        assert_eq!(seconds(90).to_std(), Some(Duration::from_secs(90)));
        assert_eq!(millis(1_500).to_std(), Some(Duration::from_millis(1_500)));
        assert_eq!(Delta::ZERO.to_std(), Some(Duration::ZERO));
        assert_eq!(seconds(-1).to_std(), None);
    }

    #[test]
    fn from_std_keeps_nanoseconds() {
        let value = Duration::new(301, 123_456_789);
        let delta = Delta::from_std(value).unwrap();
        assert_eq!(delta.num_millis(), 301_123);
        assert_eq!(
            format!("{delta:?}"),
            "TimeDelta { secs: 301, nanos: 123456789 }"
        );
        assert_eq!(delta.to_std(), Some(value));
        assert_eq!(Delta::from_std(Duration::ZERO), Some(Delta::ZERO));
        assert_eq!(Delta::from_std(Duration::MAX), None);
    }

    #[test]
    fn operators_move_like_chrono() {
        let start = base(120_000_000);
        let span = days(2) + millis(5);
        assert_eq!((start + span).rfc3339(), "2026-01-07T10:17:19.125+00:00");
        assert_eq!((start - span).rfc3339(), "2026-01-03T10:17:19.115+00:00");
        assert_eq!((start + span) - start, span);
        assert_eq!(start - (start + span), -span);
        let mut moving = start;
        moving += seconds(1);
        assert_eq!(moving.unix_seconds(), BASE_SECS + 1);
        moving -= seconds(2);
        assert_eq!(moving.unix_seconds(), BASE_SECS - 1);
        let day = Day::from_ymd(2026, 1, 5).unwrap();
        assert_eq!((day + days(1)).ymd(), "2026-01-06");
        assert_eq!((day - days(5)).ymd(), "2025-12-31");
        // Only whole days move a date, truncated toward zero, as with
        // chrono.
        assert_eq!((day + hours(47)).ymd(), "2026-01-06");
        assert_eq!((day - hours(47)).ymd(), "2026-01-04");
        assert_eq!((day + -hours(47)).ymd(), "2026-01-04");
    }

    #[test]
    fn offset_stamp_moves_and_keeps_its_offset_as_chrono_does() {
        let written = parse_rfc3339("2026-01-05T23:30:00.120-05:00").unwrap();
        let span = hours(1) + millis(5);
        assert_eq!((written + span).rfc3339(), "2026-01-06T00:30:00.125-05:00");
        assert_eq!((written - span).rfc3339(), "2026-01-05T22:30:00.115-05:00");
        assert_eq!((written + span).offset_seconds(), -18_000);
        assert_eq!((written + span).day().ymd(), "2026-01-06");
    }

    #[test]
    #[should_panic(expected = "`Stamp + Delta` overflowed")]
    fn stamp_plus_delta_panics_out_of_range_as_chrono_does() {
        let _ = Stamp::UNIX_EPOCH + days(400_000_000);
    }

    #[test]
    #[should_panic(expected = "`OffsetStamp + Delta` overflowed")]
    fn offset_stamp_plus_delta_panics_out_of_range_as_chrono_does() {
        let _ = Stamp::UNIX_EPOCH.to_offset_utc() + days(400_000_000);
    }

    #[test]
    fn constructors() {
        assert_eq!(Stamp::default(), Stamp::UNIX_EPOCH);
        assert_eq!(
            (
                Stamp::UNIX_EPOCH.unix_seconds(),
                Stamp::UNIX_EPOCH.subsec_nanos()
            ),
            (0, 0)
        );

        let built = Stamp::from_ymd_hms(2026, 1, 5, 10, 17, 19).unwrap();
        assert_eq!(built.rfc3339_secs_z(), "2026-01-05T10:17:19Z");
        assert_eq!(built, base(0));
        assert_eq!(Stamp::from_ymd_hms(2026, 2, 30, 0, 0, 0), None);
        assert_eq!(Stamp::from_ymd_hms(2026, 1, 5, 24, 0, 0), None);
        assert_eq!(Stamp::from_ymd_hms(2026, 1, 5, 10, 17, 60), None);

        // (time, seconds, nanoseconds) as chrono's `From<SystemTime>` gave.
        let times = [
            (
                UNIX_EPOCH + Duration::new(1_767_608_239, 120_000_000),
                1_767_608_239,
                120_000_000,
            ),
            (UNIX_EPOCH, 0, 0),
            // 100 ns: Windows keeps SystemTime in 100 ns ticks, so 1 ns
            // before the epoch would round to the epoch itself there.
            (UNIX_EPOCH - Duration::new(0, 100), -1, 999_999_900),
            (UNIX_EPOCH - Duration::new(86_400, 0), -86_400, 0),
            (UNIX_EPOCH - Duration::new(1, 500_000_000), -2, 500_000_000),
        ];
        for (time, secs, nanos) in times {
            let stamp = Stamp::from_system_time(time).unwrap();
            assert_eq!(
                (stamp.unix_seconds(), stamp.subsec_nanos()),
                (secs, nanos),
                "{time:?}"
            );
        }

        // (microseconds, seconds, nanoseconds)
        for (micros, secs, nanos) in [
            (0, 0, 0),
            (1, 0, 1_000),
            (-1, -1, 999_999_000),
            (1_767_608_239_120_001, 1_767_608_239, 120_001_000),
            (-1_500_001, -2, 499_999_000),
        ] {
            let ours = Stamp::from_unix_micros(micros).unwrap();
            assert_eq!((ours.unix_seconds(), ours.subsec_nanos()), (secs, nanos));
            assert_eq!(ours.unix_micros(), micros);
        }
        assert_eq!(Stamp::from_unix_micros(i64::MAX), None);
    }

    /// R5, R7-R9: a file time past jiff's range is `None`, never a panic.
    #[test]
    fn a_system_time_out_of_range_is_none() {
        let far = Duration::from_secs(300_000_000_000);
        if let Some(late) = UNIX_EPOCH.checked_add(far) {
            assert_eq!(Stamp::from_system_time(late), None);
        }
        if let Some(early) = UNIX_EPOCH.checked_sub(far + far) {
            assert_eq!(Stamp::from_system_time(early), None);
        }
    }

    #[test]
    fn display_utc_writes_chronos_display() {
        let fractions = ["", ".500", ".120", ".123450", ".123456789"];
        for (nanos, fraction) in NANOS.into_iter().zip(fractions) {
            assert_eq!(
                base(nanos).display_utc(),
                format!("2026-01-05 10:17:19{fraction} UTC")
            );
        }
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
        assert_eq!(after.since(last), nanos(2));
        assert_eq!(last.since(after).num_millis(), 0);
        assert_eq!(
            last.since(Stamp::from_unix(-2, 0).unwrap()).num_millis(),
            1_999
        );
        assert_eq!(
            Stamp::from_unix(-86_401, 500_000_000).unwrap().rfc3339(),
            "1969-12-30T23:59:59.500+00:00"
        );
        let early = Stamp::from_unix(-62_198_668_801, 250_000_000).unwrap();
        assert_eq!(early.day_utc().ymd(), "-0001-01-01");
        assert_eq!(format!("{:?}", early.day_utc()), "-0001-01-01");
    }

    #[test]
    fn debug_forms_are_chronos() {
        let at = base(120_000_000);
        assert_eq!(format!("{at:?}"), "2026-01-05T10:17:19.120Z");
        assert_eq!(
            format!("{:?}", parse_rfc3339("2026-01-05T12:17:19+02:00").unwrap()),
            "2026-01-05T12:17:19+02:00"
        );
        assert_eq!(
            format!("{:?}", at.to_offset_utc()),
            "2026-01-05T10:17:19.120+00:00"
        );
        let odd = OffsetStamp::at_offset(at, 5_405).unwrap();
        assert_eq!(format!("{odd:?}"), "2026-01-05T11:47:24.120+01:30:05");
        assert_eq!(odd.rfc3339(), "2026-01-05T11:47:24.120+01:30");
        assert_eq!(format!("{:?}", at.day_utc()), "2026-01-05");
        assert_eq!(format!("{:?}", Some(at.day_utc())), "Some(2026-01-05)");
        assert_eq!(
            format!("{:?}", Stamp::from_unix(-1, 999_999_999).unwrap()),
            "1969-12-31T23:59:59.999999999Z"
        );
    }

    #[test]
    fn compact_formats_hold_at_the_edges() {
        // (seconds, nanoseconds) and chrono's `%Y-%m-%d`, `%Y-%m-%d %H:%M
        // UTC`, `%Y%m%dT%H%M%S%.3fZ`, `%Y%m%dT%H%M%SZ` and `%y%m%d%H%M%S`.
        let rows: &[(i64, u32, [&str; 5])] = &[
            (
                -62_167_219_200,
                0,
                [
                    "0000-01-01",
                    "0000-01-01 00:00 UTC",
                    "00000101T000000.000Z",
                    "00000101T000000Z",
                    "000101000000",
                ],
            ),
            (
                253_402_207_200,
                999_999_999,
                [
                    "9999-12-30",
                    "9999-12-30 22:00 UTC",
                    "99991230T220000.999Z",
                    "99991230T220000Z",
                    "991230220000",
                ],
            ),
            (
                -14_182_940,
                500_000_000,
                [
                    "1969-07-20",
                    "1969-07-20 20:17 UTC",
                    "19690720T201740.500Z",
                    "19690720T201740Z",
                    "690720201740",
                ],
            ),
            (
                BASE_SECS,
                123_999_999,
                [
                    "2026-01-05",
                    "2026-01-05 10:17 UTC",
                    "20260105T101719.123Z",
                    "20260105T101719Z",
                    "260105101719",
                ],
            ),
            (
                -62_167_305_600,
                999_999_999,
                [
                    "-0001-12-31",
                    "-0001-12-31 00:00 UTC",
                    "-00011231T000000.999Z",
                    "-00011231T000000Z",
                    "991231000000",
                ],
            ),
            (
                -100_000_000_000,
                999_999_999,
                [
                    "-1199-02-15",
                    "-1199-02-15 14:13 UTC",
                    "-11990215T141320.999Z",
                    "-11990215T141320Z",
                    "010215141320",
                ],
            ),
            (
                -377_000_000_000,
                999_999_999,
                [
                    "-9977-05-07",
                    "-9977-05-07 01:46 UTC",
                    "-99770507T014640.999Z",
                    "-99770507T014640Z",
                    "230507014640",
                ],
            ),
        ];
        for (secs, nanos, [ymd, hm, millis_z, z, short]) in rows {
            let stamp = Stamp::from_unix(*secs, *nanos).unwrap();
            assert_eq!(stamp.day_utc().ymd(), *ymd, "{secs}");
            assert_eq!(stamp.ymd_hm_utc(), *hm, "{secs}");
            assert_eq!(stamp.compact_millis_z(), *millis_z, "{secs}");
            assert_eq!(stamp.compact_z(), *z, "{secs}");
            assert_eq!(stamp.yymmddhhmmss(), *short, "{secs}");
        }
        let max = Stamp(Timestamp::MAX);
        assert_eq!(max.compact_millis_z(), "99991230T220000.999Z");
        assert_eq!(Day(Date::MIN).ymd(), "-9999-01-01");
        assert_eq!(Day(Date::MAX).ymd(), "9999-12-31");
    }

    #[test]
    fn http_date_writes_what_chrono_wrote() {
        assert_eq!(base(0).http_date(), "Mon, 05 Jan 2026 10:17:19 GMT");
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
        // BP11: a single-digit day is padded.
        assert_eq!(
            Stamp::from_ymd_hms(2026, 3, 1, 8, 5, 3)
                .unwrap()
                .http_date(),
            "Sun, 01 Mar 2026 08:05:03 GMT"
        );
        assert_eq!(
            Stamp::from_unix(-1, 999_999_999).unwrap().http_date(),
            "Wed, 31 Dec 1969 23:59:59 GMT"
        );
    }

    #[test]
    fn saturating_sub_days_never_panics() {
        let day = Day::from_ymd(2026, 1, 5).unwrap();
        let far = day.saturating_sub_days(20000 * 366);
        assert!(far < day);
        assert_eq!(far, Day(Date::MIN));
        assert_eq!(far.ymd(), "-9999-01-01");
        assert!(day.saturating_sub_days(i64::MAX) <= far);
        assert!(day.saturating_sub_days(i64::MIN) > day);
        assert!(day.saturating_sub_days(-(20000 * 366)) > day);
        assert_eq!(day.saturating_sub_days(i64::MIN), Day(Date::MAX));
    }

    #[test]
    fn saturating_moves_stop_at_the_ends_instead_of_panicking() {
        let now = base(0);
        // Inside the range nothing is held.
        assert_eq!(now.saturating_add(seconds(60)), now + seconds(60));
        assert_eq!(now.saturating_sub(seconds(60)), now - seconds(60));
        // The longest span either way, and past it.
        assert_eq!(saturating_seconds(DELTA_MAX_SECS), seconds(DELTA_MAX_SECS));
        for n in [DELTA_MAX_SECS, DELTA_MAX_SECS + 1, i64::MAX] {
            let span = saturating_seconds(n);
            if n > DELTA_MAX_SECS {
                assert_eq!(span, Delta::MAX, "{n}");
            }
            assert_eq!(now.saturating_add(span), Stamp::LATEST, "{n}");
            assert_eq!(now.saturating_sub(span), Stamp::EARLIEST, "{n}");
            assert_eq!(now.saturating_add(-span), Stamp::EARLIEST, "{n}");
            assert_eq!(now.saturating_sub(-span), Stamp::LATEST, "{n}");
        }
        assert_eq!(saturating_seconds(i64::MIN), Delta::MIN);
        assert_eq!(saturating_seconds(-DELTA_MAX_SECS - 1), Delta::MIN);
        // A span that only just crosses the latest instant from now.
        let left = Stamp::LATEST.since(now).num_seconds();
        assert_eq!(now.saturating_add(seconds(left)), Stamp::LATEST);
        assert_eq!(now.saturating_add(seconds(left + 1)), Stamp::LATEST);
        assert_eq!(Stamp::LATEST.rfc3339(), "9999-12-30T22:00:00+00:00");
        assert_eq!(
            parse_rfc3339(&Stamp::LATEST.rfc3339()).map(|at| at.stamp()),
            Some(Stamp::LATEST)
        );
        assert_eq!(Stamp::EARLIEST.checked_sub(seconds(1)), None);
    }

    #[test]
    fn an_instant_past_the_latest_is_told_apart_from_one_nobody_can_read() {
        for past in [
            "9999-12-31T23:59:59Z",
            "9999-12-31T00:00:00Z",
            "9999-12-30T22:00:01Z",
            "9999-12-31T01:00:00+02:00",
        ] {
            assert_eq!(parse_rfc3339(past), None, "{past}");
            assert!(rfc3339_past_latest(past), "{past}");
        }
        for not_past in [
            "9999-12-30T22:00:00Z",
            "2026-01-05T10:17:19Z",
            "never",
            "",
            "9999-12-32T00:00:00Z",
            "9999-12-31T24:00:00Z",
            "9999-12-31",
        ] {
            assert!(!rfc3339_past_latest(not_past), "{not_past}");
        }
    }
}
