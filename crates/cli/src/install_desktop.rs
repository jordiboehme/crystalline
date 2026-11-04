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
//! Then the crystalline-intelligence skill is saved as a zip for Claude
//! Desktop's skill upload: into the person's Downloads folder, or the state
//! folder beside the extension when there is none, never over a file that
//! is already there. Unless `--no-open` is given, the zip is shown in Finder
//! or File Explorer. A skill that has not changed since the last save is not
//! saved again. The routing text the server sends tells the agent to read
//! the `skills` tool as well, so Claude Desktop gets the current playbooks
//! even when no skill was uploaded.
//!
//! Crystalline cannot see what Claude Desktop installed, so every run builds
//! the extension again and the record says only what this command did last.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crystalline_core::desktop_extension::{sha256_hex, skill_zip};
use crystalline_core::desktop_record::{self, DesktopRecord, SkillRecord};
use crystalline_service::desktop_offer::{BuiltExtension, build_extension};
use crystalline_service::desktop_open;

/// The Crystalline version this binary builds the extension for.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where Claude Desktop installs an extension from a file.
pub(crate) const INSTALL_ROUTE: &str =
    "Settings > Extensions > Advanced settings > Install Extension...";

/// Where Claude Desktop takes a skill upload.
pub(crate) const SKILLS_ROUTE: &str = "Customize > Skills";

/// The skill a run saves for Claude Desktop, which is also the zip's name.
const SKILL_NAME: &str = "crystalline-intelligence";

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

/// The refusal of `uninstall --force`.
pub(crate) const FORCE_REFUSAL: &str = "--force does not apply to claude-desktop.";

/// Where a person removes an extension in Claude Desktop.
const REMOVE_ROUTE: &str = "Settings > Extensions";

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
    let opener = opts.open.then(desktop_open::for_this_process);

    if !opts.skip_extension {
        let built = build_extension(&mut record, None, opener.as_deref())?;
        notices.extend(built.warning.clone());
        extension = Some(built);
    }

    let mut skill = None;
    if !opts.skip_skill {
        match save_skill(&mut record, opener.as_deref()) {
            Ok(saved) => {
                notices.extend(
                    saved
                        .reveal_error
                        .as_ref()
                        .map(|error| format!("Could not show the file: {error}.")),
                );
                skill = Some(saved);
            }
            // Nothing else was done, so the run fails.
            Err(e) if extension.is_none() => return Err(e),
            Err(e) => notices.push(format!("{e}.")),
        }
    }

    // The extension is built (or the skill saved) by now, so a record that
    // cannot be saved is a notice and the run still exits 0: the person can
    // install the file.
    let changed = extension.is_some() || skill.as_ref().is_some_and(|s| s.status == SAVED);
    if changed {
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
        println!(
            "{}",
            report_json(extension.as_ref(), skill.as_ref(), &notices)
        );
    } else {
        print!(
            "{}",
            report_text(extension.as_ref(), skill.as_ref(), &notices)
        );
    }
    Ok(())
}

/// Run `crystalline uninstall claude-desktop`: delete the built extension,
/// its leftovers and Crystalline's record, then say where to finish in
/// Claude Desktop. The skill zip is left alone, and so is the install
/// receipt (Claude Desktop is not in it).
pub(crate) fn run_uninstall(json: bool) -> anyhow::Result<()> {
    if !cfg!(any(target_os = "macos", windows)) {
        anyhow::bail!(LINUX_REFUSAL);
    }
    let bundle = desktop_record::bundle_path()?;
    let record_path = desktop_record::record_path()?;

    let removed_file = remove_if_there(&bundle)?.then(|| bundle.display().to_string());
    for leftover in [
        with_suffix(&bundle, ".previous"),
        with_suffix(&bundle, ".partial"),
        with_suffix(&record_path, ".partial"),
        crystalline_service::desktop_offer::lock_path()?,
    ] {
        remove_if_there(&leftover)?;
    }
    let removed_record = remove_if_there(&record_path)?;
    // The folder goes only when nothing else is in it.
    if let Some(folder) = bundle.parent() {
        let _ = std::fs::remove_dir(folder);
    }

    if json {
        println!(
            "{}",
            json!({
                "target": "claude-desktop",
                "removed_file": removed_file,
                "removed_record": removed_record,
            })
        );
        return Ok(());
    }
    match &removed_file {
        Some(file) => {
            println!("Removed the built extension ({file}) and Crystalline's record of it.")
        }
        None if removed_record => {
            println!("Removed Crystalline's record of the Claude Desktop install.")
        }
        None => println!("Crystalline has no record of a Claude Desktop install on this computer."),
    }
    println!(
        "Crystalline cannot remove the extension from Claude Desktop. Remove it there: {REMOVE_ROUTE}, then Crystalline."
    );
    println!("The skill stays in Claude Desktop until you delete it under {SKILLS_ROUTE}.");
    Ok(())
}

