//! A newer `crystalline mcp` offers the Claude Desktop extension update.
//!
//! The extension `install claude-desktop` builds carries the Crystalline
//! version that built it, and its launcher passes that version on as
//! `CRYSTALLINE_LAUNCHER_VERSION`, with `CRYSTALLINE_CHANNEL=desktop`. When
//! this binary is newer, Claude Desktop runs an old extension after an
//! upgrade, and the update is offered
//! ([`crate::desktop_offer::offer_update`]: same builder as
//! `install claude-desktop`, once per version, one offer at a time). The
//! "Claude Desktop only" bundle starts this binary with the channel `mcpb`
//! and no launcher, so it never gets here.
//!
//! Two routes, one rule:
//!
//! - Outside an app package (macOS, and Windows when Claude Desktop is not
//!   the packaged app) the bridge offers it itself, on a plain thread of
//!   its own. This needs no daemon, so it works for an embedded session
//!   too, and it adds no round trip to the start.
//! - Inside Claude Desktop's app package on Windows the bridge writes
//!   nothing: a file under AppData would land in the package's private
//!   copy. It asks the daemon, which runs outside the package, with the
//!   `desktop_offer` control command (`answer_ctl`), and the daemon
//!   checks the record and offers it the same way. The request carries the
//!   bridge's own version, and the daemon offers only when its version is
//!   the same: it builds at its own version, so a daemon older or newer
//!   than the bridge (one not taken over yet) answers `nothing`, and the
//!   offer comes once the daemon matches. The bridge sends the request and
//!   does not wait for the answer.
//!
//! The session never waits for an offer: the bridge's thread and the
//! daemon's are plain threads, not tokio's blocking pool (the runtime
//! waits for that pool when it shuts down, and a slow open must not keep
//! the process alive after the client left). Nothing here prints: stdout
//! is the JSON-RPC channel, and an offer's outcome is a line in the log
//! (stderr for the bridge, `daemon.log` for the daemon).

use serde_json::{Value, json};

use crystalline_core::VERSION;

use crate::desktop_offer::{self, Offered};
use crate::desktop_open;
use crate::overlay::{CHANNEL_ENV, LAUNCHER_VERSION_ENV};

/// The control command a packaged bridge sends the daemon.
pub const DESKTOP_OFFER_COMMAND: &str = "desktop_offer";

/// The start of the log line for an offer that failed.
const FAILED: &str = "could not offer the Claude Desktop extension update to Crystalline";

/// The launcher version of an older Claude Desktop extension that started
/// this process, from the environment: `Some` only on the `desktop`
/// channel, on macOS or Windows, when the launcher's version is older than
/// this binary.
pub(crate) fn older_extension() -> Option<String> {
    let channel = std::env::var(CHANNEL_ENV).ok();
    let launcher = std::env::var(LAUNCHER_VERSION_ENV).ok();
    older(channel.as_deref(), launcher.as_deref(), desktop_os()).map(str::to_string)
}

/// Whether Claude Desktop runs on this system.
fn desktop_os() -> bool {
    cfg!(any(target_os = "macos", windows))
}

/// [`older_extension`] without the environment.
fn older<'a>(
    channel: Option<&str>,
    launcher: Option<&'a str>,
    desktop_os: bool,
) -> Option<&'a str> {
    let launcher = launcher?;
    (desktop_os && channel == Some("desktop") && desktop_offer::is_newer(VERSION, launcher))
        .then_some(launcher)
}

/// Offer the update for an extension of `launcher` on a thread of its own
/// and log what came of it. Returns at once.
pub(crate) fn offer_on_thread(launcher: String) {
    let spawned = std::thread::Builder::new()
        .name("desktop-offer".to_string())
        .spawn(move || offer_and_log(&launcher));
    if let Err(e) = spawned {
        tracing::warn!("{FAILED} {VERSION}: {e}");
    }
}

/// Offer the update for an extension of `launcher` and log the outcome:
/// one warning when it failed, an info line otherwise.
fn offer_and_log(launcher: &str) {
    let opener = desktop_open::for_this_process();
    match desktop_offer::offer_update(launcher, opener.as_ref()) {
        Ok(Offered::Now) => {
            tracing::info!("offered the Claude Desktop extension update to Crystalline {VERSION}")
        }
        Ok(Offered::Already) => tracing::info!(
            "the Claude Desktop extension update to Crystalline {VERSION} was offered already"
        ),
        Ok(Offered::Busy) => tracing::info!(
            "another Crystalline is offering the Claude Desktop extension update; this one does nothing"
        ),
        Ok(Offered::Nothing) => {}
        Err(e) => tracing::warn!("{FAILED} {VERSION}: {e:#}"),
    }
}

/// The daemon's answer to `desktop_offer`, one of three words. `nothing`:
/// the bridge is another Crystalline version than this daemon, the
/// extension is not older, this is not macOS or Windows, or there is no
/// record of an extension. `already`: the record shows this version as
/// offered. `started`: the offer runs on a thread of its own, so the
/// answer comes at once. Only `started` starts a thread. The daemon checks
/// the record itself; the bridge names only its launcher's version and its
/// own.
pub(crate) fn answer_ctl(req: &Value) -> Result<Value, String> {
    let launcher = req
        .get("launcher")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{DESKTOP_OFFER_COMMAND} needs the launcher version"))?;
    let bridge = req.get("version").and_then(Value::as_str);
    let answer = if bridge != Some(VERSION)
        || !desktop_os()
        || !desktop_offer::is_newer(VERSION, launcher)
    {
        "nothing"
    } else {
        match desktop_offer::recorded_now() {
            Some(Offered::Already) => "already",
            Some(_) => "nothing",
            None => {
                offer_on_thread(launcher.to_string());
                "started"
            }
        }
    };
    Ok(json!({ "offer": answer }))
}

