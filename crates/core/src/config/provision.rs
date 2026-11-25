//! One domain's provisioning decision on this machine: the value of a domain
//! entry's `provision` key in `config.yaml`.
//!
//! The key takes four shapes: `true`, `false`, a map with `except` and
//! `also`, or a map with `only`. `true` and `false` keep the bytes they always
//! had, so a configuration that never used a pick stays byte-identical and an
//! older binary still reads it. A map is new: an older binary cannot read it.
//!
//! Serde is written by hand on both sides. The reader is a visitor with a
//! boolean and a map arm, so a wrong map gets a sentence that names the
//! problem (an untagged enum would only say that nothing matched), and the
//! deserializer still puts the key's path (`domains.harbor.provision`) in
//! front of it. The same impls serve YAML (`config.yaml`) and JSON (the
//! configuration document).

use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The `except` key of a `provision` map.
const EXCEPT: &str = "except";
/// The `also` key of a `provision` map.
const ALSO: &str = "also";
/// The `only` key of a `provision` map.
const ONLY: &str = "only";

/// One domain's provisioning decision on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provision {
    /// `false`: nothing of this domain is installed.
    Deny,
    /// `true`, or `{except, also}`: everything the domain ships, except the
    /// `except` names, and optional artifacts only when named in `also`.
    /// New artifacts the domain ships later are installed.
    All {
        /// Artifact keys that are left out.
        except: Vec<String>,
        /// Optional artifact keys that are taken.
        also: Vec<String>,
    },
    /// `{only: [...]}`: only these names. New artifacts are not installed.
    Only(Vec<String>),
}

impl From<bool> for Provision {
    /// `true` is plain [`Provision::all`], `false` is [`Provision::Deny`].
    fn from(allow: bool) -> Provision {
        if allow {
            Provision::all()
        } else {
            Provision::Deny
        }
    }
}

impl Provision {
    /// Plain `true`: everything the domain ships except optional artifacts.
    pub const fn all() -> Provision {
        Provision::All {
            except: Vec::new(),
            also: Vec::new(),
        }
    }

    /// Whether this decision installs anything at all: every shape but
    /// [`Provision::Deny`]. An `Only` list is never empty after
    /// [`Provision::normalized`].
    pub fn includes_any(&self) -> bool {
        !matches!(self, Provision::Deny)
    }

    /// Whether this decision installs the artifact `key` (`<kind>/<name>`),
    /// where `optional` says whether the domain marks it optional.
    ///
    /// - `Deny` selects nothing.
    /// - `All` leaves out every key in `except`, and an optional artifact is
    ///   taken only when its key is in `also`. `except` wins when a hand edit
    ///   puts one key in both lists.
    /// - `Only` selects exactly its keys; `optional` changes nothing.
    ///
    /// Keys compare exactly: the comparison is case-sensitive, so
    /// `skills/Tide` does not select `skills/tide`.
    pub fn selects(&self, key: &str, optional: bool) -> bool {
        let named = |list: &[String]| list.iter().any(|n| n == key);
        match self {
            Provision::Deny => false,
            Provision::All { except, also } => {
                if named(except) {
                    false
                } else if optional {
                    named(also)
                } else {
                    true
                }
            }
            Provision::Only(only) => named(only),
        }
    }

