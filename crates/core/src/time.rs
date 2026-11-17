//! Dates and instants for every crate: the one place that knows which date
//! library sits underneath.
//!
//! Every other crate talks to dates through this module only: the newtypes
//! [`Stamp`] (a UTC instant), [`OffsetStamp`] (an instant plus the offset it
//! was written with), [`Day`] (a calendar date) and [`Delta`] (an exact
//! span of seconds and nanoseconds), the clock ([`now`], [`today_utc`]), the
//! duration helpers ([`days`], [`hours`], [`minutes`], [`seconds`],
//! [`millis`]) and the named emitters. None of the newtypes implements
//! `Display` or serde's traits, so every byte that leaves Crystalline goes
//! through a helper named here.
//!
//! The values are still backed by chrono in this stage; the names are what
//! the rest of the workspace moves onto first.

use chrono::{DateTime, Days, FixedOffset, NaiveDate, SecondsFormat, TimeDelta, Utc};
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::ops::{Add, Mul, Neg, Sub};

/// A UTC instant with nanosecond precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp(DateTime<Utc>);

/// An instant together with the UTC offset it was written with.
///
/// Equality, ordering and hashing look at the instant only, so
/// `10:00+02:00` and `08:00+00:00` are the same value; the offset matters
/// only when the value is written out again.
#[derive(Clone, Copy, Debug)]
pub struct OffsetStamp(DateTime<FixedOffset>);

/// A calendar date with no time of day and no offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
pub fn days(n: i64) -> Delta {
    Delta(TimeDelta::days(n))
}

/// `n` hours. Panics when the span is out of range.
pub fn hours(n: i64) -> Delta {
    Delta(TimeDelta::hours(n))
}

/// `n` minutes. Panics when the span is out of range.
pub fn minutes(n: i64) -> Delta {
    Delta(TimeDelta::minutes(n))
}

/// `n` seconds. Panics when the span is out of range.
pub fn seconds(n: i64) -> Delta {
    Delta(TimeDelta::seconds(n))
}

/// `n` milliseconds.
pub fn millis(n: i64) -> Delta {
    Delta(TimeDelta::milliseconds(n))
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
        self.0.to_rfc3339()
    }

    /// RFC 3339 in whole seconds with `Z`: `2026-01-05T10:17:19Z`.
    pub fn rfc3339_secs_z(&self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    /// RFC 3339 in whole seconds with `+00:00`: `2026-01-05T10:17:19+00:00`.
    pub fn rfc3339_secs(&self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Secs, false)
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

    /// The written offset in seconds east of UTC.
    pub fn offset_seconds(&self) -> i32 {
        self.0.offset().local_minus_utc()
    }

    /// RFC 3339 in the written offset (`+00:00` for offset zero) with the
    /// fraction in groups of none, 3, 6 or 9 digits, as chrono's
    /// `to_rfc3339` writes it.
    pub fn rfc3339(&self) -> String {
        self.0.to_rfc3339()
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
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
