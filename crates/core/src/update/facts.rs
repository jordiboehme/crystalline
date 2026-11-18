//! What a daemon knows about updating, as its ctl `update_facts` answer
//! carries it: the install method it detected at start, whether this is an
//! official build, whether the passive check is on, and its last check in
//! the shape that may leave the daemon ([`UpdateCache::answered`]).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::cache::UpdateCache;
use super::install::{Detection, InstallMethod, Refusal};

/// Why the passive check is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffReason {
    /// `update.check` is not on (the default).
    NotTurnedOn,
    /// `NO_UPDATE_NOTIFIER` is set in the daemon's environment.
    NoUpdateNotifier,
    /// `CI` is set in the daemon's environment.
    Ci,
    /// A Docker install: pull the new image.
    Docker,
    /// The Claude Desktop-only bundle: the newer .mcpb is opened by hand.
    DesktopExtension,
    /// There is no state folder to cache a check in, so none could run.
    NoStateFolder,
}

impl OffReason {
    const ALL: [OffReason; 6] = [
        OffReason::NotTurnedOn,
        OffReason::NoUpdateNotifier,
        OffReason::Ci,
        OffReason::Docker,
        OffReason::DesktopExtension,
        OffReason::NoStateFolder,
    ];

    /// The reason as a short fixed sentence.
    pub fn text(&self) -> &'static str {
        match self {
            OffReason::NotTurnedOn => "not turned on",
            OffReason::NoUpdateNotifier => "NO_UPDATE_NOTIFIER is set",
            OffReason::Ci => "CI is set",
            OffReason::Docker => "Docker",
            OffReason::DesktopExtension => "Claude Desktop extension",
            OffReason::NoStateFolder => "no state folder",
        }
    }

    /// Why the passive check must not run whatever the setting says, or
    /// `None`: `NO_UPDATE_NOTIFIER` or `CI` set to anything but an empty
    /// value, a Docker install or the Claude Desktop extension. The method
    /// is asked, not the refusal, since an unofficial build replaces the
    /// refusal and keeps the method. The setting and the state folder are
    /// the caller's to add.
    pub fn suppressing(
        env: &dyn Fn(&str) -> Option<String>,
        detection: &Detection,
    ) -> Option<OffReason> {
        let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
        if set("NO_UPDATE_NOTIFIER") {
            Some(OffReason::NoUpdateNotifier)
        } else if set("CI") {
            Some(OffReason::Ci)
        } else if detection.method == InstallMethod::Docker {
            Some(OffReason::Docker)
        } else if detection.method == InstallMethod::Mcpb {
            Some(OffReason::DesktopExtension)
        } else {
            None
        }
    }
}

/// Whether the passive check is on, written as `on` or `off: <reason>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    On,
    Off(OffReason),
}

impl CheckState {
    /// The wire text: `on` or `off: <reason>`.
    pub fn text(&self) -> String {
        match self {
            CheckState::On => "on".to_string(),
            CheckState::Off(reason) => format!("off: {}", reason.text()),
        }
    }

    /// The state the wire text names; `None` for any other text.
    pub fn parse(text: &str) -> Option<CheckState> {
        if text == "on" {
            return Some(CheckState::On);
        }
        let reason = text.strip_prefix("off: ")?;
        OffReason::ALL
            .into_iter()
            .find(|known| known.text() == reason)
            .map(CheckState::Off)
    }
}

impl Serialize for CheckState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text())
    }
}

impl<'de> Deserialize<'de> for CheckState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        CheckState::parse(&text)
            .ok_or_else(|| serde::de::Error::custom("not an update check state"))
    }
}

/// The ctl `update_facts` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateFacts {
    /// The install method the daemon detected at start (S18).
    pub method: InstallMethod,
    /// Whether the daemon is an official release build.
    pub official: bool,
    /// Why `update now` would not act for the daemon's binary, if it would
    /// not.
    #[serde(default)]
    pub refusal: Option<Refusal>,
    /// Whether the passive check is on, and why not.
    pub check: CheckState,
    /// The last check, already [`UpdateCache::answered`]; `None` before
    /// any.
    #[serde(default)]
    pub cache: Option<UpdateCache>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Stamp;
    use crate::update::{CheckOutcome, UPDATE_CACHE_FORMAT};

    #[test]
    fn the_check_state_round_trips_and_refuses_other_text() {
        assert_eq!(CheckState::parse("on"), Some(CheckState::On));
        for reason in OffReason::ALL {
            let state = CheckState::Off(reason);
            assert_eq!(CheckState::parse(&state.text()), Some(state));
        }
        for other in ["", "On", "off", "off: ", "off: maybe", "off:not turned on"] {
            assert_eq!(CheckState::parse(other), None, "{other}");
        }
    }

    #[test]
    fn the_facts_round_trip_with_stable_keys() {
        let facts = UpdateFacts {
            method: InstallMethod::Tarball,
            official: true,
            refusal: Some(Refusal::MarkerPathMismatch {
                marker: InstallMethod::Deb,
                path: InstallMethod::Tarball,
            }),
            check: CheckState::Off(OffReason::NotTurnedOn),
            cache: Some(UpdateCache {
                v: UPDATE_CACHE_FORMAT,
                checked_at: Stamp::from_ymd_hms(2026, 11, 18, 9, 0, 0).unwrap(),
                outcome: CheckOutcome::Failed {
                    reason: "the check failed".to_string(),
                },
                etag: None,
                last_release: None,
            }),
        };
        let value = serde_json::to_value(&facts).unwrap();
        assert_eq!(value["method"], "tarball");
        assert_eq!(value["check"], "off: not turned on");
        assert_eq!(
            value["refusal"],
            serde_json::json!({"marker_path_mismatch": {"marker": "deb", "path": "tarball"}})
        );
        let back: UpdateFacts = serde_json::from_value(value).unwrap();
        assert_eq!(back, facts);
        for (method, key) in [
            (InstallMethod::Homebrew, "homebrew"),
            (InstallMethod::Msi, "msi"),
            (InstallMethod::Mcpb, "mcpb"),
            (InstallMethod::Unknown, "unknown"),
        ] {
            assert_eq!(serde_json::to_value(method).unwrap(), key);
        }
        assert_eq!(
            serde_json::to_value(Refusal::NotOfficial).unwrap(),
            "not_official"
        );
    }
}