    /// Every name in every list of this decision, for the stale and malformed
    /// checks. `except` before `also`.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let (first, second): (&[String], &[String]) = match self {
            Provision::Deny => (&[], &[]),
            Provision::All { except, also } => (except, also),
            Provision::Only(only) => (only, &[]),
        };
        first.iter().chain(second).map(String::as_str)
    }

    /// This decision with each list sorted and deduplicated. `All` with both
    /// lists empty stays `All` (it writes `true`); `Only` with an empty list
    /// becomes `Deny`, since it installs nothing.
    pub fn normalized(self) -> Provision {
        fn tidy(mut list: Vec<String>) -> Vec<String> {
            list.sort();
            list.dedup();
            list
        }
        match self {
            Provision::Deny => Provision::Deny,
            Provision::All { except, also } => Provision::All {
                except: tidy(except),
                also: tidy(also),
            },
            Provision::Only(only) if only.is_empty() => Provision::Deny,
            Provision::Only(only) => Provision::Only(tidy(only)),
        }
    }

    /// A short canonical text for this decision, for the session prefilter
    /// to notice a changed pick. `None` for plain `true`, so a receipt written
    /// before picks existed still matches. Otherwise for example
    /// `except=a,b;also=c`, `only=a,b` or `deny`. Computed on the normalized
    /// form, so list order and repeats do not change it.
    pub fn fingerprint(&self) -> Option<String> {
        match self.clone().normalized() {
            Provision::Deny => Some("deny".to_string()),
            Provision::All { except, also } => {
                let mut parts = Vec::new();
                if !except.is_empty() {
                    parts.push(format!("{EXCEPT}={}", except.join(",")));
                }
                if !also.is_empty() {
                    parts.push(format!("{ALSO}={}", also.join(",")));
                }
                (!parts.is_empty()).then(|| parts.join(";"))
            }
            Provision::Only(only) => Some(format!("{ONLY}={}", only.join(","))),
        }
    }
}

impl Serialize for Provision {
    /// Plain `true` for `All` with empty lists and `false` for `Deny`, so a
    /// configuration without a pick keeps its bytes. A pick writes a map with
    /// only its non-empty lists, `except` before `also`, or `{only: [...]}`.
    /// Lists are written sorted and deduplicated.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.clone().normalized() {
            Provision::Deny => serializer.serialize_bool(false),
            Provision::All { except, also } if except.is_empty() && also.is_empty() => {
                serializer.serialize_bool(true)
            }
            Provision::All { except, also } => {
                let len = usize::from(!except.is_empty()) + usize::from(!also.is_empty());
                let mut map = serializer.serialize_map(Some(len))?;
                if !except.is_empty() {
                    map.serialize_entry(EXCEPT, &except)?;
                }
                if !also.is_empty() {
                    map.serialize_entry(ALSO, &also)?;
                }
                map.end()
            }
            Provision::Only(only) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(ONLY, &only)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Provision {
    /// Reads `true`, `false` or a map of `except`, `also` and `only`. A map
    /// with none of them, a map with `only` beside `except` or `also`, an
    /// unknown key (a typo such as `exept`) and a repeated key fail the load,
    /// so a pick is never silently read as "everything". The result is
    /// [`Provision::normalized`]. Names are not checked for shape here: a
    /// malformed name loads, and status and doctor report it.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Provision, D::Error> {
        deserializer.deserialize_any(ProvisionVisitor)
    }
}

/// The visitor behind [`Provision`]'s `Deserialize`: a boolean or a map.
struct ProvisionVisitor;

impl<'de> Visitor<'de> for ProvisionVisitor {
    type Value = Provision;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`true`, `false`, or a map with `except`, `also` or `only`")
    }

    fn visit_bool<E: de::Error>(self, allow: bool) -> Result<Provision, E> {
        Ok(Provision::from(allow))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Provision, A::Error> {
        let mut except: Option<Vec<String>> = None;
        let mut also: Option<Vec<String>> = None;
        let mut only: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            let slot = match key.as_str() {
                EXCEPT => &mut except,
                ALSO => &mut also,
                ONLY => &mut only,
                other => {
                    return Err(de::Error::custom(format!(
                        "`provision` has no key `{other}`; it takes `except`, `also` or `only`"
                    )));
                }
            };
            if slot.is_some() {
                return Err(de::Error::custom(format!(
                    "`provision` names `{key}` twice"
                )));
            }
            *slot = Some(map.next_value()?);
        }
        match (except, also, only) {
            (None, None, None) => Err(de::Error::custom(
                "`provision` needs `except`, `also` or `only`, or `true` or `false`",
            )),
            (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => Err(de::Error::custom(
                "`provision` takes `only`, or `except` and `also`, never both",
            )),
            (None, None, Some(only)) => Ok(Provision::Only(only).normalized()),
            (except, also, None) => Ok(Provision::All {
                except: except.unwrap_or_default(),
                also: also.unwrap_or_default(),
            }
            .normalized()),
        }
    }
}