/// `path` with `suffix` appended to its last component.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Delete a file; whether there was one.
fn remove_if_there(path: &Path) -> anyhow::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => anyhow::bail!("Could not remove {}: {e}", path.display()),
    }
}

/// [`SavedSkill::status`] when the zip was written (or an identical one was
/// found) this run.
const SAVED: &str = "saved";

/// [`SavedSkill::status`] when the recorded zip still holds this skill.
const UNCHANGED: &str = "unchanged";

/// What the skill step did.
struct SavedSkill {
    /// The zip, written or found.
    file: PathBuf,
    /// [`SAVED`] or [`UNCHANGED`].
    status: &'static str,
    /// Whether the zip was shown in Finder or File Explorer.
    revealed: bool,
    /// Why showing it failed, when it did.
    reveal_error: Option<String>,
}

/// Save the crystalline-intelligence skill as a zip and remember it in
/// `record`, unless the recorded zip still holds this skill. A saved zip is
/// shown through `opener` (none with `--no-open`); a failed show is the
/// caller's notice, never an error.
fn save_skill(
    record: &mut DesktopRecord,
    opener: Option<&dyn desktop_open::DesktopOpener>,
) -> anyhow::Result<SavedSkill> {
    let asset = crystalline_core::skill(SKILL_NAME)
        .ok_or_else(|| anyhow::anyhow!("this crystalline ships no {SKILL_NAME} skill"))?;
    let sha256 = sha256_hex(asset.content.as_bytes());
    if let Some(known) = &record.skill
        && known.sha256 == sha256
        && Path::new(&known.file).is_file()
    {
        return Ok(SavedSkill {
            file: PathBuf::from(&known.file),
            status: UNCHANGED,
            revealed: false,
            reveal_error: None,
        });
    }

    let bytes = skill_zip(asset)?;
    let state = desktop_record::bundle_path()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("the state folder has no parent"))?;
    let written = match downloads_dir() {
        Some(downloads) => skill_target(&downloads, &bytes).or_else(|_| {
            std::fs::create_dir_all(&state)?;
            skill_target(&state, &bytes)
        }),
        None => std::fs::create_dir_all(&state).and_then(|()| skill_target(&state, &bytes)),
    };
    let (file, _reused) =
        written.map_err(|e| anyhow::anyhow!("Could not save the {SKILL_NAME} skill: {e}"))?;

    record.skill = Some(SkillRecord {
        name: SKILL_NAME.to_string(),
        sha256,
        file: file.display().to_string(),
        saved_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    let shown = opener.map(|opener| opener.reveal(&file));
    Ok(SavedSkill {
        file,
        status: SAVED,
        revealed: matches!(shown, Some(Ok(()))),
        reveal_error: shown.and_then(Result::err),
    })
}

/// The person's Downloads folder: `$HOME/Downloads` on macOS,
/// `%USERPROFILE%\Downloads` on Windows. `None` when the variable is unset
/// or the folder does not exist. No known-folder lookup, so a test can point
/// it elsewhere through the environment.
fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    if home.is_empty() {
        return None;
    }
    let dir = PathBuf::from(home).join("Downloads");
    dir.is_dir().then_some(dir)
}

