//! Serde adapters for `#[serde(with = "...")]`: the only way a date goes into
//! or out of a serialized struct, because the newtypes implement neither
//! `Serialize` nor `Deserialize`.
//!
//! Writing is what chrono's serde wrote: [`stamp`] gives `Z` and a fraction
//! of none, 3, 6 or 9 digits (`2026-01-05T10:17:19.120Z`), [`offset_stamp`]
//! gives `Z` for offset zero and `+hh:mm` otherwise, and [`day`] gives
//! `%Y-%m-%d`. Reading is what chrono's `Deserialize` accepted, through
//! [`parse_lenient_stamp`](super::parse_lenient_stamp),
//! [`parse_lenient_offset`](super::parse_lenient_offset) and
//! [`parse_lenient_date`](super::parse_lenient_date).
//!
//! Every adapter has an `_option` twin for an `Option` field. It writes
//! `None` as `null`, and the field must also carry `#[serde(default)]`:
//! `with` alone makes a missing key an error, where chrono's own `Option`
//! field read it as `None`.
//!
//! The `*_json` functions give the same text as a `serde_json::Value` for
//! `json!` sites.

use super::{
    Day, OffsetStamp, Stamp, parse_lenient_date, parse_lenient_offset, parse_lenient_stamp,
};
use ::serde::de::{self, Deserializer, Visitor};
use ::serde::ser::Serializer;
use serde_json::Value;
use std::fmt;

fn stamp_text(value: Stamp) -> String {
    let mut text = value.rfc3339();
    text.truncate(text.len() - "+00:00".len());
    text.push('Z');
    text
}

fn offset_stamp_text(value: OffsetStamp) -> String {
    if value.offset_seconds() == 0 {
        stamp_text(value.stamp())
    } else {
        value.rfc3339()
    }
}

fn day_text(value: Day) -> String {
    value.ymd()
}

/// A stamp as the text [`stamp`] writes, for a `json!` site.
pub fn stamp_json(value: Stamp) -> Value {
    Value::String(stamp_text(value))
}

/// An offset stamp as the text [`offset_stamp`] writes, for a `json!` site.
pub fn offset_stamp_json(value: OffsetStamp) -> Value {
    Value::String(offset_stamp_text(value))
}

/// A day as the text [`day`] writes, for a `json!` site.
pub fn day_json(value: Day) -> Value {
    Value::String(day_text(value))
}

struct Lenient<T> {
    parse: fn(&str) -> Option<T>,
    expecting: &'static str,
}

impl<T> Visitor<'_> for Lenient<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        (self.parse)(v).ok_or_else(|| E::invalid_value(de::Unexpected::Str(v), &self))
    }
}

struct Maybe<T>(Lenient<T>);

impl<'de, T> Visitor<'de> for Maybe<T> {
    type Value = Option<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} or null", self.0.expecting)
    }

    fn visit_none<E: de::Error>(self) -> Result<Option<T>, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Option<T>, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Option<T>, D::Error> {
        d.deserialize_str(self.0).map(Some)
    }
}

macro_rules! adapter {
    ($name:ident, $option:ident, $ty:ty, $text:ident, $parse:ident, $expecting:literal, $doc:literal) => {
        #[doc = $doc]
        pub mod $name {
            use super::*;

            /// Writes the value as chrono's serde did.
            pub fn serialize<S: Serializer>(value: &$ty, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&$text(*value))
            }

            /// Reads what chrono's `Deserialize` accepted.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<$ty, D::Error> {
                d.deserialize_str(Lenient {
                    parse: $parse,
                    expecting: $expecting,
                })
            }
        }

        #[doc = $doc]
        ///
        /// The `Option` twin: `None` is `null`. The field must carry
        /// `#[serde(default)]` so a missing key loads as `None`.
        pub mod $option {
            use super::*;

            /// Writes the value as chrono's serde did, `None` as `null`.
            pub fn serialize<S: Serializer>(value: &Option<$ty>, s: S) -> Result<S::Ok, S::Error> {
                match value {
                    Some(value) => s.serialize_str(&$text(*value)),
                    None => s.serialize_none(),
                }
            }

            /// Reads what chrono's `Deserialize` accepted, `null` as `None`.
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$ty>, D::Error> {
                d.deserialize_option(Maybe(Lenient {
                    parse: $parse,
                    expecting: $expecting,
                }))
            }
        }
    };
}

