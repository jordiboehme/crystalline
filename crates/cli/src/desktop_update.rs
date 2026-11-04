//! A newer `crystalline mcp` offers the Claude Desktop extension update.
//!
//! The extension `install claude-desktop` builds carries the Crystalline
//! version that built it, and its launcher passes that version on as
//! `CRYSTALLINE_LAUNCHER_VERSION`, with `CRYSTALLINE_CHANNEL=desktop`. When
//! the Crystalline it starts is newer, Claude Desktop runs an old extension
//! after an upgrade. Then this binary builds the extension again at its own
//! version, exactly as `install claude-desktop` does
//! ([`crate::install_desktop::build_extension`]), and hands it to Claude
//! Desktop, which shows it as an update of the installed extension.
//!
//! It happens once per version: the record keeps the version last offered
//! ([`DesktopRecord::offered`]), so a person who declines the dialog is not
//! asked again on every start. Only a person who once ran
//! `install claude-desktop` (a record with an extension) is asked. The
//! "Claude Desktop only" bundle starts this binary with the channel `mcpb`
//! and no launcher, so it never gets here.
//!
//! Inside an app package (Claude Desktop's on Windows) nothing is offered:
//! the build and the record would land in the package's private copy of
//! AppData, where `install`, `doctor` and `uninstall` never look.
//!
//! The work runs on a plain thread of its own, so the MCP session starts
//! and serves as usual and never waits for it. A plain thread and not
//! tokio's blocking pool, because the runtime waits for that pool when it
//! shuts down, and a slow open must not keep the process alive after the
//! client left. Nothing here prints: stdout is the JSON-RPC channel, and a
//! failure is one line in the bridge's log (stderr).

use crystalline_remote::server_client::version_at_least;
use crystalline_service::overlay::{CHANNEL_ENV, LAUNCHER_VERSION_ENV};

use crate::desktop_record::{self, DesktopRecord};

/// The Crystalline version this binary offers.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What this start knows about itself.
#[derive(Debug, Clone, Copy)]
struct Start<'a> {
    /// `CRYSTALLINE_CHANNEL`.
    channel: Option<&'a str>,
    /// `CRYSTALLINE_LAUNCHER_VERSION`, the extension's version.
    launcher: Option<&'a str>,
    /// This binary's version.
    own: &'a str,
    /// Whether this process runs inside an app package.
    packaged: bool,
    /// Whether this is macOS or Windows, where Claude Desktop runs.
    desktop_os: bool,
}

/// What a start does about the extension.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Nothing: not an older extension, no record, or not here.
    Nothing,
    /// The update for this version was offered by an earlier start.
    OfferedAlready,
    /// Build and open the update, then save this record with it.
    Offer(Box<DesktopRecord>),
}

/// Whether `newer` is a strictly higher `major.minor.patch` than `older`.
/// A pre-release suffix is ignored, and a version that does not parse is
/// never newer.
fn is_newer(newer: &str, older: &str) -> bool {
    version_at_least(newer, older) && !version_at_least(older, newer)
}

/// Whether `start` is this binary run by an older extension's launcher,
/// on macOS or Windows and outside an app package.
fn runs_an_older_extension(start: Start<'_>) -> bool {
    start.desktop_os
        && !start.packaged
        && start.channel == Some("desktop")
        && start
            .launcher
            .is_some_and(|launcher| is_newer(start.own, launcher))
}

/// The decision for `start` with the record `record` (`None` when there is
/// none or it cannot be read).
fn decide(start: Start<'_>, record: Option<DesktopRecord>) -> Decision {
    if !runs_an_older_extension(start) {
        return Decision::Nothing;
    }
    let Some(record) = record.filter(|r| r.extension.is_some()) else {
        return Decision::Nothing;
    };
    match &record.offered {
        Some(offered) if version_at_least(offered, start.own) => Decision::OfferedAlready,
        _ => Decision::Offer(Box::new(record)),
    }
}

/// Decide for this process and, when an update is due, offer it on a
/// thread of its own. Returns at once, before anything is built. The answer
/// is whether the update for this version is offered (now or by an earlier
/// start), so the bridge's launcher note can say so.
pub(crate) fn offer_in_background() -> bool {
    let channel = std::env::var(CHANNEL_ENV).ok();
    let launcher = std::env::var(LAUNCHER_VERSION_ENV).ok();
    let start = Start {
        channel: channel.as_deref(),
        launcher: launcher.as_deref(),
        own: VERSION,
        packaged: crystalline_service::runs_in::PackageContext::here().is_packaged(),
        desktop_os: cfg!(any(target_os = "macos", windows)),
    };
    if !runs_an_older_extension(start) {
        return false;
    }
    // A record that cannot be read is no record.
    let record = desktop_record::record_path()
        .ok()
        .and_then(|path| desktop_record::load(&path).ok().flatten());
    match decide(start, record) {
        Decision::Nothing => false,
        Decision::OfferedAlready => {
            tracing::info!(
                "the Claude Desktop extension update to Crystalline {VERSION} was offered already"
            );
            true
        }
        Decision::Offer(record) => {
            let spawned = std::thread::Builder::new()
                .name("desktop-update".to_string())
                .spawn(move || match offer(*record) {
                    Ok(()) => tracing::info!(
                        "offered the Claude Desktop extension update to Crystalline {VERSION}"
                    ),
                    Err(e) => tracing::warn!(
                        "could not offer the Claude Desktop extension update to Crystalline {VERSION}: {e:#}"
                    ),
                });
            if let Err(e) = spawned {
                tracing::warn!(
                    "could not offer the Claude Desktop extension update to Crystalline {VERSION}: {e}"
                );
                return false;
            }
            true
        }
    }
}

