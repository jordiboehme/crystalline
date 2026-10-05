//! The two answers a machine keeps on disk per source, and nothing else of
//! the server's knowledge: the routing model with its domain list
//! (`routing.json`) and the maintenance status the Stop hook reads
//! (`hook_status.json`), in the source's host folder, each with the etag it
//! came with, the account it was answered for and the last failure to
//! refresh it. A cache written for another account is never read: the
//! answers are scoped to who asked.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::RemoteError;
use crate::mounts::RemoteDomain;
use crate::server_client::{Connection, CtlAnswer, RemoteFailure, UNREACHABLE_WORDS};

/// The cached routing model.
pub const ROUTING_FILE: &str = "routing.json";
/// The cached maintenance status.
pub const HOOK_STATUS_FILE: &str = "hook_status.json";
/// How old a cache may be before a reader with no daemon calls it stale.
pub const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
/// How much longer than its budget a fetch waits before it stops waiting
/// itself: the request's own limit normally ends it first, and records why.
const GRACE: Duration = Duration::from_millis(500);

/// One cached answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cached {
    /// The account it was answered for.
    pub account: String,
    /// The etag it came with.
    pub etag: String,
    /// When the server last confirmed it.
    pub fetched_at: DateTime<Utc>,
    /// The answer.
    pub data: Value,
    /// Why the last try to refresh it failed, `None` when it succeeded.
    #[serde(default)]
    pub last_failure: Option<String>,
}

/// What a fetch came back with.
#[derive(Debug)]
pub enum Fetched {
    /// The server answered (new data, or the cache confirmed).
    Fresh(Cached),
    /// The server did not answer; this is the last answer it gave.
    Stale {
        /// The cached answer.
        cached: Cached,
        /// Why the server did not answer.
        failure: RemoteFailure,
    },
    /// The server did not answer and nothing is cached.
    Missing(RemoteFailure),
}

/// The cached answer in `host_dir`, when it was answered for `account`.
pub fn read_cached(host_dir: &Path, file: &str, account: &str) -> Option<Cached> {
    let bytes = std::fs::read(host_dir.join(file)).ok()?;
    let cached: Cached = serde_json::from_slice(&bytes).ok()?;
    (cached.account == account).then_some(cached)
}

/// Write an answer, atomically.
pub fn write_cached(host_dir: &Path, file: &str, cached: &Cached) -> Result<(), RemoteError> {
    let bytes = serde_json::to_vec(cached).map_err(|e| RemoteError::State(e.to_string()))?;
    crystalline_core::config::save_bytes(&host_dir.join(file), &bytes)
        .map_err(|e| RemoteError::State(format!("could not save a cached answer: {e}")))
}

fn stale_or_missing(
    host_dir: &Path,
    file: &str,
    cached: Option<Cached>,
    failure: RemoteFailure,
) -> Fetched {
    match cached {
        Some(mut cached) => {
            cached.last_failure = Some(failure.to_string());
            let _ = write_cached(host_dir, file, &cached);
            Fetched::Stale { cached, failure }
        }
        None => Fetched::Missing(failure),
    }
}

/// Ask `cmd` with the cached etag, within `budget` overall, and keep what
/// comes back. A source inside its down window answers its failure at once,
/// and the cache is served stale.
pub async fn fetch_cached(
    connection: &Connection,
    cmd: &str,
    file: &str,
    budget: Duration,
) -> Fetched {
    let account = connection.source().account.clone();
    let host_dir = connection.host_dir().to_path_buf();
    let cached = read_cached(&host_dir, file, &account);
    let mut request = json!({ "v": 1, "cmd": cmd });
    if let Some(cached) = &cached {
        request["if_none_match"] = json!(cached.etag);
    }
    let answer = tokio::time::timeout(budget + GRACE, connection.ctl_within(request, budget))
        .await
        .unwrap_or_else(|_| {
            let failure = Err(RemoteFailure::TimedOut {
                source: connection.source().name.clone(),
                url: connection.source().url.clone(),
                after: budget,
            });
            connection.note(&failure);
            failure
        });
    match answer {
        Ok(CtlAnswer::NotModified { .. }) => match cached {
            Some(mut cached) => {
                cached.fetched_at = Utc::now();
                cached.last_failure = None;
                let _ = write_cached(&host_dir, file, &cached);
                Fetched::Fresh(cached)
            }
            None => Fetched::Missing(RemoteFailure::Refused(
                "the server answered not_modified to a request that held no etag".to_string(),
            )),
        },
        Ok(CtlAnswer::Data { data, etag }) => {
            let fresh = Cached {
                account,
                etag: etag.unwrap_or_default(),
                fetched_at: Utc::now(),
                data,
                last_failure: None,
            };
            let _ = write_cached(&host_dir, file, &fresh);
            Fetched::Fresh(fresh)
        }
        Err(failure) => stale_or_missing(&host_dir, file, cached, failure),
    }
}

/// The domains a cached or fresh routing model lists.
pub fn remote_domains(routing: &Value) -> Vec<RemoteDomain> {
    routing["domains"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value(row.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The one line a stale part of the routing block carries. `failure` is the
/// recorded reason (a [`RemoteFailure`]'s words), `None` for a copy that is
/// merely old. The line names the real cause: a server that did not answer,
/// a sign-in that ran out, or what the server answered instead.
pub fn stale_line(
    source: &str,
    url: &str,
    fetched_at: DateTime<Utc>,
    failure: Option<&str>,
) -> String {
    let when = fetched_at.format("%Y-%m-%d %H:%M UTC");
    match failure {
        Some(failure) if failure.contains("sign in again") => format!(
            "Note: the sign-in to {source} ({url}) has run out, so its domains in this routing block are the copy from {when} and may be out of date. Ask the user to run: crystalline connect {url}"
        ),
        Some(failure) if failure.contains(UNREACHABLE_WORDS) => format!(
            "Note: {source} ({url}) cannot be reached right now, so its domains in this routing block are the copy from {when} and may be out of date."
        ),
        Some(failure) => format!(
            "Note: {source} ({url}) did not send its routing ({failure}), so its domains in this routing block are the copy from {when} and may be out of date."
        ),
        None => format!(
            "Note: {source} ({url}) has not been refreshed since {when}, so its domains in this routing block may be out of date."
        ),
    }
}
