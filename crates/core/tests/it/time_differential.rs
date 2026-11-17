//! The hand-written date grammars against chrono, on strings made by
//! mutating valid ones.
//!
//! Each string goes through every parser in `crystalline_core::time` and the
//! chrono function it stands in for: both must refuse, or both accept with
//! the same instant, the same offset and the same RFC 3339 bytes. While
//! chrono is behind the module no difference is allowed; once jiff is,
//! `KNOWN_DIFFERENCES` names the ones a ruling allows.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, Utc};
use crystalline_core::time::{
    Day, OffsetStamp, Stamp, parse_date, parse_kept_stamp, parse_lenient_date,
    parse_lenient_offset, parse_lenient_stamp, parse_rfc2822, parse_rfc3339,
};
use serde::Deserialize;
use std::time::Instant;

/// The `_option` adapters, one field each.
#[derive(Deserialize)]
struct MaybeStampField(
    #[serde(default, with = "crystalline_core::time::serde::stamp_option")] Option<Stamp>,
);
#[derive(Deserialize)]
struct MaybeOffsetField(
    #[serde(default, with = "crystalline_core::time::serde::offset_stamp_option")]
    Option<OffsetStamp>,
);
#[derive(Deserialize)]
struct MaybeDayField(
    #[serde(default, with = "crystalline_core::time::serde::day_option")] Option<Day>,
);

