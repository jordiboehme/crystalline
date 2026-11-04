//! A newer `crystalline mcp` offers the Claude Desktop extension update.
//!
//! The extension `install claude-desktop` builds carries the Crystalline
//! version that built it, and its launcher passes that version on as
//! `CRYSTALLINE_LAUNCHER_VERSION`, with `CRYSTALLINE_CHANNEL=desktop`. When
//! this binary is newer, Claude Desktop runs an old extension after an
//! upgrade, and the update is offered
//! ([`crystalline_core::desktop_offer::offer_update`]: same builder as
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
//!   checks the record and offers it the same way.
//!
//! The session never waits for an offer: the bridge's thread and the
//! daemon's are plain threads, not tokio's blocking pool (the runtime
//! waits for that pool when it shuts down, and a slow open must not keep
//! the process alive after the client left). Nothing here prints: stdout
//! is the JSON-RPC channel, and an offer's outcome is a line in the log
//! (stderr for the bridge, `daemon.log` for the daemon).

use serde_json::{Value, json};

use crystalline_core::VERSION;
use crystalline_core::desktop_offer::{self, Offered};
use crystalline_core::desktop_open;

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

/// The daemon's answer to `desktop_offer`: `{"offer": "nothing"}` or
/// `"already"` from the record, else `"started"` with the offer running on
/// a thread of its own, so the answer comes at once. The daemon checks the
/// record itself; the bridge only names its launcher's version.
pub(crate) fn answer_ctl(req: &Value) -> Result<Value, String> {
    let launcher = req
        .get("launcher")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{DESKTOP_OFFER_COMMAND} needs the launcher version"))?;
    let answer = if !desktop_os() || !desktop_offer::is_newer(VERSION, launcher) {
        "nothing"
    } else if desktop_offer::offered_already() {
        "already"
    } else {
        offer_on_thread(launcher.to_string());
        "started"
    };
    Ok(json!({ "offer": answer }))
}

/// How long a packaged bridge waits for the daemon's answer.
const ASK_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether the launcher note may say the update was offered: only when an
/// earlier offer of this version worked. Outside a package the bridge reads
/// the record; inside one it asks the daemon, which also starts the offer
/// there. `false` when no older extension started this process.
pub(crate) async fn offer_for_bridge(launcher: Option<&str>, packaged: bool) -> bool {
    let Some(launcher) = launcher else {
        return false;
    };
    if !packaged {
        return desktop_offer::offered_already();
    }
    let request = json!({ "v": 1, "cmd": DESKTOP_OFFER_COMMAND, "launcher": launcher });
    let answer = tokio::time::timeout(ASK_WAIT, crate::client::ctl_if_running_passive(request))
        .await
        .ok();
    match answer {
        Some(Ok(Some(data))) => data.get("offer").and_then(Value::as_str) == Some("already"),
        Some(Ok(None)) => {
            tracing::info!(
                "no daemon answered, so the Claude Desktop extension update was not offered"
            );
            false
        }
        Some(Err(e)) => {
            tracing::warn!("{FAILED} {VERSION}: the daemon refused it ({e:#})");
            false
        }
        None => {
            tracing::warn!("{FAILED} {VERSION}: the daemon did not answer in time");
            false
        }
    }
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
            answer_ctl(&json!({ "v": 1, "cmd": DESKTOP_OFFER_COMMAND, "launcher": VERSION })),
            Ok(json!({ "offer": "nothing" })),
            "an extension of this version gets nothing"
        );
    }
}
