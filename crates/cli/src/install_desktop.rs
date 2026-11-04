//! `crystalline install claude-desktop`: build the hollow Claude Desktop
//! extension on this machine and record what was built.
//!
//! Claude Desktop is not a coding harness: it has no hooks, no skill folder
//! Crystalline can write into and no per-project config. So it is not a
//! [`crystalline_core::HarnessKind`] and gets this module of its own. The
//! extension is the launcher-only bundle from
//! [`crystalline_core::desktop_extension`], with the crystalline that
//! `resolve_program` finds on PATH (or this binary) pinned in its manifest,
//! built by `crystalline_service::desktop_offer::build_extension`, which the
//! extension update uses too.
//! It is written to `<state_dir>/claude-desktop/crystalline.mcpb` and
//! remembered in `<state_dir>/claude-desktop.json` (`crystalline_core::desktop_record`).
//! Unless `--no-open` is given, the file is then handed to Claude Desktop
//! (`crystalline_service::desktop_open`), which shows its install dialog.
//!
//! Crystalline cannot see what Claude Desktop installed, so every run builds
//! the extension again and the record says only what this command did last.

use serde_json::{Value, json};

use crystalline_core::desktop_record::{self, DesktopRecord};
use crystalline_service::desktop_offer::{BuiltExtension, build_extension};
use crystalline_service::desktop_open;

/// The Crystalline version this binary builds the extension for.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where Claude Desktop installs an extension from a file.
pub(crate) const INSTALL_ROUTE: &str =
    "Settings > Extensions > Advanced settings > Install Extension...";

/// The last line of every human run.
const FOOTER: &str =
    "Crystalline cannot see what Claude Desktop has installed. It only remembers what it did last.";

/// What a run says when Claude Desktop opened the extension.
const OPENED: &str = "Claude Desktop shows its install dialog. Confirm it there.";

/// The extra step on Windows when the open failed.
const DOUBLE_CLICK: &str = "You can also double-click the file in File Explorer.";

/// The refusal off macOS and Windows.
const LINUX_REFUSAL: &str =
    "Claude Desktop is not available on Linux, so there is nothing to install.";

/// The refusal of `--project`.
pub(crate) const PROJECT_REFUSAL: &str =
    "--project does not apply to claude-desktop: Claude Desktop extensions are per user.";

/// The refusal of `--skip-hooks`.
pub(crate) const SKIP_HOOKS_REFUSAL: &str =
    "--skip-hooks does not apply to claude-desktop: Claude Desktop has no hooks.";

/// The refusal of `--skip-mcp` with `--skip-skills`.
const NOTHING_TO_INSTALL: &str = "--skip-mcp and --skip-skills together leave nothing to install.";

/// The steps `install claude-desktop` takes.
pub(crate) struct DesktopInstallOptions {
    /// Do not build the extension (`--skip-mcp`).
    pub skip_extension: bool,
    /// Do not save the skill (`--skip-skills`).
    pub skip_skill: bool,
    /// Hand the extension to Claude Desktop (no `--no-open`).
    pub open: bool,
}

/// Run `crystalline install claude-desktop`.
pub(crate) fn run_install(opts: DesktopInstallOptions, json: bool) -> anyhow::Result<()> {
    if !cfg!(any(target_os = "macos", windows)) {
        anyhow::bail!(LINUX_REFUSAL);
    }
    if opts.skip_extension && opts.skip_skill {
        anyhow::bail!(NOTHING_TO_INSTALL);
    }

    let record_path = desktop_record::record_path()?;
    // A record this binary cannot read is no record: it is rewritten whole,
    // and the report says so once the new one is written.
    let (mut record, unreadable) = match desktop_record::load(&record_path) {
        Ok(found) => (found.unwrap_or_default(), None),
        Err(reason) => (DesktopRecord::default(), Some(reason)),
    };
    let mut notices = Vec::new();
    let mut extension = None;

    if !opts.skip_extension {
        let opener = opts.open.then(desktop_open::for_this_process);
        let built = build_extension(&mut record, None, opener.as_deref())?;
        notices.extend(built.warning.clone());
        extension = Some(built);
    }

    // The extension is built by now, so a record that cannot be saved is a
    // notice and the run still exits 0: the person can install the file.
    if extension.is_some() {
        match desktop_record::save(&record_path, &record) {
            Ok(()) => notices.extend(unreadable.map(|reason| {
                format!(
                    "{} could not be read ({reason}), so it was written fresh.",
                    record_path.display()
                )
            })),
            Err(e) => notices.push(format!(
                "Crystalline could not remember this build in {} ({e}), so doctor and uninstall will not know about it.",
                record_path.display()
            )),
        }
    }

    if json {
        println!("{}", report_json(extension.as_ref(), &notices));
    } else {
        print!("{}", report_text(extension.as_ref(), &notices));
    }
    Ok(())
}

/// The one JSON object `--json` prints.
fn report_json(extension: Option<&BuiltExtension>, notices: &[String]) -> Value {
    json!({
        "target": "claude-desktop",
        "version": VERSION,
        "extension": extension.map(|e| json!({
            "file": e.file,
            "binary": e.binary,
            "opened": e.opened,
            "open_error": e.open_error,
        })),
        "skill": Value::Null,
        "notices": notices,
    })
}

/// The human report, one line each, the footer last.
fn report_text(extension: Option<&BuiltExtension>, notices: &[String]) -> String {
    let mut out = String::new();
    if let Some(e) = extension {
        out.push_str(&format!(
            "Built the Claude Desktop extension for Crystalline {VERSION}: {}\n",
            e.file
        ));
        out.push_str(&format!("It runs {}.\n", e.binary));
        if e.opened {
            out.push_str(OPENED);
            out.push('\n');
        } else {
            if let Some(error) = &e.open_error {
                out.push_str(&format!("Could not open Claude Desktop: {error}.\n"));
            }
            out.push_str(&format!(
                "To install it, open Claude Desktop, go to {INSTALL_ROUTE} and pick this file.\n"
            ));
            if e.open_error.is_some() && cfg!(windows) {
                out.push_str(DOUBLE_CLICK);
                out.push('\n');
            }
        }
    }
    for notice in notices {
        out.push_str(notice);
        out.push('\n');
    }
    out.push_str(FOOTER);
    out.push('\n');
    out
}
