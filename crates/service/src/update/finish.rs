//! `crystalline update finish`'s service side: the handover of the daemon
//! to the binary a tar.gz update just put in place (S4).
//!
//! The new binary attaches as a strictly newer client, so the existing
//! displacement path stops the old daemon and starts its successor with the
//! displaced daemon's own start options (database, config file, read-only
//! mode, HTTP address, allowed hosts, environment names). The old binary
//! never stops the daemon itself: a stopped daemon loses how it was
//! started. A handover marker in the state folder
//! ([`crate::instance::HANDOVER_FILE`]) holds while it runs, so a connected
//! bridge waits for this successor instead of starting its own (S24: the
//! session survives the handover).

use crystalline_core::config;

use crate::instance::{
    SuccessorPlan, ask_holder_at, attach_after_displacement, displace_for_handover, displaces,
    successor_plan_here,
};
use crate::runs_in::PackageContext;

/// What [`hand_over`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handover {
    /// This process runs inside an app package, where a client never
    /// displaces a daemon (`attach_policy_for`). Nothing was touched.
    Packaged,
    /// No daemon answers on this state folder's pipe.
    NoDaemon,
    /// The daemon runs this version or a newer one, or is a standby.
    NotOlder { version: String },
    /// An older daemon was left running, because its start cannot be
    /// repeated from here: `why` says what is missing, such as an
    /// environment variable it read.
    Kept { version: String, why: String },
    /// An older daemon was asked to stop and is still running.
    StillRunning { version: String },
    /// The older daemon stopped and its successor of this version runs.
    HandedOver { from: String },
    /// The older daemon stopped, but no successor became ready.
    NoSuccessor { from: String },
}

/// Hand the running daemon over to this binary when it is older: decide
/// first whether its successor can be started from here with its own start
/// options, and only then stop it, so a daemon whose environment is missing
/// in this shell keeps running instead of coming back without it.
pub async fn hand_over() -> Handover {
    let here = PackageContext::here();
    if here.is_packaged() {
        return Handover::Packaged;
    }
    let Ok(sock) = config::service_sock_path() else {
        return Handover::NoDaemon;
    };
    let Some(facts) = ask_holder_at(&sock).await else {
        return Handover::NoDaemon;
    };
    let version = facts.version.clone();
    if !displaces(&facts, crystalline_core::VERSION, here) {
        return Handover::NotOlder { version };
    }
    if let SuccessorPlan::Wait { why, .. } = successor_plan_here(facts.start.as_ref()) {
        return Handover::Kept { version, why };
    }
    // Marked before the old daemon is asked to stop, and removed when this
    // call returns: a connected bridge reconnects the moment its socket
    // closes, and while the marker holds it waits for the successor started
    // below instead of starting one with its own flags.
    let (gone, _marker) = displace_for_handover(&sock, facts.pid).await;
    if !gone {
        return Handover::StillRunning { version };
    }
    match attach_after_displacement(facts.start).await {
        Some(_) => Handover::HandedOver { from: version },
        None => Handover::NoSuccessor { from: version },
    }
}
