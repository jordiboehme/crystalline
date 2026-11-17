//! The hand-written date grammars against chrono, on strings made by
//! mutating valid ones.
//!
//! Each string goes through every parser and serde adapter in
//! `crystalline_core::time` and the chrono function it stands in for: both
//! must refuse, or both accept with the same instant, the same offset, the
//! same RFC 3339 bytes and the same serialized bytes. Both sides are reduced
//! to plain numbers and strings, read through public accessors, so the test
//! needs no bridge between the two and runs unchanged once jiff is behind
//! the module.
//!
//! While chrono is behind the module no difference is allowed; once jiff
//! is, `known_difference` names a difference that a ruling allows (and only
//! when our side gives exactly the ruled result), and `KNOWN_DIFFERENCES`
//! lists the names that may be skipped.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Utc};
use crystalline_core::time::{
    Day, OffsetStamp, Stamp, parse_date, parse_kept_stamp, parse_lenient_date,
    parse_lenient_offset, parse_lenient_stamp, parse_rfc2822, parse_rfc3339,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// The `_option` adapters, one field each.
#[derive(Serialize, Deserialize)]
struct MaybeStampField(
    #[serde(default, with = "crystalline_core::time::serde::stamp_option")] Option<Stamp>,
);
#[derive(Serialize, Deserialize)]
struct MaybeOffsetField(
    #[serde(default, with = "crystalline_core::time::serde::offset_stamp_option")]
    Option<OffsetStamp>,
);
#[derive(Serialize, Deserialize)]
struct MaybeDayField(
    #[serde(default, with = "crystalline_core::time::serde::day_option")] Option<Day>,
);

/// The adapters of `crystalline_core::time::serde` on one field each.
#[derive(Serialize, Deserialize)]
struct StampField(#[serde(with = "crystalline_core::time::serde::stamp")] Stamp);
#[derive(Serialize, Deserialize)]
struct OffsetField(#[serde(with = "crystalline_core::time::serde::offset_stamp")] OffsetStamp);
#[derive(Serialize, Deserialize)]
struct DayField(#[serde(with = "crystalline_core::time::serde::day")] Day);

/// The differences a ruling allows, as (name, reason), each one a name
/// [`known_difference`] can return. Empty while chrono is behind
/// `crystalline_core::time`.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[];

/// J4: chrono keeps `:60`; the ruled result is the same instant with the
/// second held at 59 and the same fraction.
const LEAP_SECOND_CLAMPED: &str = "leap second clamped";

/// J4: a year outside 0000-9999, or an instant outside jiff's `Timestamp`
/// range, is refused.
const OUT_OF_RANGE_REFUSED: &str = "out of range refused";

/// jiff's `Timestamp::MIN` and `Timestamp::MAX` in whole Unix seconds:
/// -9999-01-01T00:00:00 plus 25:59:59, and 9999-12-31T23:59:59 minus it.
const TIMESTAMP_MIN_SECONDS: i64 = -377_705_023_201;
const TIMESTAMP_MAX_SECONDS: i64 = 253_402_207_200;

/// The random strings that also go through the serde adapter rows: one in
/// this many. Those rows cost about four times the parser rows (YAML most of
/// all); every seed and every one-edit string goes through them all.
const ADAPTER_EVERY: usize = 5;

/// How many mutated strings one run checks.
const STRINGS: usize = 300_000;

/// Valid strings to mutate, one or more for every grammar.
const SEEDS: &[&str] = &[
    "2026-01-05T10:17:19Z",
    "2026-01-05T10:17:19+00:00",
    "2026-01-05T10:17:19.120+02:00",
    "2026-01-05t10:17:19.123456789z",
    "2026-01-05 10:17:19-04:30",
    "2026-12-31T23:59:60Z",
    "9999-12-31T23:59:59.999999999-00:00",
    "0000-01-01T00:00:00\u{2212}23:59",
    "2024-02-29T12:00:00.5+05:45",
    " 2026-1-5T1:7:9 +0200 ",
    "+2026-01-05T10:17:19 UTC",
    "-0001-01-05T10:17:19+02 00",
    "2026 - 01 - 05T10:17:19.1234567891Z",
    "+10000-01-01T00:00:00Z",
    "2026-01-05",
    "2026-1-5",
    " 2026-01-05 ",
    "2026 - 01 - 05",
    "-0001-01-01",
    "20260105T101719Z",
    "20261231T235960Z",
    "Mon, 05 Jan 2026 10:17:19 +0000",
    "Mon, 5 Jan 2026 10:17:19 GMT",
    "5 Jan 26 10:17 EST (comment)",
    "Thu, 31 Dec 2026 23:59:60 -0230",
    "Mon, 05 Jan 2026 10:17:19 A",
];

/// Characters a mutation may put in: digits come from the digit change,
/// these are the separators and names the grammars react to.
const ALPHABET: &[char] = &[
    'T', 't', ' ', '-', ':', '.', 'Z', 'z', '+', '\u{2212}', '\t', '\n', '\u{a0}', '\u{3000}', 'U',
    'C', 'u', 'c', ',', '(', ')', '\\', '[', ']', 'J', 'a', 'n', 'M', 'o', 'G', 'E', 'S', 'P', 'D',
    'e', 'b', 'A', '0', '1', '5', '6', '9',
];

/// Endings a mutation may append: the suffixes other grammars accept and
/// these must refuse, which stacked single edits rarely build.
const SUFFIXES: &[&str] = &[":30", ":00", "[UTC]", "Z", "z", " ", "+02:00", ".5", "00"];

/// Every string one edit away from `seed`: each deletion, duplication and
/// adjacent swap, and each substitution and insertion of a digit or an
/// `ALPHABET` character, plus each suffix appended.
fn one_edit_away(seed: &str) -> Vec<String> {
    let chars: Vec<char> = seed.chars().collect();
    let pool: Vec<char> = ('0'..='9').chain(ALPHABET.iter().copied()).collect();
    let text = |v: Vec<char>| v.into_iter().collect::<String>();
    let mut out = Vec::new();
    for at in 0..chars.len() {
        let mut v = chars.clone();
        v.remove(at);
        out.push(text(v));
        let mut v = chars.clone();
        v.insert(at, chars[at]);
        out.push(text(v));
        if at + 1 < chars.len() {
            let mut v = chars.clone();
            v.swap(at, at + 1);
            out.push(text(v));
        }
        for &c in &pool {
            let mut v = chars.clone();
            v[at] = c;
            out.push(text(v));
        }
    }
    for at in 0..=chars.len() {
        for &c in &pool {
            let mut v = chars.clone();
            v.insert(at, c);
            out.push(text(v));
        }
    }
    out.extend(SUFFIXES.iter().map(|suffix| format!("{seed}{suffix}")));
    out
}

/// splitmix64: a fixed seed gives the same strings on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One to three mutations of a seed: drop, double or swap characters, change
/// a digit, replace or insert a separator, append a suffix.
fn mutate(rng: &mut Rng, seed: &str) -> String {
    let mut chars: Vec<char> = seed.chars().collect();
    for _ in 0..=rng.below(3) {
        if chars.is_empty() {
            chars.push(ALPHABET[rng.below(ALPHABET.len())]);
            continue;
        }
        let at = rng.below(chars.len());
        match rng.below(7) {
            0 => {
                chars.remove(at);
            }
            1 => chars.insert(at, chars[at]),
            2 if at + 1 < chars.len() => chars.swap(at, at + 1),
            3 => {
                let digits: Vec<usize> = (0..chars.len())
                    .filter(|&i| chars[i].is_ascii_digit())
                    .collect();
                if !digits.is_empty() {
                    let place = digits[rng.below(digits.len())];
                    chars[place] = char::from(b'0' + rng.below(10) as u8);
                }
            }
            4 => chars[at] = ALPHABET[rng.below(ALPHABET.len())],
            5 => chars.extend(SUFFIXES[rng.below(SUFFIXES.len())].chars()),
            _ => chars.insert(at, ALPHABET[rng.below(ALPHABET.len())]),
        }
    }
    chars.into_iter().collect()
}

/// The outcome of one parser, reduced to plain numbers and bytes: refused,
/// or the instant, the written offset, the RFC 3339 bytes and the
/// serialized bytes; for a date its fields, `%Y-%m-%d` and the serialized
/// bytes.
#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    Refused,
    Instant {
        unix_seconds: i64,
        subsec_nanos: u32,
        offset_seconds: i32,
        rfc3339: String,
        serialized: String,
    },
    Date {
        year: i32,
        month: u32,
        day: u32,
        ymd: String,
        serialized: String,
    },
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("a value serializes")
}

fn offset_outcome(value: Option<DateTime<FixedOffset>>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Instant {
        unix_seconds: v.timestamp(),
        subsec_nanos: v.timestamp_subsec_nanos(),
        offset_seconds: v.offset().local_minus_utc(),
        rfc3339: v.to_rfc3339(),
        serialized: json(&v),
    })
}

fn utc_outcome(value: Option<DateTime<Utc>>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Instant {
        unix_seconds: v.timestamp(),
        subsec_nanos: v.timestamp_subsec_nanos(),
        offset_seconds: 0,
        rfc3339: v.to_rfc3339(),
        serialized: json(&v),
    })
}

fn chrono_day(value: Option<NaiveDate>) -> Outcome {
    use chrono::Datelike;
    value.map_or(Outcome::Refused, |v| Outcome::Date {
        year: v.year(),
        month: v.month(),
        day: v.day(),
        ymd: v.format("%Y-%m-%d").to_string(),
        serialized: json(&v),
    })
}

fn our_offset(value: Option<OffsetStamp>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Instant {
        unix_seconds: v.stamp().unix_seconds(),
        subsec_nanos: v.stamp().subsec_nanos(),
        offset_seconds: v.offset_seconds(),
        rfc3339: v.rfc3339(),
        serialized: json(&OffsetField(v)),
    })
}

