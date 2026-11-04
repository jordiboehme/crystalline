//! Build the Claude Desktop extension, and offer its update when a newer
//! Crystalline runs under an older extension.
//!
//! [`build_extension`] is what `crystalline install claude-desktop` does:
//! pack the hollow extension at this version ([`crate::desktop_extension`]),
//! write it to the pinned path, hand it to Claude Desktop when asked, and
//! put it into the record ([`crate::desktop_record`]).
//!
//! [`offer_update`] is the same build for a running Crystalline whose
//! extension is older: the extension's launcher passes its own version on,
//! and when this binary is newer and the person once ran
//! `install claude-desktop` (a record with an extension), the update is
//! built and opened, so Claude Desktop shows it as an update of the
//! installed extension. It is offered once per version: the record keeps
//! the version last offered, written only after Claude Desktop took the
//! file, so a person who declines is not asked on every start and a failed
//! open is tried again next time. One offer runs at a time: an exclusive
//! lock on `<state_dir>/claude-desktop.offer.lock` covers the build and the
//! open, and a second caller that finds it held does nothing. The lock is
//! the operating system's, so a process that dies lets go of it.
//!
//! The MCP bridge calls [`offer_update`] itself on macOS and outside an app
//! package; inside Claude Desktop's package on Windows it asks the daemon,
//! which runs outside the package, to call it (the `desktop_offer` control
//! command). Nothing here prints or logs; the callers do.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::CommandSpelling;
use crate::VERSION;
use crate::desktop_extension::{Bundle, files, pack, shim_sha256};
use crate::desktop_open::DesktopOpener;
use crate::desktop_record::{self, DesktopRecord, ExtensionRecord};
use crate::program::resolve_program;
use crate::version::version_at_least;

/// What a build did with the extension.
#[derive(Debug)]
pub struct BuiltExtension {
    /// Where the `.mcpb` was written.
    pub file: String,
    /// The binary pinned in its manifest.
    pub binary: String,
    /// Whether Claude Desktop took the file.
    pub opened: bool,
    /// Why the open failed, when it was tried and failed.
    pub open_error: Option<String>,
    /// A notice about the pinned path, for the caller to pass on.
    pub warning: Option<String>,
}

/// Build the extension at this version into the pinned path, hand it to
/// `opener` when one is given, and put what was done into `record` (in
/// memory: the caller saves it). An open that worked also sets
/// `record.offered` to this version. `keep` is a binary path kept while it
/// still names an executable crystalline, else the crystalline on PATH or
/// this binary is pinned ([`resolve_program`]).
pub fn build_extension(
    record: &mut DesktopRecord,
    keep: Option<&str>,
    opener: Option<&dyn DesktopOpener>,
) -> anyhow::Result<BuiltExtension> {
    let path = std::env::var_os("PATH");
    let resolved = resolve_program(
        "Claude Desktop",
        CommandSpelling::AbsolutePathEntry,
        path.as_deref(),
        keep,
    );
    let file = desktop_record::bundle_path()?;
    let zip = pack(&files(
        Bundle::Hollow {
            binary: Some(&resolved.program),
        },
        VERSION,
    ))
    .context("could not pack the Claude Desktop extension")?;
    desktop_record::write_replacing(&file, &zip)
        .with_context(|| format!("could not write {}", file.display()))?;
    let open_error = opener.and_then(|opener| opener.open_extension(&file).err());
    let opened = opener.is_some() && open_error.is_none();
    let built = BuiltExtension {
        file: file.display().to_string(),
        binary: resolved.program,
        opened,
        open_error,
        warning: resolved.warning,
    };
    record.extension = Some(ExtensionRecord {
        version: VERSION.to_string(),
        binary: built.binary.clone(),
        shim_sha256: shim_sha256(),
        file: built.file.clone(),
        built_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        opened,
    });
    if opened {
        record.offered = Some(VERSION.to_string());
    }
    Ok(built)
}