/// How many names [`skill_target`] tries before it gives up.
const TARGET_LIMIT: usize = 1000;

/// The first of `crystalline-intelligence.zip`, `crystalline-intelligence
/// 1.zip`, `crystalline-intelligence 2.zip` and so on in `dir` that either
/// does not exist (it is written, created new) or holds exactly `bytes` (it
/// is reused; the flag says so). A file that is already there is never
/// overwritten.
fn skill_target(dir: &Path, bytes: &[u8]) -> std::io::Result<(PathBuf, bool)> {
    for n in 0..TARGET_LIMIT {
        let name = match n {
            0 => format!("{SKILL_NAME}.zip"),
            n => format!("{SKILL_NAME} {n}.zip"),
        };
        let path = dir.join(name);
        match std::fs::read(&path) {
            Ok(found) if found == bytes => return Ok((path, true)),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // A folder or an unreadable file by that name: not ours to touch.
            Err(_) if path.exists() => continue,
            Err(e) => return Err(e),
        }
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            // Someone made it since the read: try the next name.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        return Ok((path, false));
    }
    Err(std::io::Error::other(format!(
        "{TARGET_LIMIT} files named {SKILL_NAME} are already in {}",
        dir.display()
    )))
}

/// The one JSON object `--json` prints.
fn report_json(
    extension: Option<&BuiltExtension>,
    skill: Option<&SavedSkill>,
    notices: &[String],
) -> Value {
    json!({
        "target": "claude-desktop",
        "version": VERSION,
        "extension": extension.map(|e| json!({
            "file": e.file,
            "binary": e.binary,
            "opened": e.opened,
            "open_error": e.open_error,
        })),
        "skill": skill.map(|s| json!({
            "name": SKILL_NAME,
            "file": s.file.display().to_string(),
            "status": s.status,
            "revealed": s.revealed,
        })),
        "notices": notices,
    })
}

/// The human report, one line each, the footer last.
fn report_text(
    extension: Option<&BuiltExtension>,
    skill: Option<&SavedSkill>,
    notices: &[String],
) -> String {
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
    if let Some(s) = skill {
        let file = s.file.display();
        if s.status == UNCHANGED {
            out.push_str(&format!(
                "The {SKILL_NAME} skill has not changed since you saved it: {file}\n"
            ));
        } else {
            out.push_str(&format!("Saved the {SKILL_NAME} skill: {file}\n"));
            out.push_str(&format!(
                "Upload it in Claude Desktop under {SKILLS_ROUTE} and turn it on. Do not unpack the zip.\n"
            ));
            out.push_str("If you uploaded the older crystalline-memory skill, delete it there.\n");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_free_name_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let (path, reused) = skill_target(dir.path(), b"zip").unwrap();
        assert_eq!(path, dir.path().join("crystalline-intelligence.zip"));
        assert!(!reused);
        assert_eq!(std::fs::read(&path).unwrap(), b"zip");
    }

    #[test]
    fn a_file_with_the_same_bytes_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("crystalline-intelligence.zip"), b"other").unwrap();
        std::fs::write(dir.path().join("crystalline-intelligence 1.zip"), b"zip").unwrap();
        let (path, reused) = skill_target(dir.path(), b"zip").unwrap();
        assert_eq!(path, dir.path().join("crystalline-intelligence 1.zip"));
        assert!(reused);
        assert_eq!(
            std::fs::read(dir.path().join("crystalline-intelligence.zip")).unwrap(),
            b"other"
        );
    }

    #[test]
    fn a_folder_by_the_name_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("crystalline-intelligence.zip")).unwrap();
        let (path, reused) = skill_target(dir.path(), b"zip").unwrap();
        assert_eq!(path, dir.path().join("crystalline-intelligence 1.zip"));
        assert!(!reused);
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(skill_target(&dir.path().join("gone"), b"zip").is_err());
    }
}