fn our_stamp(value: Option<Stamp>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Instant {
        unix_seconds: v.unix_seconds(),
        subsec_nanos: v.subsec_nanos(),
        offset_seconds: 0,
        rfc3339: v.rfc3339(),
        serialized: json(&StampField(v)),
    })
}

fn our_day(value: Option<Day>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Date {
        year: v.year(),
        month: v.month(),
        day: v.day(),
        ymd: v.ymd(),
        serialized: json(&DayField(v)),
    })
}

/// `text` with the seconds after its first `T` (`THH:MM:SS`) set to `59`.
fn second_59(text: &str) -> String {
    let at = text.find('T').expect("a date and time") + "THH:MM:".len();
    format!("{}59{}", &text[..at], &text[at + 2..])
}

/// The ruling that allows the difference between `ours` and chrono's
/// `theirs`, if there is one and `ours` is exactly the ruled result; `None`
/// otherwise. While chrono is behind the module this is `None` for every
/// pair, because ours is chrono's own value.
fn known_difference(ours: &Outcome, theirs: &Outcome) -> Option<&'static str> {
    match theirs {
        Outcome::Instant {
            unix_seconds,
            rfc3339,
            ..
        } if !(TIMESTAMP_MIN_SECONDS..=TIMESTAMP_MAX_SECONDS).contains(unix_seconds)
            || rfc3339.starts_with(['+', '-']) =>
        {
            (*ours == Outcome::Refused).then_some(OUT_OF_RANGE_REFUSED)
        }
        Outcome::Date { year, .. } if !(0..=9999).contains(year) => {
            (*ours == Outcome::Refused).then_some(OUT_OF_RANGE_REFUSED)
        }
        Outcome::Instant {
            unix_seconds,
            subsec_nanos,
            offset_seconds,
            rfc3339,
            serialized,
        } if *subsec_nanos >= 1_000_000_000 => {
            let clamped = Outcome::Instant {
                unix_seconds: *unix_seconds,
                subsec_nanos: subsec_nanos - 1_000_000_000,
                offset_seconds: *offset_seconds,
                rfc3339: second_59(rfc3339),
                serialized: second_59(serialized),
            };
            (*ours == clamped).then_some(LEAP_SECOND_CLAMPED)
        }
        _ => None,
    }
}