/// What [`offer_update`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offered {
    /// Nothing is due: the extension is not older, or there is no record of
    /// an extension `install claude-desktop` built.
    Nothing,
    /// The update for this version was offered before.
    Already,
    /// Another offer holds the lock; this one did nothing.
    Busy,
    /// The update was built and Claude Desktop took it.
    Now,
}

impl Offered {
    /// The word the `desktop_offer` control command answers with.
    pub fn as_str(self) -> &'static str {
        match self {
            Offered::Nothing => "nothing",
            Offered::Already => "already",
            Offered::Busy => "busy",
            Offered::Now => "offered",
        }
    }
}

/// Whether `newer` is a strictly higher `major.minor.patch` than `older`.
/// A pre-release suffix is ignored, and a version that does not parse is
/// never newer.
pub fn is_newer(newer: &str, older: &str) -> bool {
    version_at_least(newer, older) && !version_at_least(older, newer)
}

/// What `record` says about an update to this version: [`Offered::Nothing`]
/// when there is no record of an extension, [`Offered::Already`] when it
/// was offered, `None` when it is due.
fn recorded(record: Option<&DesktopRecord>) -> Option<Offered> {
    let Some(record) = record.filter(|r| r.extension.is_some()) else {
        return Some(Offered::Nothing);
    };
    match &record.offered {
        Some(offered) if version_at_least(offered, VERSION) => Some(Offered::Already),
        _ => None,
    }
}

/// The record at `path`; one that cannot be read is no record.
fn load_record(path: &Path) -> Option<DesktopRecord> {
    desktop_record::load(path).ok().flatten()
}

/// Whether the update to this version was offered before, by the record
/// alone. The bridge's launcher note says "offered" only then, so it never
/// says so for an offer that failed.
pub fn offered_already() -> bool {
    desktop_record::record_path()
        .ok()
        .and_then(|path| load_record(&path))
        .is_some_and(|record| recorded(Some(&record)) == Some(Offered::Already))
}

/// Where the offer lock lives, `<state_dir>/claude-desktop.offer.lock`.
pub fn lock_path() -> anyhow::Result<PathBuf> {
    Ok(crate::config::state_dir()?.join("claude-desktop.offer.lock"))
}

/// Offer the update for an extension of version `launcher`: when this
/// binary is newer, a record of an extension exists and this version was
/// not offered yet, build the extension at this version (keeping the
/// binary the record pins while it still runs), open it with `opener` and
/// record this version as offered. One offer at a time, under the lock at
/// [`lock_path`].
///
/// A failed build or open keeps the record as it was and puts the bundle
/// that was there before back, so nothing half done stays behind; the
/// error says what failed. The next start tries again.
pub fn offer_update(launcher: &str, opener: &dyn DesktopOpener) -> anyhow::Result<Offered> {
    if !is_newer(VERSION, launcher) {
        return Ok(Offered::Nothing);
    }
    let record_path = desktop_record::record_path()?;
    if let Some(answer) = recorded(load_record(&record_path).as_ref()) {
        return Ok(answer);
    }

    let lock_file = lock_path()?;
    if let Some(parent) = lock_file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_file)
        .with_context(|| format!("could not open {}", lock_file.display()))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(Offered::Busy),
        Err(std::fs::TryLockError::Error(e)) => {
            return Err(e).with_context(|| format!("could not lock {}", lock_file.display()));
        }
    }
    // The lock is held until `lock` drops at the end of this call.

    // Another offer may have finished between the first look and the lock.
    let record = load_record(&record_path);
    if let Some(answer) = recorded(record.as_ref()) {
        return Ok(answer);
    }
    let Some(mut record) = record else {
        return Ok(Offered::Nothing);
    };
    let keep = record.extension.as_ref().map(|e| e.binary.clone());
    let bundle = desktop_record::bundle_path()?;
    let kept = Previous::set_aside(&bundle)?;
    let outcome = build_extension(&mut record, keep.as_deref(), Some(opener)).and_then(|built| {
        match built.open_error {
            Some(error) => anyhow::bail!("Claude Desktop did not open it: {error}"),
            None => Ok(()),
        }
    });
    if let Err(e) = outcome {
        kept.put_back(&bundle);
        return Err(e);
    }
    kept.forget();
    desktop_record::save(&record_path, &record)
        .with_context(|| format!("could not save {}", record_path.display()))?;
    drop(lock);
    Ok(Offered::Now)
}