/// The adapters of `crystalline_core::time::serde` on one field each.
#[derive(Deserialize)]
struct StampField(#[serde(with = "crystalline_core::time::serde::stamp")] Stamp);
#[derive(Deserialize)]
struct OffsetField(#[serde(with = "crystalline_core::time::serde::offset_stamp")] OffsetStamp);
#[derive(Deserialize)]
struct DayField(#[serde(with = "crystalline_core::time::serde::day")] Day);

/// The differences a ruling allows, as (name, reason). Empty while chrono
/// is behind `crystalline_core::time`.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[];

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

/// The outcome of one parser, reduced to what must match: refused, or the
/// instant, the written offset and the RFC 3339 bytes.
#[derive(Debug, PartialEq)]
enum Outcome {
    Refused,
    Instant(DateTime<Utc>, i32, String),
    Date(NaiveDate),
}

fn offset_outcome(value: Option<DateTime<FixedOffset>>) -> Outcome {
    value.map_or(Outcome::Refused, |v| {
        Outcome::Instant(v.to_utc(), v.offset().local_minus_utc(), v.to_rfc3339())
    })
}

fn utc_outcome(value: Option<DateTime<Utc>>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Instant(v, 0, v.to_rfc3339()))
}

fn our_offset(value: Option<OffsetStamp>) -> Outcome {
    value.map_or(Outcome::Refused, |v| {
        Outcome::Instant(v.stamp().into_chrono(), v.offset_seconds(), v.rfc3339())
    })
}

fn our_stamp(value: Option<Stamp>) -> Outcome {
    value.map_or(Outcome::Refused, |v| {
        Outcome::Instant(v.into_chrono(), 0, v.rfc3339())
    })
}

fn our_day(value: Option<Day>) -> Outcome {
    value.map_or(Outcome::Refused, |v| Outcome::Date(v.into_chrono()))
}

/// Every (helper, chrono function) pair on `input`, as (name, ours, chrono).
fn compare(input: &str) -> Vec<(&'static str, Outcome, Outcome)> {
    let json = serde_json::to_string(input).expect("a string serializes");
    let yaml = serde_yaml_ng::to_string(input).expect("a string serializes");
    vec![
        (
            "serde::stamp (JSON) / Deserialize",
            our_stamp(serde_json::from_str::<StampField>(&json).ok().map(|f| f.0)),
            utc_outcome(serde_json::from_str::<DateTime<Utc>>(&json).ok()),
        ),
        (
            "serde::stamp (YAML) / Deserialize",
            our_stamp(
                serde_yaml_ng::from_str::<StampField>(&yaml)
                    .ok()
                    .map(|f| f.0),
            ),
            utc_outcome(serde_yaml_ng::from_str::<DateTime<Utc>>(&yaml).ok()),
        ),
        (
            "serde::offset_stamp (JSON) / Deserialize",
            our_offset(serde_json::from_str::<OffsetField>(&json).ok().map(|f| f.0)),
            offset_outcome(serde_json::from_str::<DateTime<FixedOffset>>(&json).ok()),
        ),
        (
            "serde::offset_stamp (YAML) / Deserialize",
            our_offset(
                serde_yaml_ng::from_str::<OffsetField>(&yaml)
                    .ok()
                    .map(|f| f.0),
            ),
            offset_outcome(serde_yaml_ng::from_str::<DateTime<FixedOffset>>(&yaml).ok()),
        ),
        (
            "serde::day (JSON) / Deserialize",
            our_day(serde_json::from_str::<DayField>(&json).ok().map(|f| f.0)),
            chrono_day(serde_json::from_str::<NaiveDate>(&json).ok()),
        ),
        (
            "serde::day (YAML) / Deserialize",
            our_day(serde_yaml_ng::from_str::<DayField>(&yaml).ok().map(|f| f.0)),
            chrono_day(serde_yaml_ng::from_str::<NaiveDate>(&yaml).ok()),
        ),
        (
            "serde::stamp_option (JSON) / Option<DateTime<Utc>>",
            our_stamp(
                serde_json::from_str::<MaybeStampField>(&json)
                    .ok()
                    .and_then(|f| f.0),
            ),
            utc_outcome(
                serde_json::from_str::<Option<DateTime<Utc>>>(&json)
                    .ok()
                    .flatten(),
            ),
        ),
        (
            "serde::offset_stamp_option (JSON) / Option<DateTime<FixedOffset>>",
            our_offset(
                serde_json::from_str::<MaybeOffsetField>(&json)
                    .ok()
                    .and_then(|f| f.0),
            ),
            offset_outcome(
                serde_json::from_str::<Option<DateTime<FixedOffset>>>(&json)
                    .ok()
                    .flatten(),
            ),
        ),
        (
            "serde::day_option (JSON) / Option<NaiveDate>",
            our_day(
                serde_json::from_str::<MaybeDayField>(&json)
                    .ok()
                    .and_then(|f| f.0),
            ),
            chrono_day(
                serde_json::from_str::<Option<NaiveDate>>(&json)
                    .ok()
                    .flatten(),
            ),
        ),
        (
            "parse_rfc3339 / parse_from_rfc3339",
            our_offset(parse_rfc3339(input)),
            offset_outcome(DateTime::parse_from_rfc3339(input).ok()),
        ),
        (
            "parse_lenient_offset / FromStr",
            our_offset(parse_lenient_offset(input)),
            offset_outcome(input.parse::<DateTime<FixedOffset>>().ok()),
        ),
        (
            "parse_lenient_offset / Deserialize",
            our_offset(parse_lenient_offset(input)),
            offset_outcome(serde_json::from_str::<DateTime<FixedOffset>>(&json).ok()),
        ),
        (
            "parse_lenient_stamp / FromStr",
            our_stamp(parse_lenient_stamp(input)),
            utc_outcome(input.parse::<DateTime<Utc>>().ok()),
        ),
        (
            "parse_lenient_stamp / Deserialize",
            our_stamp(parse_lenient_stamp(input)),
            utc_outcome(serde_json::from_str::<DateTime<Utc>>(&json).ok()),
        ),
        (
            "parse_date / parse_from_str",
            our_day(parse_date(input)),
            chrono_day(NaiveDate::parse_from_str(input, "%Y-%m-%d").ok()),
        ),
        (
            "parse_lenient_date / FromStr",
            our_day(parse_lenient_date(input)),
            chrono_day(input.parse::<NaiveDate>().ok()),
        ),
        (
            "parse_lenient_date / Deserialize",
            our_day(parse_lenient_date(input)),
            chrono_day(serde_json::from_str::<NaiveDate>(&json).ok()),
        ),
        (
            "parse_kept_stamp / NaiveDateTime::parse_from_str",
            our_stamp(parse_kept_stamp(input)),
            utc_outcome(
                NaiveDateTime::parse_from_str(input, "%Y%m%dT%H%M%SZ")
                    .ok()
                    .map(|t| t.and_utc()),
            ),
        ),
        (
            "parse_rfc2822 / parse_from_rfc2822",
            our_offset(parse_rfc2822(input)),
            offset_outcome(DateTime::parse_from_rfc2822(input).ok()),
        ),
    ]
}

fn chrono_day(value: Option<NaiveDate>) -> Outcome {
    value.map_or(Outcome::Refused, Outcome::Date)
}

#[test]
fn helpers_agree_with_chrono_on_mutated_inputs() {
    assert!(
        KNOWN_DIFFERENCES.is_empty(),
        "chrono is behind crystalline_core::time, so no difference is allowed"
    );
    let started = Instant::now();
    let mut rng = Rng(0x5eed_2026_0105_1017);
    let names: Vec<&str> = compare("").into_iter().map(|(name, _, _)| name).collect();
    let mut accepted = vec![0usize; names.len()];
    let mut differences = 0usize;
    let mut mismatches = Vec::new();
    let mut check = |input: &str, accepted: &mut Vec<usize>| {
        for (i, (name, ours, theirs)) in compare(input).into_iter().enumerate() {
            if ours != Outcome::Refused {
                accepted[i] += 1;
            }
            if ours != theirs {
                differences += 1;
                if mismatches.len() < 20 {
                    mismatches.push(format!(
                        "{name}: {input:?}\n  ours   {ours:?}\n  chrono {theirs:?}"
                    ));
                }
            }
        }
    };
    for seed in SEEDS {
        let before = accepted.iter().sum::<usize>();
        check(seed, &mut accepted);
        assert!(
            accepted.iter().sum::<usize>() > before,
            "seed {seed:?} is read by no parser"
        );
    }
    let mut exhaustive = 0usize;
    for seed in SEEDS {
        for input in one_edit_away(seed) {
            check(&input, &mut accepted);
            exhaustive += 1;
        }
    }
    for n in 0..STRINGS {
        let input = mutate(&mut rng, SEEDS[n % SEEDS.len()]);
        check(&input, &mut accepted);
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
        "{exhaustive} strings one edit away and {STRINGS} random ones in {:?}",
        started.elapsed()
    );
}