/// Every (helper, chrono function) pair on `input`, as (name, ours,
/// chrono). With `serde` unset the adapter rows are skipped (both sides
/// `Refused`), which keeps the random strings cheap.
fn compare(input: &str, serde: bool) -> Vec<(&'static str, Outcome, Outcome)> {
    let mut rows = Vec::with_capacity(22);
    let mut row = |name: &'static str, ours: &dyn Fn() -> Outcome, theirs: &dyn Fn() -> Outcome| {
        rows.push((name, ours(), theirs()));
    };
    let (json, yaml) = if serde {
        (
            serde_json::to_string(input).expect("a string serializes"),
            serde_yaml_ng::to_string(input).expect("a string serializes"),
        )
    } else {
        (String::new(), String::new())
    };
    let mut adapter =
        |name: &'static str, ours: &dyn Fn() -> Outcome, theirs: &dyn Fn() -> Outcome| {
            if serde {
                row(name, ours, theirs);
            } else {
                row(name, &|| Outcome::Refused, &|| Outcome::Refused);
            }
        };
    adapter(
        "serde::stamp (JSON) / Deserialize",
        &|| our_stamp(serde_json::from_str::<StampField>(&json).ok().map(|f| f.0)),
        &|| utc_outcome(serde_json::from_str::<DateTime<Utc>>(&json).ok()),
    );
    adapter(
        "serde::stamp (YAML) / Deserialize",
        &|| {
            our_stamp(
                serde_yaml_ng::from_str::<StampField>(&yaml)
                    .ok()
                    .map(|f| f.0),
            )
        },
        &|| utc_outcome(serde_yaml_ng::from_str::<DateTime<Utc>>(&yaml).ok()),
    );
    adapter(
        "serde::offset_stamp (JSON) / Deserialize",
        &|| our_offset(serde_json::from_str::<OffsetField>(&json).ok().map(|f| f.0)),
        &|| offset_outcome(serde_json::from_str::<DateTime<FixedOffset>>(&json).ok()),
    );
    adapter(
        "serde::offset_stamp (YAML) / Deserialize",
        &|| {
            our_offset(
                serde_yaml_ng::from_str::<OffsetField>(&yaml)
                    .ok()
                    .map(|f| f.0),
            )
        },
        &|| offset_outcome(serde_yaml_ng::from_str::<DateTime<FixedOffset>>(&yaml).ok()),
    );
    adapter(
        "serde::day (JSON) / Deserialize",
        &|| our_day(serde_json::from_str::<DayField>(&json).ok().map(|f| f.0)),
        &|| chrono_day(serde_json::from_str::<NaiveDate>(&json).ok()),
    );
    adapter(
        "serde::day (YAML) / Deserialize",
        &|| our_day(serde_yaml_ng::from_str::<DayField>(&yaml).ok().map(|f| f.0)),
        &|| chrono_day(serde_yaml_ng::from_str::<NaiveDate>(&yaml).ok()),
    );
    adapter(
        "serde::stamp_option (JSON) / Option<DateTime<Utc>>",
        &|| {
            our_stamp(
                serde_json::from_str::<MaybeStampField>(&json)
                    .ok()
                    .and_then(|f| f.0),
            )
        },
        &|| {
            utc_outcome(
                serde_json::from_str::<Option<DateTime<Utc>>>(&json)
                    .ok()
                    .flatten(),
            )
        },
    );
    adapter(
        "serde::stamp_option (YAML) / Option<DateTime<Utc>>",
        &|| {
            our_stamp(
                serde_yaml_ng::from_str::<MaybeStampField>(&yaml)
                    .ok()
                    .and_then(|f| f.0),
            )
        },
        &|| {
            utc_outcome(
                serde_yaml_ng::from_str::<Option<DateTime<Utc>>>(&yaml)
                    .ok()
                    .flatten(),
            )
        },
    );
    adapter(
        "serde::offset_stamp_option (JSON) / Option<DateTime<FixedOffset>>",
        &|| {
            our_offset(
                serde_json::from_str::<MaybeOffsetField>(&json)
                    .ok()
                    .and_then(|f| f.0),
            )
        },
        &|| {
            offset_outcome(
                serde_json::from_str::<Option<DateTime<FixedOffset>>>(&json)
                    .ok()
                    .flatten(),
            )
        },
    );
    adapter(
        "serde::offset_stamp_option (YAML) / Option<DateTime<FixedOffset>>",
        &|| {
            our_offset(
                serde_yaml_ng::from_str::<MaybeOffsetField>(&yaml)
                    .ok()
                    .and_then(|f| f.0),
            )
        },
        &|| {
            offset_outcome(
                serde_yaml_ng::from_str::<Option<DateTime<FixedOffset>>>(&yaml)
                    .ok()
                    .flatten(),
            )
        },
    );
    adapter(
        "serde::day_option (JSON) / Option<NaiveDate>",
        &|| {
            our_day(
                serde_json::from_str::<MaybeDayField>(&json)
                    .ok()
                    .and_then(|f| f.0),
            )
        },
        &|| {
            chrono_day(
                serde_json::from_str::<Option<NaiveDate>>(&json)
                    .ok()
                    .flatten(),
            )
        },
    );
    adapter(
        "serde::day_option (YAML) / Option<NaiveDate>",
        &|| {
            our_day(
                serde_yaml_ng::from_str::<MaybeDayField>(&yaml)
                    .ok()
                    .and_then(|f| f.0),
            )
        },
        &|| {
            chrono_day(
                serde_yaml_ng::from_str::<Option<NaiveDate>>(&yaml)
                    .ok()
                    .flatten(),
            )
        },
    );
    adapter(
        "parse_lenient_offset / Deserialize",
        &|| our_offset(parse_lenient_offset(input)),
        &|| offset_outcome(serde_json::from_str::<DateTime<FixedOffset>>(&json).ok()),
    );
    adapter(
        "parse_lenient_stamp / Deserialize",
        &|| our_stamp(parse_lenient_stamp(input)),
        &|| utc_outcome(serde_json::from_str::<DateTime<Utc>>(&json).ok()),
    );
    adapter(
        "parse_lenient_date / Deserialize",
        &|| our_day(parse_lenient_date(input)),
        &|| chrono_day(serde_json::from_str::<NaiveDate>(&json).ok()),
    );
    row(
        "parse_rfc3339 / parse_from_rfc3339",
        &|| our_offset(parse_rfc3339(input)),
        &|| offset_outcome(DateTime::parse_from_rfc3339(input).ok()),
    );
    row(
        "parse_lenient_offset / FromStr",
        &|| our_offset(parse_lenient_offset(input)),
        &|| offset_outcome(input.parse::<DateTime<FixedOffset>>().ok()),
    );
    row(
        "parse_lenient_stamp / FromStr",
        &|| our_stamp(parse_lenient_stamp(input)),
        &|| utc_outcome(input.parse::<DateTime<Utc>>().ok()),
    );
    row(
        "parse_date / parse_from_str",
        &|| our_day(parse_date(input)),
        &|| chrono_day(NaiveDate::parse_from_str(input, "%Y-%m-%d").ok()),
    );
    row(
        "parse_lenient_date / FromStr",
        &|| our_day(parse_lenient_date(input)),
        &|| chrono_day(input.parse::<NaiveDate>().ok()),
    );
    row(
        "parse_kept_stamp / NaiveDateTime::parse_from_str",
        &|| our_stamp(parse_kept_stamp(input)),
        &|| {
            utc_outcome(
                NaiveDateTime::parse_from_str(input, "%Y%m%dT%H%M%SZ")
                    .ok()
                    .map(|t| t.and_utc()),
            )
        },
    );
    row(
        "parse_rfc2822 / parse_from_rfc2822",
        &|| our_offset(parse_rfc2822(input)),
        &|| offset_outcome(DateTime::parse_from_rfc2822(input).ok()),
    );
    rows
}