adapter!(
    stamp,
    stamp_option,
    Stamp,
    stamp_text,
    parse_lenient_stamp,
    "a UTC date and time",
    "A [`Stamp`] as an RFC 3339 string ending in `Z`."
);
adapter!(
    offset_stamp,
    offset_stamp_option,
    OffsetStamp,
    offset_stamp_text,
    parse_lenient_offset,
    "a date and time with an offset",
    "An [`OffsetStamp`] as an RFC 3339 string: `Z` for offset zero, else `+hh:mm`."
);
adapter!(
    day,
    day_option,
    Day,
    day_text,
    parse_lenient_date,
    "a date",
    "A [`Day`] as `YYYY-MM-DD`."
);

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde::{Deserialize, Serialize};
    use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

    #[derive(Serialize, Deserialize)]
    struct Ours {
        #[serde(with = "stamp")]
        at: Stamp,
        #[serde(with = "offset_stamp")]
        written: OffsetStamp,
        #[serde(with = "day")]
        on: Day,
        #[serde(default, with = "stamp_option")]
        maybe_at: Option<Stamp>,
        #[serde(default, with = "offset_stamp_option")]
        maybe_written: Option<OffsetStamp>,
        #[serde(default, with = "day_option")]
        maybe_on: Option<Day>,
    }

    #[derive(Serialize, Deserialize)]
    struct Theirs {
        at: DateTime<Utc>,
        written: DateTime<FixedOffset>,
        on: NaiveDate,
        maybe_at: Option<DateTime<Utc>>,
        maybe_written: Option<DateTime<FixedOffset>>,
        maybe_on: Option<NaiveDate>,
    }

    const SECS: [i64; 6] = [
        1_767_608_239,
        0,
        -1,
        253_402_300_799,
        -62_167_219_200,
        // Leap year day at noon.
        1_709_208_000,
    ];
    const NANOS: [u32; 6] = [0, 500_000_000, 120_000_000, 123_450_000, 123_456_789, 1];
    const OFFSETS: [i32; 5] = [
        0,
        7200,
        -(4 * 3600 + 30 * 60),
        5 * 3600 + 45 * 60,
        -23 * 3600,
    ];

    fn pairs() -> Vec<(Ours, Theirs)> {
        let mut out = Vec::new();
        for secs in SECS {
            for nanos in NANOS {
                for offset in OFFSETS {
                    let utc = DateTime::from_timestamp(secs, nanos).unwrap();
                    let zone = FixedOffset::east_opt(offset).unwrap();
                    let written = utc.with_timezone(&zone);
                    let on = utc.date_naive();
                    let present = nanos != 1;
                    out.push((
                        Ours {
                            at: Stamp::from_chrono(utc),
                            written: OffsetStamp::from_chrono(written),
                            on: Day::from_chrono(on),
                            maybe_at: present.then(|| Stamp::from_chrono(utc)),
                            maybe_written: present.then(|| OffsetStamp::from_chrono(written)),
                            maybe_on: present.then(|| Day::from_chrono(on)),
                        },
                        Theirs {
                            at: utc,
                            written,
                            on,
                            maybe_at: present.then_some(utc),
                            maybe_written: present.then_some(written),
                            maybe_on: present.then_some(on),
                        },
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn adapters_round_trip_like_chrono() {
        for (ours, theirs) in pairs() {
            let json = serde_json::to_string(&theirs).unwrap();
            assert_eq!(serde_json::to_string(&ours).unwrap(), json);
            let yaml = serde_yaml_ng::to_string(&theirs).unwrap();
            assert_eq!(serde_yaml_ng::to_string(&ours).unwrap(), yaml);

            let back: Ours = serde_json::from_str(&json).unwrap();
            assert_eq!(back.at, ours.at);
            assert_eq!(back.written, ours.written);
            assert_eq!(back.written.offset_seconds(), ours.written.offset_seconds());
            assert_eq!(back.on, ours.on);
            assert_eq!(back.maybe_at, ours.maybe_at);
            assert_eq!(back.maybe_written, ours.maybe_written);
            assert_eq!(back.maybe_on, ours.maybe_on);
            let back: Ours = serde_yaml_ng::from_str(&yaml).unwrap();
            assert_eq!(back.at, ours.at);
            assert_eq!(back.maybe_on, ours.maybe_on);
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn written_bytes_are_the_documented_ones() {
        let at = Stamp::from_unix(1_767_608_239, 120_000_000).unwrap();
        assert_eq!(stamp_json(at), Value::from("2026-01-05T10:17:19.120Z"));
        let zone = chrono::FixedOffset::east_opt(7200).unwrap();
        let plus_two = OffsetStamp::from_chrono(at.into_chrono().with_timezone(&zone));
        assert_eq!(
            offset_stamp_json(plus_two),
            Value::from("2026-01-05T12:17:19.120+02:00")
        );
        assert_eq!(
            offset_stamp_json(at.to_offset_utc()),
            Value::from("2026-01-05T10:17:19.120Z")
        );
        assert_eq!(day_json(at.day_utc()), Value::from("2026-01-05"));
    }

    #[test]
    fn json_helpers_match_chrono_serde() {
        for (ours, theirs) in pairs() {
            assert_eq!(
                stamp_json(ours.at),
                serde_json::to_value(theirs.at).unwrap()
            );
            assert_eq!(
                offset_stamp_json(ours.written),
                serde_json::to_value(theirs.written).unwrap()
            );
            assert_eq!(day_json(ours.on), serde_json::to_value(theirs.on).unwrap());
        }
    }

    #[test]
    fn j3_inputs_are_accepted_and_refused_like_chrono() {
        let inputs = [
            "2026-01-05T10:17:19Z",
            "2026-1-5T1:7:9Z",
            " 2026-01-05T10:17:19Z ",
            "2026-01-05T10:17:19 +0200",
            "2026-01-05T10:17:19+02:00",
            "2026-01-05T10:17:19 UTC",
            "2026-01-05T10:17:19.1234567891Z",
            "2026-01-05T10:17:60Z",
            "2026-01-05T25:17:19Z",
            "+2026-01-05T10:17:19Z",
            "2026 - 01 - 05",
            "2026-01-05",
            "2026-02-30",
            "",
            "not a date",
        ];
        for input in inputs {
            let json = serde_json::to_string(input).unwrap();
            let ours: Result<Ours, _> =
                serde_json::from_str(&format!(r#"{{"at":{json},"written":{json},"on":{json}}}"#));
            let theirs: Result<Theirs, _> =
                serde_json::from_str(&format!(r#"{{"at":{json},"written":{json},"on":{json}}}"#));
            // The struct needs all three fields to parse, so compare each
            // field on its own as well.
            assert_eq!(ours.is_ok(), theirs.is_ok(), "{input:?}");
            let one = |field: &str| {
                let body = format!(
                    r#"{{"at":"2026-01-05T10:17:19Z","written":"2026-01-05T10:17:19Z","on":"2026-01-05","{field}":{json}}}"#
                );
                (
                    serde_json::from_str::<Ours>(&body).map(|o| match field {
                        "at" => o.at.rfc3339(),
                        "written" => o.written.rfc3339(),
                        _ => o.on.ymd(),
                    }),
                    serde_json::from_str::<Theirs>(&body).map(|t| match field {
                        "at" => t.at.to_rfc3339(),
                        "written" => t.written.to_rfc3339(),
                        _ => t.on.format("%Y-%m-%d").to_string(),
                    }),
                )
            };
            for field in ["at", "written", "on"] {
                let (ours, theirs) = one(field);
                assert_eq!(ours.ok(), theirs.ok(), "{field} {input:?}");
            }
        }
    }

    #[test]
    fn non_strings_are_refused() {
        for body in [
            r#"{"at":5,"written":"2026-01-05T10:17:19Z","on":"2026-01-05"}"#,
            r#"{"at":null,"written":"2026-01-05T10:17:19Z","on":"2026-01-05"}"#,
            r#"{"at":"2026-01-05T10:17:19Z","written":[],"on":"2026-01-05"}"#,
        ] {
            assert!(serde_json::from_str::<Ours>(body).is_err(), "{body}");
            assert!(serde_json::from_str::<Theirs>(body).is_err(), "{body}");
        }
        let head =
            r#""at":"2026-01-05T10:17:19Z","written":"2026-01-05T10:17:19Z","on":"2026-01-05""#;
        for field in ["maybe_at", "maybe_written", "maybe_on"] {
            for value in [
                "5",
                r#""bad""#,
                "[]",
                r#""2026-01-05T10:17:19Z""#,
                r#""2026-01-05""#,
            ] {
                let body = format!("{{{head},\"{field}\":{value}}}");
                assert_eq!(
                    serde_json::from_str::<Ours>(&body).is_ok(),
                    serde_json::from_str::<Theirs>(&body).is_ok(),
                    "{body}"
                );
            }
        }
        let yaml_head = "at: 2026-01-05T10:17:19Z\nwritten: 2026-01-05T10:17:19Z\non: 2026-01-05\n";
        for field in [
            "at",
            "written",
            "on",
            "maybe_at",
            "maybe_written",
            "maybe_on",
        ] {
            for value in ["5", "[1]", "bad", "{a: 1}", "~"] {
                let body = format!("{yaml_head}{field}: {value}\n");
                assert_eq!(
                    serde_yaml_ng::from_str::<Ours>(&body).is_ok(),
                    serde_yaml_ng::from_str::<Theirs>(&body).is_ok(),
                    "{body}"
                );
            }
        }
    }

    #[test]
    fn an_option_adapter_without_default_refuses_a_missing_key() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Bare {
            #[serde(with = "stamp_option")]
            at: Option<Stamp>,
        }
        assert!(serde_json::from_str::<Bare>("{}").is_err());
        assert!(serde_json::from_str::<Bare>(r#"{"at":null}"#).is_ok());
    }

    #[test]
    fn missing_optional_key_loads() {
        let body =
            r#"{"at":"2026-01-05T10:17:19Z","written":"2026-01-05T10:17:19Z","on":"2026-01-05"}"#;
        let loaded: Ours = serde_json::from_str(body).unwrap();
        assert!(loaded.maybe_at.is_none());
        assert!(loaded.maybe_written.is_none());
        assert!(loaded.maybe_on.is_none());
        let yaml = "at: 2026-01-05T10:17:19Z\nwritten: 2026-01-05T10:17:19Z\non: 2026-01-05\n";
        let loaded: Ours = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(loaded.maybe_at.is_none());
        assert!(loaded.maybe_written.is_none() && loaded.maybe_on.is_none());
        for null in ["~", "null"] {
            let body = format!("{yaml}maybe_at: {null}\nmaybe_written: {null}\nmaybe_on: {null}\n");
            let loaded: Ours = serde_yaml_ng::from_str(&body).unwrap();
            assert!(loaded.maybe_at.is_none() && loaded.maybe_written.is_none());
            assert!(loaded.maybe_on.is_none());
        }
        // A written null and a missing key mean the same.
        let nulls = r#"{"at":"2026-01-05T10:17:19Z","written":"2026-01-05T10:17:19Z","on":"2026-01-05","maybe_at":null,"maybe_written":null,"maybe_on":null}"#;
        let loaded: Ours = serde_json::from_str(nulls).unwrap();
        assert!(loaded.maybe_at.is_none() && loaded.maybe_on.is_none());
        assert!(
            serde_json::to_string(&loaded)
                .unwrap()
                .contains(r#""maybe_at":null"#)
        );
    }
}