/// The bundle that was at the pinned path before an offer, moved aside to
/// `<bundle>.previous` so a failed offer can put it back.
struct Previous(Option<PathBuf>);

impl Previous {
    fn set_aside(bundle: &Path) -> anyhow::Result<Previous> {
        if !bundle.is_file() {
            return Ok(Previous(None));
        }
        let aside = previous_path(bundle);
        std::fs::rename(bundle, &aside)
            .with_context(|| format!("could not move {} aside", bundle.display()))?;
        Ok(Previous(Some(aside)))
    }

    /// Remove what the failed offer wrote and put the old bundle back.
    fn put_back(self, bundle: &Path) {
        if bundle.is_file() {
            let _ = std::fs::remove_file(bundle);
        }
        if let Some(aside) = self.0 {
            let _ = std::fs::rename(aside, bundle);
        }
    }

    /// The offer worked: the old bundle goes.
    fn forget(self) {
        if let Some(aside) = self.0 {
            let _ = std::fs::remove_file(aside);
        }
    }
}

/// `<bundle>.previous`.
fn previous_path(bundle: &Path) -> PathBuf {
    let mut aside = bundle.as_os_str().to_owned();
    aside.push(".previous");
    PathBuf::from(aside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_strictly_newer_version_is_newer() {
        assert!(is_newer("0.33.0", "0.32.9"));
        assert!(!is_newer("0.33.0", "0.33.0"));
        assert!(!is_newer("0.33.0", "0.34.0"));
        assert!(
            !is_newer("0.33.0", "0.33.0-rc.1"),
            "a pre-release is that version"
        );
        assert!(!is_newer("0.33.0", "garbage"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    fn record(offered: Option<&str>) -> DesktopRecord {
        DesktopRecord {
            extension: Some(ExtensionRecord {
                version: "0.1.0".to_string(),
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
    fn the_record_decides_whether_an_update_is_due() {
        assert_eq!(recorded(None), Some(Offered::Nothing), "no record");
        assert_eq!(
            recorded(Some(&DesktopRecord::default())),
            Some(Offered::Nothing),
            "a record without an extension"
        );
        assert_eq!(recorded(Some(&record(None))), None, "never offered");
        assert_eq!(
            recorded(Some(&record(Some("0.1.0")))),
            None,
            "an offer of an older version does not count"
        );
        assert_eq!(
            recorded(Some(&record(Some(VERSION)))),
            Some(Offered::Already)
        );
        assert_eq!(
            recorded(Some(&record(Some("999.0.0")))),
            Some(Offered::Already),
            "a newer crystalline offered more already"
        );
    }

    #[test]
    fn a_set_aside_bundle_comes_back_after_a_failed_offer() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("crystalline.mcpb");
        std::fs::write(&bundle, "old").unwrap();
        let kept = Previous::set_aside(&bundle).unwrap();
        assert!(!bundle.exists());
        std::fs::write(&bundle, "half").unwrap();
        kept.put_back(&bundle);
        assert_eq!(std::fs::read_to_string(&bundle).unwrap(), "old");
        assert!(!previous_path(&bundle).exists());

        let kept = Previous::set_aside(&bundle).unwrap();
        std::fs::write(&bundle, "new").unwrap();
        kept.forget();
        assert_eq!(std::fs::read_to_string(&bundle).unwrap(), "new");
        assert!(!previous_path(&bundle).exists());

        let none = dir.path().join("none.mcpb");
        let kept = Previous::set_aside(&none).unwrap();
        std::fs::write(&none, "half").unwrap();
        kept.put_back(&none);
        assert!(
            !none.exists(),
            "a half-done bundle with nothing before it is removed"
        );
    }
}