#[test]
fn known_difference_needs_the_exact_ruled_result() {
    let leap = utc_outcome(
        DateTime::parse_from_rfc3339("2026-12-31T23:59:60.5Z")
            .ok()
            .map(|t| t.to_utc()),
    );
    let clamped = utc_outcome(
        DateTime::parse_from_rfc3339("2026-12-31T23:59:59.5Z")
            .ok()
            .map(|t| t.to_utc()),
    );
    assert_eq!(known_difference(&clamped, &leap), Some(LEAP_SECOND_CLAMPED));
    assert_eq!(known_difference(&leap, &leap), None);
    assert_eq!(known_difference(&Outcome::Refused, &leap), None);

    for text in [
        "+10000-01-01T00:00:00Z",
        "-0001-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
    ] {
        let far = utc_outcome(text.parse::<DateTime<Utc>>().ok());
        assert_eq!(
            known_difference(&Outcome::Refused, &far),
            Some(OUT_OF_RANGE_REFUSED),
            "{text}"
        );
        assert_eq!(known_difference(&far, &far), None, "{text}");
    }
    let inside = utc_outcome("9999-12-30T22:00:00Z".parse::<DateTime<Utc>>().ok());
    assert_eq!(known_difference(&Outcome::Refused, &inside), None);
    let far_day = chrono_day("-0001-01-01".parse::<NaiveDate>().ok());
    assert_eq!(
        known_difference(&Outcome::Refused, &far_day),
        Some(OUT_OF_RANGE_REFUSED)
    );
    let day = chrono_day("2026-01-05".parse::<NaiveDate>().ok());
    assert_eq!(known_difference(&Outcome::Refused, &day), None);
    assert_eq!(
        TIMESTAMP_MAX_SECONDS,
        "9999-12-31T23:59:59Z"
            .parse::<DateTime<Utc>>()
            .unwrap()
            .timestamp()
            - 93_599
    );
    assert_eq!(
        TIMESTAMP_MIN_SECONDS,
        "-9999-01-01T00:00:00Z"
            .parse::<DateTime<Utc>>()
            .unwrap()
            .timestamp()
            + 93_599
    );
    let names: Vec<&str> = KNOWN_DIFFERENCES.iter().map(|(name, _)| *name).collect();
    assert!(
        names
            .iter()
            .all(|n| [LEAP_SECOND_CLAMPED, OUT_OF_RANGE_REFUSED].contains(n))
    );
}

