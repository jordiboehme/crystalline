//! `crystalline install claude-desktop`: build the hollow Claude Desktop
//! extension on this machine and record what was built.
//!
//! Claude Desktop is not a coding harness: it has no hooks, no skill folder
//! Crystalline can write into and no per-project config. So it is not a
//! [`crystalline_core::HarnessKind`] and gets this module of its own. The
//! extension is the launcher-only bundle from
//! [`crystalline_core::desktop_extension`], with the crystalline that
//! `resolve_program` finds on PATH (or this binary) pinned in its manifest.
//! It is written to `<state_dir>/claude-desktop/crystalline.mcpb` and
//! remembered in `<state_dir>/claude-desktop.json` (`desktop_record.rs`).
//!
//! Crystalline cannot see what Claude Desktop installed, so every run builds
//! the extension again and the record says only what this command did last.

use anyhow::Context;
use serde_json::{Value, json};

use crystalline_core::CommandSpelling;
use crystalline_core::desktop_extension::{Bundle, files, pack, shim_sha256};

use crate::desktop_record::{self, ExtensionRecord};
use crate::harness_command::resolve_program;

/// The Crystalline version this binary builds the extension for.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where Claude Desktop installs an extension from a file.
pub(crate) const INSTALL_ROUTE: &str =
    "Settings > Extensions > Advanced settings > Install Extension...";

/// The last line of every human run.
const FOOTER: &str =
    "Crystalline cannot see what Claude Desktop has installed. It only remembers what it did last.";

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
    #[expect(dead_code, reason = "the opener that reads it is not written yet")]
    pub open: bool,
}

/// What a run did with the extension.
struct BuiltExtension {
    file: String,
    binary: String,
    opened: bool,
    open_error: Option<String>,
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
    // A record this binary cannot read is no record: it is rewritten whole.
    let mut record = desktop_record::load(&record_path)
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut notices = Vec::new();
    let mut extension = None;

    if !opts.skip_extension {
        let path = std::env::var_os("PATH");
        let resolved = resolve_program(
            "Claude Desktop",
            CommandSpelling::AbsolutePathEntry,
            path.as_deref(),
            None,
        );
        notices.extend(resolved.warning);
        let file = desktop_record::bundle_path()?;
        let zip = pack(&files(
            Bundle::Hollow {
                binary: Some(&resolved.program),
            },
            VERSION,
        ))
        .context("could not pack the Claude Desktop extension")?;
        desktop_record::write_replacing(&file, &zip)?;
        let built = BuiltExtension {
            file: file.display().to_string(),
            binary: resolved.program,
            opened: false,
            open_error: None,
        };
        record.extension = Some(ExtensionRecord {
            version: VERSION.to_string(),
            binary: built.binary.clone(),
            shim_sha256: shim_sha256(),
            file: built.file.clone(),
            built_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            opened: built.opened,
        });
        extension = Some(built);
    }

    if extension.is_some() {
        desktop_record::save(&record_path, &record)?;
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
        if !e.opened {
            out.push_str(&format!(
                "To install it, open Claude Desktop, go to {INSTALL_ROUTE} and pick this file.\n"
            ));
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
