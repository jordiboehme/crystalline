//! Serde adapters for `#[serde(with = "...")]`: the only way a date goes into
//! or out of a serialized struct, because the newtypes implement neither
//! `Serialize` nor `Deserialize`.
//!
//! Writing is what chrono's serde wrote: [`stamp`] gives `Z` and a fraction
//! of none, 3, 6 or 9 digits (`2026-01-05T10:17:19.120Z`), [`offset_stamp`]
//! gives `Z` for offset zero and `+hh:mm` otherwise, and [`day`] gives
//! `%Y-%m-%d`. Reading is what chrono's `Deserialize` accepted, through
//! [`parse_lenient_stamp`], [`parse_lenient_offset`] and
//! [`parse_lenient_date`].
//!
//! One edge of the year range (J4) does not survive a round trip: a year
//! is judged as written, so `0000-01-01T00:30:00+01:00` is read, but its
//! instant lies in the year -0001, which [`stamp`] writes and the next
//! load refuses. The same holds for any [`Stamp`] before the year 0000.
//! No stored date is that early.
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

/// A map from names to [`Stamp`]s (`BTreeMap<String, Stamp>`), each value
/// written and read as [`stamp`] does. A missing key needs
/// `#[serde(default)]` on the field to load as an empty map, as for the
/// `_option` adapters.
pub mod stamp_map {
    use super::*;
    use ::serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    /// One value of the map, read through [`stamp`](super::stamp).
    struct Value(Stamp);