#[test]
fn helpers_agree_with_chrono_on_mutated_inputs() {
    // Stages 1 and 2: chrono is behind the module, so no difference is
    // allowed. Task 3.1 drops this assert when it fills KNOWN_DIFFERENCES
    // with the named rulings.
    assert!(
        KNOWN_DIFFERENCES.is_empty(),
        "chrono is behind crystalline_core::time, so no difference is allowed"
    );
    let started = Instant::now();
    let mut rng = Rng(0x5eed_2026_0105_1017);
    let names: Vec<&str> = compare("", true)
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    let allowed: Vec<&str> = KNOWN_DIFFERENCES.iter().map(|(name, _)| *name).collect();
    let mut accepted = vec![0usize; names.len()];
    let mut differences = 0usize;
    let mut mismatches = Vec::new();
    let mut check = |input: &str, serde: bool, accepted: &mut Vec<usize>| {
        for (i, (name, ours, theirs)) in compare(input, serde).into_iter().enumerate() {
            if ours != Outcome::Refused {
                accepted[i] += 1;
            }
            let ruled = known_difference(&ours, &theirs);
            if ours == theirs || ruled.is_some_and(|r| allowed.contains(&r)) {
                continue;
            }
            differences += 1;
            if mismatches.len() < 20 {
                let ruled = ruled.map_or(String::new(), |r| format!(" ({r}, not allowed)"));
                mismatches.push(format!(
                    "{name}: {input:?}{ruled}\n  ours   {ours:?}\n  chrono {theirs:?}"
                ));
            }
        }
    };
    for seed in SEEDS {
        let before = accepted.iter().sum::<usize>();
        check(seed, true, &mut accepted);
        assert!(
            accepted.iter().sum::<usize>() > before,
            "seed {seed:?} is read by no parser"
        );
    }
    let mut exhaustive = 0usize;
    for seed in SEEDS {
        for input in one_edit_away(seed) {
            check(&input, true, &mut accepted);
            exhaustive += 1;
        }
    }
    for n in 0..STRINGS {
        let input = mutate(&mut rng, SEEDS[n % SEEDS.len()]);
        check(&input, n % ADAPTER_EVERY == 0, &mut accepted);
    }
    assert!(
        mismatches.is_empty(),
        "{differences} differences from chrono (first 20):\n{}",
        mismatches.join("\n")
    );
    for (name, count) in names.iter().zip(&accepted) {
        println!("{name}: {count} accepted");
        assert!(
            *count > STRINGS / 300,
            "{name} accepted only {count} strings"
        );
    }
    println!(
        "{exhaustive} strings one edit away and {STRINGS} random ones (adapters on every {ADAPTER_EVERY}th) in {:?}",
        started.elapsed()
    );
}
