//! What a domain's Reflection is, seen from one instance. Plain data: the
//! engine decides it, every surface reports it.

use serde::Serialize;

/// Where a Reflection that is in effect lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionHome {
    /// A domain connected to GitHub whose `reflection_home` is this instance.
    Here,
    /// A domain with no GitHub origin: its Reflection is local to whoever holds it.
    Local,
}

/// What a domain's Reflection is, as seen from this instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectionState {
    /// `reflection` is off (or the domain is virtual).
    Off,
    /// Switched on while engrams still live in `reflections/`: these paths.
    Pending {
        /// The domain-relative paths of the engrams that wait to move out.
        engrams: Vec<String>,
    },
    /// Switched on for a GitHub-connected domain with no `reflection_home`,
    /// or for a team domain (its MANIFEST carries the `team` marker) that
    /// is not connected on this instance: either way its home, the team
    /// server, is not here and has not been filled in.
    Homeless,
    /// Switched on, and its home is another instance.
    Elsewhere {
        /// The home instance's address.
        url: String,
    },
    /// Switched on, and another instance on this shared database holds the
    /// domain's host lock, so that instance serves it (R92).
    /// `label` is `None` while a rename pauses a domain whose lock nobody
    /// holds: no instance serves it until the rename ends and claims it.
    HostedElsewhere {
        /// The host's label (its `DomainHost.label`).
        label: Option<String>,
    },
    /// In effect here.
    On {
        /// Where it lives.
        home: ReflectionHome,
    },
}

impl ReflectionState {
    /// The wire word (`GET /domains/{domain}/reflection`'s `state`).
    pub fn word(&self) -> &'static str {
        match self {
            ReflectionState::Off => "off",
            ReflectionState::Pending { .. } => "pending",
            ReflectionState::Homeless => "homeless",
            ReflectionState::Elsewhere { .. } => "elsewhere",
            ReflectionState::HostedElsewhere { .. } => "hosted_elsewhere",
            ReflectionState::On { .. } => "on",
        }
    }
}

impl ReflectionHome {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            ReflectionHome::Here => "here",
            ReflectionHome::Local => "local",
        }
    }
}