    impl<'de> Deserialize<'de> for Value {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
            super::stamp::deserialize(d).map(Value)
        }
    }

    /// Writes every value as chrono's serde did.
    pub fn serialize<S: Serializer>(
        value: &BTreeMap<String, Stamp>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        let texts: BTreeMap<&String, String> =
            value.iter().map(|(k, v)| (k, stamp_text(*v))).collect();
        texts.serialize(s)
    }

    /// Reads every value as chrono's `Deserialize` did.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, Stamp>, D::Error> {
        let values = BTreeMap::<String, Value>::deserialize(d)?;
        Ok(values.into_iter().map(|(k, v)| (k, v.0)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

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

    /// The same fields as plain strings: what [`Ours`] must serialize as.
    #[derive(Serialize)]
    struct Texts {
        at: String,
        written: String,
        on: String,
        maybe_at: Option<String>,
        maybe_written: Option<String>,
        maybe_on: Option<String>,
    }

    /// (seconds, UTC date and time, its local form at each nonzero offset
    /// of [`OFFSETS`]), the bytes chrono's serde wrote, without the
    /// fraction and the offset.
    const SECS: [(i64, &str, [&str; 4]); 6] = [
        (
            1_767_608_239,
            "2026-01-05T10:17:19",
            [
                "2026-01-05T12:17:19",
                "2026-01-05T05:47:19",
                "2026-01-05T16:02:19",
                "2026-01-04T11:17:19",
            ],
        ),
        (
            0,
            "1970-01-01T00:00:00",
            [
                "1970-01-01T02:00:00",
                "1969-12-31T19:30:00",
                "1970-01-01T05:45:00",
                "1969-12-31T01:00:00",
            ],
        ),
        (
            -1,
            "1969-12-31T23:59:59",
            [
                "1970-01-01T01:59:59",
                "1969-12-31T19:29:59",
                "1970-01-01T05:44:59",
                "1969-12-31T00:59:59",
            ],
        ),
        // jiff's latest whole second (J4: 9999-12-31T23:59:59Z is past it).
        (
            253_402_207_200,
            "9999-12-30T22:00:00",
            [
                "9999-12-31T00:00:00",
                "9999-12-30T17:30:00",
                "9999-12-31T03:45:00",
                "9999-12-29T23:00:00",
            ],
        ),
        // 0000-01-02: a day later than the year's first, so every offset
        // still writes the year 0000 (J4 refuses to read a year before it).
        (
            -62_167_132_800,
            "0000-01-02T00:00:00",
            [
                "0000-01-02T02:00:00",
                "0000-01-01T19:30:00",
                "0000-01-02T05:45:00",
                "0000-01-01T01:00:00",
            ],
        ),
        // Leap year day at noon.
        (
            1_709_208_000,
            "2024-02-29T12:00:00",
            [
                "2024-02-29T14:00:00",
                "2024-02-29T07:30:00",
                "2024-02-29T17:45:00",
                "2024-02-28T13:00:00",
            ],
        ),
    ];

    /// (nanoseconds, the fraction written for them: none, 3, 6 or 9 digits).
    const NANOS: [(u32, &str); 6] = [
        (0, ""),
        (500_000_000, ".500"),
        (120_000_000, ".120"),
        (123_450_000, ".123450"),
        (123_456_789, ".123456789"),
        (1, ".000000001"),
    ];

    /// (offset seconds, the offset written for it).
    const OFFSETS: [(i32, &str); 5] = [
        (0, "Z"),
        (7200, "+02:00"),
        (-(4 * 3600 + 30 * 60), "-04:30"),
        (5 * 3600 + 45 * 60, "+05:45"),
        (-23 * 3600, "-23:00"),
    ];

    /// Every value of the matrix with the strings it must be written as.
    fn cases() -> Vec<(Ours, Texts)> {
        let mut out = Vec::new();
        for (secs, utc, locals) in SECS {
            for (nanos, fraction) in NANOS {
                for (i, (offset, offset_text)) in OFFSETS.into_iter().enumerate() {
                    let at = Stamp::from_unix(secs, nanos).unwrap();
                    let written = OffsetStamp::at_offset(at, offset).unwrap();
                    let present = nanos != 1;
                    let at_text = format!("{utc}{fraction}Z");
                    let local = if i == 0 { utc } else { locals[i - 1] };
                    let written_text = format!("{local}{fraction}{offset_text}");
                    let on_text = utc[..10].to_string();
                    out.push((
                        Ours {
                            at,
                            written,
                            on: at.day_utc(),
                            maybe_at: present.then_some(at),
                            maybe_written: present.then_some(written),
                            maybe_on: present.then(|| at.day_utc()),
                        },
                        Texts {
                            maybe_at: present.then(|| at_text.clone()),
                            maybe_written: present.then(|| written_text.clone()),
                            maybe_on: present.then(|| on_text.clone()),
                            at: at_text,
                            written: written_text,
                            on: on_text,
                        },
                    ));
                }
            }
        }
        out
    }

    #[test]
    fn adapters_write_the_frozen_bytes_and_read_them_back() {
        for (ours, texts) in cases() {
            let json = serde_json::to_string(&texts).unwrap();
            assert_eq!(serde_json::to_string(&ours).unwrap(), json);
            let yaml = serde_yaml_ng::to_string(&texts).unwrap();
            assert_eq!(serde_yaml_ng::to_string(&ours).unwrap(), yaml);
            assert_eq!(stamp_json(ours.at), Value::from(texts.at.as_str()));
            assert_eq!(
                offset_stamp_json(ours.written),
                Value::from(texts.written.as_str())
            );
            assert_eq!(day_json(ours.on), Value::from(texts.on.as_str()));

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
        let plus_two = OffsetStamp::at_offset(at, 7200).unwrap();
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

    /// A written year 0000 just after midnight at a positive offset is read
    /// (J4 judges the year as written), but its UTC instant lies in the
    /// year -0001, so the `stamp` adapter writes a year the next load
    /// refuses. The offset adapter keeps the written offset and reads back.
    #[test]
    fn a_stamp_early_in_year_0000_is_read_but_its_written_form_is_refused() {
        let input = "0000-01-01T00:30:00+01:00";
        let at = parse_lenient_stamp(input).expect("a written year 0000 is read");
        assert_eq!(stamp_json(at), Value::from("-0001-12-31T23:30:00Z"));
        let reload = serde_json::from_str::<UtcField>(r#"{"at":"-0001-12-31T23:30:00Z"}"#);
        assert!(reload.is_err());

        let written = parse_lenient_offset(input).unwrap();
        let text = offset_stamp_json(written);
        assert_eq!(text, Value::from(input));
        let reload: OffsetField =
            serde_json::from_value(serde_json::json!({ "at": text })).unwrap();
        assert_eq!(reload.at, written);
    }

    #[derive(Deserialize)]
    struct UtcField {
        #[serde(with = "stamp")]
        #[allow(dead_code)]
        at: Stamp,
    }

    #[derive(Deserialize)]
    struct OffsetField {
        #[serde(with = "offset_stamp")]
        at: OffsetStamp,
    }

    #[test]
    fn j3_inputs_are_accepted_and_refused_as_chrono_did() {
        // (input, `at` read, `written` read, `on` read), each in RFC 3339
        // (`%Y-%m-%d` for `on`); `None` is a refusal.
        let inputs = [
            (
                "2026-01-05T10:17:19Z",
                Some("2026-01-05T10:17:19+00:00"),
                Some("2026-01-05T10:17:19+00:00"),
                None,
            ),
            (
                "2026-1-5T1:7:9Z",
                Some("2026-01-05T01:07:09+00:00"),
                Some("2026-01-05T01:07:09+00:00"),
                None,
            ),
            (
                " 2026-01-05T10:17:19Z ",
                Some("2026-01-05T10:17:19+00:00"),
                Some("2026-01-05T10:17:19+00:00"),
                None,
            ),
            (
                "2026-01-05T10:17:19 +0200",
                Some("2026-01-05T08:17:19+00:00"),
                Some("2026-01-05T10:17:19+02:00"),
                None,
            ),
            (
                "2026-01-05T10:17:19+02:00",
                Some("2026-01-05T08:17:19+00:00"),
                Some("2026-01-05T10:17:19+02:00"),
                None,
            ),
            (
                "2026-01-05T10:17:19 UTC",
                Some("2026-01-05T10:17:19+00:00"),
                Some("2026-01-05T10:17:19+00:00"),
                None,
            ),
            (
                "2026-01-05T10:17:19.1234567891Z",
                Some("2026-01-05T10:17:19.123456789+00:00"),
                Some("2026-01-05T10:17:19.123456789+00:00"),
                None,
            ),
            // J4: chrono kept a leap second, jiff reads it as `:59`.
            (
                "2026-01-05T10:17:60Z",
                Some("2026-01-05T10:17:59+00:00"),
                Some("2026-01-05T10:17:59+00:00"),
                None,
            ),
            ("2026-01-05T25:17:19Z", None, None, None),
            (
                "+2026-01-05T10:17:19Z",
                Some("2026-01-05T10:17:19+00:00"),
                Some("2026-01-05T10:17:19+00:00"),
                None,
            ),
            ("2026 - 01 - 05", None, None, Some("2026-01-05")),
            ("2026-01-05", None, None, Some("2026-01-05")),
            ("2026-02-30", None, None, None),
            ("", None, None, None),
            ("not a date", None, None, None),
        ];
        for (input, at, written, on) in inputs {
            let json = serde_json::to_string(input).unwrap();
            let whole: Result<Ours, _> =
                serde_json::from_str(&format!(r#"{{"at":{json},"written":{json},"on":{json}}}"#));
            // The struct needs all three fields to parse, so compare each
            // field on its own as well.
            assert_eq!(
                whole.is_ok(),
                at.is_some() && written.is_some() && on.is_some(),
                "{input:?}"
            );
            for (field, expected) in [("at", at), ("written", written), ("on", on)] {
                // The other two fields hold valid values; a key given twice
                // would be refused as a duplicate whatever its value.
                let value = |name: &str, valid: &str| {
                    if name == field {
                        json.clone()
                    } else {
                        format!("{valid:?}")
                    }
                };
                let body = format!(
                    r#"{{"at":{},"written":{},"on":{}}}"#,
                    value("at", "2026-01-05T10:17:19Z"),
                    value("written", "2026-01-05T10:17:19Z"),
                    value("on", "2026-01-05"),
                );
                let read = serde_json::from_str::<Ours>(&body)
                    .ok()
                    .map(|o| match field {
                        "at" => o.at.rfc3339(),
                        "written" => o.written.rfc3339(),
                        _ => o.on.ymd(),
                    });
                assert_eq!(read.as_deref(), expected, "{field} {input:?}");
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
                // Only a string of the field's own kind is read, as chrono
                // read it.
                let accepted = match field {
                    "maybe_on" => value == r#""2026-01-05""#,
                    _ => value == r#""2026-01-05T10:17:19Z""#,
                };
                assert_eq!(
                    serde_json::from_str::<Ours>(&body).is_ok(),
                    accepted,
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
                // Only `~` (null) in an `_option` field is read.
                let accepted = value == "~" && field.starts_with("maybe_");
                assert_eq!(
                    serde_yaml_ng::from_str::<Ours>(&body).is_ok(),
                    accepted,
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

    #[derive(Serialize, Deserialize)]
    struct OursMap {
        #[serde(default, with = "stamp_map")]
        nudges: BTreeMap<String, Stamp>,
    }

    #[test]
    fn stamp_map_writes_the_frozen_bytes() {
        let instants = [
            Stamp::from_unix(1_767_608_239, 120_000_000).unwrap(),
            Stamp::from_unix(-1, 999_999_999).unwrap(),
            Stamp::UNIX_EPOCH,
        ];
        // (entries, JSON, YAML) as chrono's serde wrote them.
        let maps = [
            (Vec::new(), r#"{"nudges":{}}"#, "nudges: {}\n"),
            (
                vec![("search", instants[0])],
                r#"{"nudges":{"search":"2026-01-05T10:17:19.120Z"}}"#,
                "nudges:\n  search: 2026-01-05T10:17:19.120Z\n",
            ),
            (
                vec![
                    ("tool-0", instants[0]),
                    ("tool-1", instants[1]),
                    ("tool-2", instants[2]),
                ],
                r#"{"nudges":{"tool-0":"2026-01-05T10:17:19.120Z","tool-1":"1969-12-31T23:59:59.999999999Z","tool-2":"1970-01-01T00:00:00Z"}}"#,
                "nudges:\n  tool-0: 2026-01-05T10:17:19.120Z\n  tool-1: 1969-12-31T23:59:59.999999999Z\n  tool-2: 1970-01-01T00:00:00Z\n",
            ),
        ];
        for (entries, json, yaml) in maps {
            let ours = OursMap {
                nudges: entries.iter().map(|(k, t)| (k.to_string(), *t)).collect(),
            };
            assert_eq!(serde_json::to_string(&ours).unwrap(), json);
            assert_eq!(serde_yaml_ng::to_string(&ours).unwrap(), yaml);
            let back: OursMap = serde_json::from_str(json).unwrap();
            assert_eq!(back.nudges, ours.nudges);
            let back: OursMap = serde_yaml_ng::from_str(yaml).unwrap();
            assert_eq!(back.nudges, ours.nudges);
        }
        let lenient = r#"{"nudges":{"a":" 2026-1-5T1:7:9 +0200 "}}"#;
        let ours: OursMap = serde_json::from_str(lenient).unwrap();
        assert_eq!(ours.nudges["a"].rfc3339(), "2026-01-04T23:07:09+00:00");
        let missing: OursMap = serde_json::from_str("{}").unwrap();
        assert!(missing.nudges.is_empty());
        let bad = r#"{"nudges":{"a":"2026-02-30T00:00:00Z"}}"#;
        assert!(serde_json::from_str::<OursMap>(bad).is_err());
    }
}