/// The `desktop_offer` request a packaged bridge sends for `launcher`.
fn offer_request(launcher: &str) -> Value {
    json!({
        "v": 1,
        "cmd": DESKTOP_OFFER_COMMAND,
        "launcher": launcher,
        "version": VERSION,
    })
}

/// The bridge's part, once it is attached to the daemon. Inside a package
/// it sends `desktop_offer` on a task of its own and never waits for the
/// answer, so the session is served at once (Jordi's rule: the session
/// never waits for an offer). The answer is only logged. A plain function,
/// not an async one, so no caller can await the request by mistake.
///
/// Returns whether the launcher note may say the update was offered: only
/// when the record this process can read shows this version as offered,
/// that is when an earlier offer worked. `false` when no older extension
/// started this process.
pub(crate) fn offer_for_bridge(launcher: Option<&str>, packaged: bool) -> bool {
    offer_for_bridge_with(launcher, packaged, crate::client::ctl_if_running_passive)
}

/// [`offer_for_bridge`] with the way the request reaches the daemon given,
/// so a test can hand in one that never answers and reaches no daemon.
fn offer_for_bridge_with<F, Fut>(launcher: Option<&str>, packaged: bool, send: F) -> bool
where
    F: FnOnce(Value) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Option<Value>>> + Send + 'static,
{
    let Some(launcher) = launcher else {
        return false;
    };
    if packaged {
        let asked = send(offer_request(launcher));
        tokio::spawn(async move {
            match asked.await {
                Ok(Some(data)) => {
                    let answer = data.get("offer").and_then(|word| word.as_str());
                    let answer = answer.unwrap_or("?");
                    tracing::info!(
                        "the daemon answered the Claude Desktop extension update: {answer}"
                    )
                }
                Ok(None) => tracing::info!(
                    "no daemon answered, so the Claude Desktop extension update was not offered"
                ),
                Err(e) => tracing::warn!("{FAILED} {VERSION}: the daemon refused it ({e:#})"),
            }
        });
    }
    desktop_offer::offered_already()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_older_desktop_extension_on_macos_or_windows_counts() {
        assert_eq!(older(Some("desktop"), Some("0.1.0"), true), Some("0.1.0"));
        assert_eq!(
            older(Some("desktop"), Some(VERSION), true),
            None,
            "same version"
        );
        assert_eq!(
            older(Some("desktop"), Some("999.0.0"), true),
            None,
            "newer launcher"
        );
        assert_eq!(older(Some("desktop"), Some("garbage"), true), None);
        assert_eq!(older(Some("desktop"), None, true), None, "no launcher");
        assert_eq!(
            older(Some("mcpb"), Some("0.1.0"), true),
            None,
            "the Desktop-only bundle"
        );
        assert_eq!(older(None, Some("0.1.0"), true), None);
        assert_eq!(
            older(Some("desktop"), Some("0.1.0"), false),
            None,
            "not macOS or Windows"
        );
    }

    #[test]
    fn the_daemon_needs_the_launcher_version() {
        assert_eq!(
            answer_ctl(&json!({ "v": 1, "cmd": DESKTOP_OFFER_COMMAND })),
            Err("desktop_offer needs the launcher version".to_string())
        );
        assert_eq!(
            answer_ctl(
                &json!({ "v": 1, "cmd": DESKTOP_OFFER_COMMAND, "launcher": VERSION, "version": VERSION })
            ),
            Ok(json!({ "offer": "nothing" })),
            "an extension of this version gets nothing"
        );
    }

    /// The daemon builds at its own version, so it offers only to a bridge
    /// of that version. These answers come before the record is read, so
    /// the test touches no state.
    #[test]
    fn the_daemon_offers_only_to_a_bridge_of_its_own_version() {
        for bridge in [json!("0.0.1"), json!("999.0.0"), Value::Null] {
            let mut req = offer_request("0.1.0");
            req["version"] = bridge.clone();
            assert_eq!(
                answer_ctl(&req),
                Ok(json!({ "offer": "nothing" })),
                "{bridge}"
            );
        }
        assert_eq!(offer_request("0.1.0")["version"], VERSION);
    }

    /// The bridge sends the request and returns at once, even when nothing
    /// will ever answer it.
    #[tokio::test]
    async fn the_bridge_never_waits_for_the_daemon() {
        let (sent, mut seen) = tokio::sync::mpsc::unbounded_channel();
        let started = std::time::Instant::now();
        let _ = offer_for_bridge_with(Some("0.1.0"), true, move |request| {
            sent.send(request).unwrap();
            // A daemon that never answers.
            std::future::pending::<anyhow::Result<Option<Value>>>()
        });
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
        let request = seen.recv().await.expect("the request went out");
        assert_eq!(request, offer_request("0.1.0"));

        let never = |_| async { panic!("only a packaged bridge asks the daemon") };
        let _ = offer_for_bridge_with(Some("0.1.0"), false, never);
        assert!(
            !offer_for_bridge_with(None, true, never),
            "no older extension"
        );
    }
}