/// Build the extension at this version, keeping the binary the record
/// pins while it still names a crystalline, open it in Claude Desktop and
/// save the record with this version as offered. A failed build or open
/// saves nothing, so the next start tries again.
fn offer(mut record: DesktopRecord) -> anyhow::Result<()> {
    let keep = record.extension.as_ref().map(|e| e.binary.clone());
    let built = crate::install_desktop::build_extension(&mut record, keep.as_deref(), true)?;
    if let Some(warning) = &built.warning {
        tracing::info!("Claude Desktop extension: {warning}");
    }
    if let Some(error) = built.open_error {
        anyhow::bail!("Claude Desktop did not open it: {error}");
    }
    let path = desktop_record::record_path()?;
    desktop_record::save(&path, &record)
        .map_err(|e| anyhow::anyhow!("could not save {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(launcher: Option<&str>) -> Start<'_> {
        Start {
            channel: Some("desktop"),
            launcher,
            own: "0.33.0",
            packaged: false,
            desktop_os: true,
        }
    }

    fn record(offered: Option<&str>) -> DesktopRecord {
        DesktopRecord {
            extension: Some(desktop_record::ExtensionRecord {
                version: "0.32.0".to_string(),
                binary: "/opt/homebrew/bin/crystalline".to_string(),
                shim_sha256: "ab".repeat(32),
                file: "/state/claude-desktop/crystalline.mcpb".to_string(),
                built_at: "2026-11-04T10:00:00Z".to_string(),
                opened: true,
            }),
            offered: offered.map(str::to_string),
            ..DesktopRecord::default()
        }
    }

    #[test]
    fn only_an_older_launcher_gets_the_offer() {
        assert_eq!(
            decide(start(Some("0.32.0")), Some(record(None))),
            Decision::Offer(Box::new(record(None)))
        );
        assert_eq!(
            decide(start(Some("0.33.0")), Some(record(None))),
            Decision::Nothing,
            "same version"
        );
        assert_eq!(
            decide(start(Some("0.34.0")), Some(record(None))),
            Decision::Nothing,
            "newer launcher"
        );
        assert_eq!(
            decide(start(Some("0.33.0-rc.1")), Some(record(None))),
            Decision::Nothing,
            "a pre-release of this version is this version"
        );
        assert_eq!(
            decide(start(Some("garbage")), Some(record(None))),
            Decision::Nothing
        );
        assert_eq!(
            decide(start(None), Some(record(None))),
            Decision::Nothing,
            "no launcher"
        );
    }

    #[test]
    fn without_a_record_of_an_extension_nothing_is_offered() {
        assert_eq!(decide(start(Some("0.32.0")), None), Decision::Nothing);
        let skill_only = DesktopRecord::default();
        assert_eq!(
            decide(start(Some("0.32.0")), Some(skill_only)),
            Decision::Nothing
        );
    }

    #[test]
    fn an_update_is_offered_once_per_version() {
        assert_eq!(
            decide(start(Some("0.32.0")), Some(record(Some("0.33.0")))),
            Decision::OfferedAlready
        );
        assert_eq!(
            decide(start(Some("0.32.0")), Some(record(Some("0.32.0")))),
            Decision::Offer(Box::new(record(Some("0.32.0")))),
            "an offer of an older version does not count"
        );
        assert_eq!(
            decide(start(Some("0.32.0")), Some(record(Some("0.34.0")))),
            Decision::OfferedAlready,
            "a newer crystalline offered more already"
        );
    }

    #[test]
    fn only_the_desktop_extension_on_macos_or_windows_outside_a_package() {
        let due = start(Some("0.32.0"));
        for not_here in [
            Start {
                channel: Some("mcpb"),
                ..due
            },
            Start {
                channel: None,
                ..due
            },
            Start {
                packaged: true,
                ..due
            },
            Start {
                desktop_os: false,
                ..due
            },
        ] {
            assert_eq!(
                decide(not_here, Some(record(None))),
                Decision::Nothing,
                "{not_here:?}"
            );
        }
    }
}
