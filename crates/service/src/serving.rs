//! What this process serves, recorded once by `run_serve` before it takes
//! the lock: how the daemon was started, what its HTTP endpoint is bound to
//! and which hosts it answers. The engine reads it to tell a caller on this
//! machine where the web pages are, so it lives below the runtime; the
//! `instance` module re-exports every item, and its paths are unchanged.

use serde::{Deserialize, Serialize};

/// How a daemon process came to be running.
///
/// **Recorded and reported, never branched on.** `attach_policy` keeps its one
/// version axis; this field exists so an operator and a probe can tell a
/// managed daemon from one an agent's `crystalline mcp` connection spawned,
/// which is what hid the 2026-09-10 outage for 277 restarts. Arbitrating
/// between two daemons on it was considered and rejected: part A removes the
/// reason they differ instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartMode {
    /// A `crystalline serve` a person, a unit file or a container entrypoint
    /// invoked.
    Serve,
    /// Spawned by a client that found no daemon; see `spawn_daemon`.
    Autostart,
}

impl StartMode {
    /// The wire spelling, for a message or a JSON body.
    pub fn as_str(self) -> &'static str {
        match self {
            StartMode::Serve => "serve",
            StartMode::Autostart => "autostart",
        }
    }
}

/// What a daemon bound its HTTP endpoint to.
///
/// Three shapes rather than an `Option<String>`, because "the endpoint is
/// deliberately closed" and "the holder did not record one" are different
/// facts and a refusal that conflates them tells an operator something untrue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpBinding {
    /// The holder recorded nothing here. True of a record written before the
    /// field existed, and equally of one published by a holder that never
    /// served (the `hold-lock` test command), so every message rendered from
    /// this says the holder *did not record it* and never names a version.
    #[default]
    Unrecorded,
    /// The endpoint is off (`service.http: false`, or `serve --http off`).
    Off,
    /// Bound at this `host:port`. Serialized as the bare address, so a person
    /// reading `service.json` sees the address rather than a wrapper.
    #[serde(untagged)]
    Bound(String),
}

impl HttpBinding {
    /// The address, when there is one.
    pub fn address(&self) -> Option<&str> {
        match self {
            HttpBinding::Bound(addr) => Some(addr),
            _ => None,
        }
    }
}

/// Rewrite an unroutable bind address to its loopback equivalent: `0.0.0.0`
/// and `[::]` are addresses a server can listen on but a client can never
/// dial, and people naturally paste the same address they gave `serve --http`.
pub fn loopback_connect_addr(addr: &str) -> String {
    if let Some(port) = addr.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{port}")
    } else if let Some(port) = addr.strip_prefix("[::]:") {
        format!("127.0.0.1:{port}")
    } else {
        addr.to_string()
    }
}

/// What this process asked to serve, recorded by `run_serve` before it takes
/// the lock.
///
/// One place, three readers: the lock record (`Ownership::publish`), the
/// refusal a losing `serve` prints, and the `/health` body. Recording it ahead
/// of the lock is what lets the refusal name what this invocation wanted, which
/// is the fact the old message left out.
#[derive(Debug, Clone)]
pub struct ServeIntent {
    pub started_by: StartMode,
    pub http: HttpBinding,
    pub allowed_hosts: Vec<String>,
}

static SERVE_INTENT: std::sync::OnceLock<ServeIntent> = std::sync::OnceLock::new();

/// Record what this process asked to serve. The first call wins; a later one
/// is ignored, so a record and a `/health` body can never disagree.
pub fn record_serve_intent(intent: ServeIntent) {
    let _ = SERVE_INTENT.set(intent);
}

/// What this process asked to serve, when it is a daemon that recorded it.
/// `None` in every process that is not serving.
pub fn serve_intent() -> Option<&'static ServeIntent> {
    SERVE_INTENT.get()
}
