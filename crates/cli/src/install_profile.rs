//! Install and uninstall for the profile harnesses (Cursor, Kiro, Gemini CLI
//! and Qwen Code): everything their profile row describes, written once.
//!
//! The legacy three keep their own path in [`crate::install`]; `run_install`
//! and `run_uninstall` there hand a profile harness over to this module.
//! Every file here is edited span by span ([`crystalline_core::jsonc_edit`]),
//! so the person's other entries, comments and layout come back byte for
//! byte, and every file is read fresh right before its own edit: Gemini CLI
//! and Qwen Code keep the MCP entry and the hook in one settings file, and a
//! stale read would undo the first edit with the second.
//!
//! A part that cannot be done (a file that does not parse, a value of an
//! unexpected type) is reported as `failed` with the reason and the file is
//! left unchanged; the other parts still run, and the receipt does not
//! record the failed part, because the MCP gate reads it.

use std::path::{Path, PathBuf};

use crystalline_core::harness::pointer;
use crystalline_core::jsonc_edit::{self, Edit, JsonIn, JsoncError, SegBuf};
use crystalline_core::{HarnessKind, HookDialect, McpStyle, PathSpec, PointerStyle, config};
use serde_json::Value;

use crate::harness_command::resolve_program;
use crate::harness_files::{read_text, save_edited};
use crate::hook_dialect;
use crate::install::{
    self, HooksReport, InstallOptions, InstallReport, McpReport, PointerReport, ReceiptReport,
    ReconcileMode, SkillsReport, UninstallReport,
};
use crate::mcp_json;
use crate::receipt::{self, InstallRecord, Receipt};
use crate::skills_placement::{self, Folders, Placement};

/// One run for one harness: the roots every profile path resolves under,
/// where backups go, and what the run collects beside the parts (the
/// backups it took and the notices to print). Tests build one over
/// temporary folders.
struct Run {
    harness: HarnessKind,
    home: PathBuf,
    copilot_home: PathBuf,
    state_dir: Option<PathBuf>,
    backups: Vec<String>,
    notices: Vec<String>,
}

impl Run {
    fn new(harness: HarnessKind) -> Run {
        let (home, copilot_home) = skills_placement::real_roots();
        Run {
            harness,
            home,
            copilot_home,
            state_dir: config::state_dir().ok(),
            backups: Vec::new(),
            notices: Vec::new(),
        }
    }

    /// A profile path under this run's roots.
    fn at(&self, spec: PathSpec) -> PathBuf {
        spec.resolve_in(&self.home, &self.copilot_home)
    }

    fn folders(&self) -> Folders<'_> {
        Folders {
            home: &self.home,
            copilot_home: &self.copilot_home,
        }
    }

    /// The file the session hook lives in.
    fn hook_file(&self) -> Option<PathBuf> {
        self.harness.profile().hooks.file().map(|f| self.at(f))
    }

    /// The file a JSON-entry MCP style writes.
    fn mcp_file(&self) -> Option<PathBuf> {
        match self.harness.profile().mcp {
            McpStyle::JsonEntry { file, .. } => Some(self.at(file)),
            McpStyle::Legacy | McpStyle::Cli { .. } => None,
        }
    }

    /// The owned pointer file, when the profile has one.
    fn pointer_file(&self) -> Option<PathBuf> {
        match self.harness.profile().pointer {
            PointerStyle::OwnedFile { path, .. } => Some(self.at(path)),
            PointerStyle::None => None,
        }
    }

    /// Write `after` over `before` with a backup of a file the person had,
    /// and remember the backup for the report.
    fn save(&mut self, path: &Path, before: Option<&str>, after: &str) -> anyhow::Result<bool> {
        let written = save_edited(self.state_dir.as_deref(), self.harness, path, before, after)?;
        if let Some(backup) = &written.backup {
            self.backups.push(backup.display().to_string());
        }
        Ok(written.changed)
    }
}

/// Refuse `--project` before anything is read or written: the profile
/// harnesses support user scope only.
fn refuse_project(harness: HarnessKind, project: bool) -> anyhow::Result<()> {
    if project {
        anyhow::bail!(
            "project scope is not supported for {} yet; run without --project",
            harness.display_name()
        );
    }
    Ok(())
}

/// The text of a file and its parsed value: `Ok(None)` for a file that does
/// not exist, `Err` with a sentence naming the path for one that cannot be
/// read or parsed.
fn read_parsed(path: &Path) -> Result<Option<(String, Value)>, String> {
    let text = read_text(path).map_err(|e| e.to_string())?;
    match text {
        None => Ok(None),
        Some(text) => match jsonc_edit::parse_value(&text) {
            Ok(value) => Ok(Some((text, value))),
            Err(e) => Err(file_error(path, &e)),
        },
    }
}

fn file_error(path: &Path, e: &JsoncError) -> String {
    format!("{}: {e}", path.display())
}

/// The program a stored install already points the harness at: the
/// session hook's program, else the MCP entry's command. A file that cannot
/// be read gives nothing here; its own part reports the problem.
fn stored_program(run: &Run) -> Option<String> {
    let dialect = &run.harness.profile().hooks;
    let from_hook = run
        .hook_file()
        .and_then(|file| read_parsed(&file).ok().flatten())
        .and_then(|(_, root)| hook_dialect::stored_program(dialect, &root));
    from_hook.or_else(|| {
        let (_, root) = read_parsed(&run.mcp_file()?).ok().flatten()?;
        root.get("mcpServers")?
            .get("crystalline")?
            .get("command")?
            .as_str()
            .filter(|c| crate::harness_command::is_crystalline_program(c))
            .map(str::to_string)
    })
}

/// The program to write for `harness`, and the warning about it, if any.
/// `keep` is for the session-start refresh only: it keeps a stored absolute
/// path that still runs, because a GUI app's PATH there may lack crystalline
/// (spec decision 6). An explicit install always takes the crystalline on
/// PATH (or this binary), so it can move a harness off an old binary.
fn program_for(run: &Run, keep: bool) -> (String, Option<String>) {
    let harness = run.harness;
    let keep = if keep { stored_program(run) } else { None };
    let path = std::env::var_os("PATH");
    let resolved = resolve_program(
        harness,
        harness.profile().command_spelling,
        path.as_deref(),
        keep.as_deref(),
    );
    (resolved.program, resolved.warning)
}

// --- MCP ---------------------------------------------------------------------

fn mcp_report(status: &'static str, path: &Path) -> McpReport {
    let mut report = McpReport::new(status, None);
    report.path = Some(path.display().to_string());
    report
}

/// Write or repair the MCP entry. The report's status is `failed` when the
/// file could not be edited; the file is then unchanged.
fn install_mcp(run: &mut Run, program: &str) -> McpReport {
    let harness = run.harness;
    let Some(path) = run.mcp_file() else {
        return McpReport::new("failed", None);
    };
    let failed = |error: String| {
        let mut report = mcp_report("failed", &path);
        report.manual_command = Some(mcp_json::manual_hint(harness, program, &path));
        report.error = Some(error);
        report
    };
    let (text, root) = match read_parsed(&path) {
        Ok(Some((text, root))) => (Some(text), Some(root)),
        Ok(None) => (None, None),
        Err(e) => return failed(e),
    };
    let (state, edits) = match mcp_json::plan_mcp_install(harness, root.as_ref(), program) {
        Ok(planned) => planned,
        Err(e) => return failed(format!("{}: {e}", path.display())),
    };
    let mut report = mcp_report(mcp_json::install_status(state), &path);
    if state == mcp_json::EntryState::Customised {
        report.manual_command = Some(mcp_json::manual_hint(harness, program, &path));
        return report;
    }
    match jsonc_edit::apply_edits(text.as_deref(), &edits) {
        Ok(None) => {}
        Ok(Some(after)) => {
            let backups = run.backups.len();
            if let Err(e) = run.save(&path, text.as_deref(), &after) {
                return failed(e.to_string());
            }
            report.backup = run.backups.get(backups).cloned();
        }
        Err(e) => return failed(file_error(&path, &e)),
    }
    report
}

/// Take the MCP entry out, when it is ours (or always, with `force`).
fn uninstall_mcp(run: &mut Run, force: bool) -> McpReport {
    let harness = run.harness;
    let Some(path) = run.mcp_file() else {
        return McpReport::new("not-present", None);
    };
    let failed = |error: String| {
        let mut report = mcp_report("failed", &path);
        report.error = Some(error);
        report
    };
    let (text, root) = match read_parsed(&path) {
        Ok(Some(found)) => found,
        Ok(None) => return mcp_report("not-present", &path),
        Err(e) => return failed(e),
    };
    let (state, edits) = match mcp_json::plan_mcp_uninstall(harness, &root, force) {
        Ok(planned) => planned,
        Err(e) => return failed(format!("{}: {e}", path.display())),
    };
    let mut report = mcp_report(mcp_json::uninstall_status(state, force), &path);
    match jsonc_edit::apply_edits(Some(&text), &edits) {
        Ok(None) => {}
        // A shared file is never deleted, even one install created (spec
        // decision 10): it stays as what the removal leaves.
        Ok(Some(after)) => {
            let backups = run.backups.len();
            if let Err(e) = run.save(&path, Some(&text), &after) {
                return failed(e.to_string());
            }
            report.backup = run.backups.get(backups).cloned();
        }
        Err(e) => return failed(file_error(&path, &e)),
    }
    report
}

// --- the session hook ----------------------------------------------------------

/// What the hook part did, beside its report.
struct HookOutcome {
    report: HooksReport,
    /// The hook of ours is in the file after this run.
    installed: bool,
    /// Why the hook could not be installed or removed.
    error: Option<String>,
}

fn hook_outcome(path: &Path, status: &'static str, written: bool, installed: bool) -> HookOutcome {
    HookOutcome {
        report: HooksReport {
            path: path.display().to_string(),
            session_start: status,
            stop: None,
            prompt: None,
            written,
        },
        installed,
        error: None,
    }
}

fn hook_failed(path: &Path, error: String) -> HookOutcome {
    HookOutcome {
        error: Some(error),
        ..hook_outcome(path, "failed", false, false)
    }
}

/// Add the session hook running `program`, or rewrite an older spelling of
/// ours in place.
fn install_hook(run: &mut Run, program: &str) -> HookOutcome {
    let harness = run.harness;
    let dialect = &harness.profile().hooks;
    let Some(path) = run.hook_file() else {
        return hook_failed(
            Path::new(""),
            "this harness has no session hook".to_string(),
        );
    };
    let command = hook_dialect::session_command(harness, program);
    let text = match read_text(&path) {
        Ok(text) => text,
        Err(e) => return hook_failed(&path, e.to_string()),
    };
    // A missing or blank file starts as the dialect's skeleton (Cursor's
    // `"version": 1`, Kiro's owned file), set key by key so a new file comes
    // out exactly as the skeleton plus our entry.
    let blank = text.as_deref().is_none_or(|t| t.trim().is_empty());
    let (root, mut edits) = if blank {
        match hook_dialect::new_file_skeleton(dialect) {
            Some(JsonIn::Object(members)) => {
                let root = JsonIn::Object(members.clone()).to_value();
                let edits = members
                    .into_iter()
                    .map(|(key, value)| Edit::Set {
                        path: vec![SegBuf::Key(key)],
                        value,
                    })
                    .collect();
                (root, edits)
            }
            _ => (Value::Object(serde_json::Map::new()), Vec::new()),
        }
    } else {
        match jsonc_edit::parse_value(text.as_deref().unwrap_or_default()) {
            Ok(root) => (root, Vec::new()),
            Err(e) => return hook_failed(&path, file_error(&path, &e)),
        }
    };
    let present = hook_dialect::session_hook_present(dialect, &root);
    let planned = hook_dialect::plan_install(dialect, &root, &command);
    if planned.is_empty() && !present {
        return hook_failed(
            &path,
            format!(
                "{}: the hooks there are not in the shape this install expects, so the session hook was not added. The file is unchanged",
                path.display()
            ),
        );
    }
    edits.extend(planned);
    let written = match jsonc_edit::apply_edits(text.as_deref(), &edits) {
        Ok(None) => false,
        Ok(Some(after)) => match run.save(&path, text.as_deref(), &after) {
            Ok(changed) => changed,
            Err(e) => return hook_failed(&path, e.to_string()),
        },
        Err(e) => return hook_failed(&path, file_error(&path, &e)),
    };
    let status = if present { "already-present" } else { "added" };
    hook_outcome(&path, status, written, true)
}

/// Take every session hook of ours out. A shared file is never deleted;
/// Kiro's owned file goes when nothing but its skeleton is left.
fn uninstall_hook(run: &mut Run) -> HookOutcome {
    let dialect = &run.harness.profile().hooks;
    let Some(path) = run.hook_file() else {
        return hook_outcome(Path::new(""), "absent", false, false);
    };
    let (text, root) = match read_parsed(&path) {
        Ok(Some(found)) => found,
        Ok(None) => return hook_outcome(&path, "absent", false, false),
        Err(e) => return hook_failed(&path, e),
    };
    let present = hook_dialect::session_hook_present(dialect, &root);
    let edits = hook_dialect::plan_uninstall(dialect, &root);
    let after = match jsonc_edit::apply_edits(Some(&text), &edits) {
        Ok(after) => after,
        Err(e) => return hook_failed(&path, file_error(&path, &e)),
    };
    let left = after.as_deref().unwrap_or(&text);
    let owned_and_empty = matches!(dialect, HookDialect::KiroOwned { .. })
        && jsonc_edit::parse_value(left)
            .is_ok_and(|v| v == hook_dialect::kiro_skeleton().to_value());
    let written = if owned_and_empty {
        match remove_owned_file(&path) {
            Ok(()) => true,
            Err(e) => return hook_failed(&path, e.to_string()),
        }
    } else if let Some(after) = after {
        match run.save(&path, Some(&text), &after) {
            Ok(changed) => changed,
            Err(e) => return hook_failed(&path, e.to_string()),
        }
    } else {
        false
    };
    let status = if present { "removed" } else { "absent" };
    hook_outcome(&path, status, written, false)
}

/// Delete a file Crystalline owns by name, and its folder when that leaves
/// it empty.
fn remove_owned_file(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(anyhow::anyhow!("could not remove {}: {e}", path.display())),
    }
    if let Some(dir) = path.parent()
        && let Ok(mut entries) = std::fs::read_dir(dir)
        && entries.next().is_none()
    {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

/// The hook part on its own, for the `HooksStyle::Profile` arm of
/// [`install::hooks_style`] callers: a failure is an error.
pub(crate) fn install_hook_report(harness: HarnessKind) -> anyhow::Result<HooksReport> {
    let mut run = Run::new(harness);
    let (program, _) = program_for(&run, false);
    let outcome = install_hook(&mut run, &program);
    match outcome.error {
        Some(e) => Err(anyhow::anyhow!(e)),
        None => Ok(outcome.report),
    }
}

/// The hook removal on its own, for the `HooksStyle::Profile` arm.
pub(crate) fn uninstall_hook_report(harness: HarnessKind) -> anyhow::Result<HooksReport> {
    let mut run = Run::new(harness);
    let outcome = uninstall_hook(&mut run);
    match outcome.error {
        Some(e) => Err(anyhow::anyhow!(e)),
        None => Ok(outcome.report),
    }
}

// --- the pointer file ------------------------------------------------------------

/// Write the owned pointer file a profile names (Kiro's steering file). A
/// file of that name without our marker is the person's and is kept.
fn install_pointer(run: &mut Run) -> Option<PointerReport> {
    let path = run.pointer_file()?;
    let report = |status| {
        Some(PointerReport {
            path: path.display().to_string(),
            status,
        })
    };
    let wanted = pointer::steering_file_text();
    let text = match read_text(&path) {
        Ok(text) => text,
        Err(e) => {
            run.notices.push(e.to_string());
            return report("failed");
        }
    };
    match text.as_deref() {
        Some(t) if t == wanted => report("already-present"),
        Some(t) if !pointer::is_managed_steering(t) => {
            run.notices.push(format!(
                "{} has no Crystalline marker, so it was left as it is.",
                path.display()
            ));
            report("kept-modified")
        }
        _ => match run.save(&path, text.as_deref(), &wanted) {
            Ok(_) if text.is_some() => report("updated"),
            Ok(_) => report("added"),
            Err(e) => {
                run.notices.push(e.to_string());
                report("failed")
            }
        },
    }
}

/// Delete the owned pointer file when it carries our marker (or always,
/// with `force`).
fn uninstall_pointer(run: &mut Run, force: bool) -> Option<PointerReport> {
    let path = run.pointer_file()?;
    let report = |status| {
        Some(PointerReport {
            path: path.display().to_string(),
            status,
        })
    };
    let text = match read_text(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return report("absent"),
        Err(e) => {
            run.notices.push(e.to_string());
            return report("failed");
        }
    };
    let managed = pointer::is_managed_steering(&text);
    if !managed && !force {
        run.notices.push(format!(
            "Kept {}: it has no Crystalline marker. Run the uninstall with --force to remove it.",
            path.display()
        ));
        return report("kept-modified");
    }
    match remove_owned_file(&path) {
        Ok(()) if managed => report("removed"),
        Ok(()) => report("removed-forced"),
        Err(e) => {
            run.notices.push(e.to_string());
            report("failed")
        }
    }
}

// --- skills -------------------------------------------------------------------------

/// Install the skills where the harness gets them: nothing but the covered
/// marker when another harness's folder covers it, the managed set in its
/// own folder otherwise.
fn install_skills(
    harness: HarnessKind,
    book: &Receipt,
    prior: &[receipt::RecordedSkill],
    folders: &Folders<'_>,
) -> anyhow::Result<(SkillsReport, Vec<receipt::RecordedSkill>)> {
    match skills_placement::placement(harness, book, folders) {
        Placement::Covered { folder, by } => Ok((
            SkillsReport {
                dir: folder.display().to_string(),
                skills: Vec::new(),
                covered_by: Some(by.id()),
            },
            vec![skills_placement::covered_marker(by)],
        )),
        Placement::Write(dir) => install::reconcile_skills(&dir, prior, ReconcileMode::Install),
    }
}

// --- notices -------------------------------------------------------------------------

/// The version-skew notice for the program a profile harness runs. The bare
/// spelling is the legacy PATH check; an absolute one, which an install takes
/// from PATH (or this binary), is asked directly.
fn program_notice(harness: HarnessKind, program: &str) -> Option<String> {
    if program == "crystalline" {
        return install::path_binary_notice();
    }
    let mine = env!("CARGO_PKG_VERSION");
    let out = std::process::Command::new(program)
        .arg("--version")
        .output()
        .ok();
    let version = out.as_ref().filter(|o| o.status.success()).and_then(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .and_then(|l| l.trim().strip_prefix("crystalline ").map(str::to_string))
    });
    match version {
        Some(v) if v == mine => None,
        Some(v) => Some(format!(
            "{} runs {program}, the crystalline on your PATH, which is version {v}, not this binary's {mine}. Upgrade it, or put this binary first on your PATH and run the install again.",
            harness.display_name()
        )),
        None => Some(format!(
            "{} runs {program}, the crystalline on your PATH, but it did not answer --version. Check which crystalline your PATH finds and run the install again.",
            harness.display_name()
        )),
    }
}

/// The notes a harness adds after an install. `hook_done` is whether this
/// run installed the session hook.
fn harness_notes(harness: HarnessKind, book: &Receipt, hook_done: bool) -> Vec<String> {
    let mut notes = Vec::new();
    let hooked = |kind: HarnessKind| {
        book.find(kind.id(), "user", None)
            .is_some_and(|row| row.parts.hooks)
    };
    match harness {
        HarnessKind::Cursor => {
            // The Claude Code hook is silenced inside Cursor only while
            // Cursor has a hook of its own.
            if hooked(HarnessKind::ClaudeCode) && hooked(HarnessKind::Cursor) {
                notes.push(
                    "Cursor also runs your Claude Code hooks. The Claude Code routing hook stays silent inside Cursor, so the routing block arrives once."
                        .to_string(),
                );
            }
            if hook_done {
                notes.push(
                    "Cursor reads ~/.cursor/hooks.json when it starts. Restart Cursor to load the hook."
                        .to_string(),
                );
            }
        }
        HarnessKind::Kiro => {
            if hook_done {
                notes.push(
                    "Kiro CLI 2.x and custom agents may not run hooks from ~/.kiro/hooks. The steering file ~/.kiro/steering/crystalline.md points the agent at the routing block either way."
                        .to_string(),
                );
            }
        }
        HarnessKind::ClaudeCode
        | HarnessKind::Codex
        | HarnessKind::Copilot
        | HarnessKind::Gemini
        | HarnessKind::Qwen => {}
    }
    notes
}

/// Save the receipt and describe the outcome; a failure becomes a notice.
fn save_receipt(
    path: Option<&Path>,
    book: &Receipt,
    failure: &str,
    notices: &mut Vec<String>,
) -> Option<ReceiptReport> {
    let Some(path) = path else {
        notices.push(format!(
            "Could not resolve the state directory for the install receipt; {failure}"
        ));
        return None;
    };
    let written = match receipt::save(path, book) {
        Ok(()) => true,
        Err(e) => {
            notices.push(format!(
                "Could not write the install receipt ({e}). {failure}"
            ));
            false
        }
    };
    Some(ReceiptReport {
        path: path.display().to_string(),
        written,
    })
}

// --- entry points -------------------------------------------------------------------

/// `crystalline install` for a profile harness.
pub(crate) fn run_install(opts: &InstallOptions, json: bool) -> anyhow::Result<()> {
    let harness = opts.harness;
    refuse_project(harness, opts.project)?;
    let mut run = Run::new(harness);
    let receipt_path = receipt::receipt_path().ok();
    let mut book = receipt_path
        .as_deref()
        .map(|p| receipt::load(p).unwrap_or_default())
        .unwrap_or_default();
    let prior = book.find(harness.id(), "user", None).cloned();

    let (program, program_warning) = if opts.skip_mcp && opts.skip_hooks {
        (String::new(), None)
    } else {
        program_for(&run, false)
    };

    // A failed MCP part carries its reason in its own report line.
    let mcp = (!opts.skip_mcp).then(|| install_mcp(&mut run, &program));
    let hook = (!opts.skip_hooks).then(|| install_hook(&mut run, &program));
    if let Some(error) = hook.as_ref().and_then(|h| h.error.clone()) {
        run.notices
            .push(format!("The session hook was not installed: {error}."));
    }
    let pointer = install_pointer(&mut run);

    let prior_skills = prior.as_ref().map(|p| p.skills.as_slice()).unwrap_or(&[]);
    let (skills, new_records) = if opts.skip_skills {
        (None, None)
    } else {
        let (report, records) = install_skills(harness, &book, prior_skills, &run.folders())?;
        (Some(report), Some(records))
    };

    let mcp_done = mcp.as_ref().is_some_and(|m| m.status != "failed");
    let hook_done = hook.as_ref().is_some_and(|h| h.installed);
    let prior_parts = prior.as_ref().map(|p| p.parts).unwrap_or(receipt::Parts {
        mcp: false,
        hooks: false,
        skills: false,
    });
    let parts = receipt::Parts {
        mcp: prior_parts.mcp || mcp_done,
        hooks: prior_parts.hooks || hook_done,
        skills: prior_parts.skills || !opts.skip_skills,
    };
    // As in the legacy install: a recorded hooks or skills part this run did
    // not bring up to date keeps the earlier version, so the session-start
    // refresh still sees the difference and catches up.
    let stale = (!hook_done && parts.hooks) || (opts.skip_skills && parts.skills);
    let version = match (&prior, stale) {
        (Some(p), true) => p.version.clone(),
        _ => env!("CARGO_PKG_VERSION").to_string(),
    };
    book.upsert(InstallRecord {
        harness: harness.id().to_string(),
        scope: "user".to_string(),
        project_path: None,
        version,
        parts,
        skills: match new_records {
            Some(records) => records,
            None => prior.map(|p| p.skills).unwrap_or_default(),
        },
    });
    let (_, rebalance_notices) = skills_placement::rebalance(&mut book, &run.folders(), None);

    let mut notices = Vec::new();
    notices.extend(program_warning);
    if !program.is_empty() {
        notices.extend(program_notice(harness, &program));
    }
    if let Ok(loaded) = crystalline_service::overlay::load(None)
        && loaded.effective.domains.is_empty()
    {
        notices.push(
            "No domains registered yet. Create one with: crystalline domain add <name> <path> - or let an agent create one at runtime with the add_domain tool."
                .to_string(),
        );
    }
    notices.append(&mut run.notices);
    notices.extend(harness_notes(harness, &book, hook_done));
    notices.extend(rebalance_notices);
    let receipt_report = save_receipt(
        receipt_path.as_deref(),
        &book,
        "Session-start auto-updates will not cover this install until a later `crystalline install` succeeds.",
        &mut notices,
    );

    let report = InstallReport {
        harness: harness.id(),
        scope: "user",
        mcp,
        hooks: hook.map(|h| h.report),
        pointer,
        skills,
        receipt: receipt_report,
        backups: run.backups,
        notices,
    };
    install::print_install(&report, harness, json)
}

/// `crystalline uninstall` for a profile harness.
pub(crate) fn run_uninstall(
    harness: HarnessKind,
    project: bool,
    force: bool,
    json: bool,
) -> anyhow::Result<()> {
    refuse_project(harness, project)?;
    let mut run = Run::new(harness);
    let receipt_path = receipt::receipt_path().ok();
    let mut book = receipt_path
        .as_deref()
        .map(|p| receipt::load(p).unwrap_or_default())
        .unwrap_or_default();
    let prior = book.find(harness.id(), "user", None).cloned();

    let mcp = uninstall_mcp(&mut run, force);
    let hook = uninstall_hook(&mut run);
    if let Some(error) = &hook.error {
        run.notices
            .push(format!("The session hook was not removed: {error}."));
    }
    let pointer = uninstall_pointer(&mut run, force);

    let dir = run.at(harness.profile().skills_write);
    let prior_skills = prior.as_ref().map(|p| p.skills.as_slice()).unwrap_or(&[]);
    let mut skills = install::skills_to_remove(
        harness,
        true,
        &dir,
        prior_skills,
        force,
        &book,
        &run.folders(),
    )?;
    // A covered row wrote nothing, so nothing was removed: say whose folder
    // covered it.
    if let Some(by) = prior_skills.iter().find_map(skills_placement::covered_by) {
        skills.dir = run.at(by.profile().skills_write).display().to_string();
        skills.covered_by = Some(by.id());
    }

    let removed = book.remove(harness.id(), "user", None);
    // With this row gone, a harness it covered writes its own folder again
    // (the hand-over), seeded by the removed row's list.
    let (rebalanced, rebalance_notices) =
        skills_placement::rebalance(&mut book, &run.folders(), prior.as_ref());
    let mut notices = std::mem::take(&mut run.notices);
    notices.extend(rebalance_notices);
    let receipt_report = if removed || rebalanced {
        save_receipt(
            receipt_path.as_deref(),
            &book,
            "It may still list this install as present.",
            &mut notices,
        )
    } else {
        None
    };

    let report = UninstallReport {
        harness: harness.id(),
        scope: "user",
        mcp,
        hooks: hook.report,
        pointer,
        skills,
        receipt: receipt_report,
        backups: run.backups,
        notices,
    };
    install::print_uninstall(&report, harness, json)
}

/// The session-start refresh of one profile harness's receipt row: the
/// hook (keeping a stored absolute program that still runs, spec decision
/// 6), the pointer and the skills in `Auto` mode. Never the MCP entry, as
/// for the legacy harnesses.
pub(crate) fn reconcile(harness: HarnessKind, entry: &mut InstallRecord) -> anyhow::Result<()> {
    let mut run = Run::new(harness);
    if entry.parts.hooks {
        let (program, _) = program_for(&run, true);
        if let Some(error) = install_hook(&mut run, &program).error {
            anyhow::bail!(error);
        }
    }
    install_pointer(&mut run);
    // A covered row wrote no files: Auto mode on its marker alone would see
    // every skill missing and record an empty list, losing the marker the
    // hand-over depends on.
    if entry.parts.skills && !skills_placement::is_covered(entry) {
        let dir = run.at(harness.profile().skills_write);
        let (_, records) = install::reconcile_skills(&dir, &entry.skills, ReconcileMode::Auto)?;
        entry.skills = records;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_command::TEST_PROGRAM;

    /// A run whose every path lies under `home`.
    fn run_in(harness: HarnessKind, home: &Path) -> Run {
        Run {
            harness,
            home: home.to_path_buf(),
            copilot_home: home.join(".copilot"),
            state_dir: Some(home.join("state")),
            backups: Vec::new(),
            notices: Vec::new(),
        }
    }

    /// Kiro's hook file is ours by name: it goes, with its folder, when an
    /// uninstall leaves nothing but its skeleton.
    #[test]
    fn the_kiro_hook_file_goes_when_only_its_skeleton_is_left() {
        let tmp = tempfile::tempdir().unwrap();
        let mut run = run_in(HarnessKind::Kiro, tmp.path());
        let file = tmp.path().join(".kiro/hooks/crystalline.json");
        let added = install_hook(&mut run, TEST_PROGRAM);
        assert!(added.installed && added.error.is_none());
        assert_eq!(added.report.session_start, "added");
        let again = install_hook(&mut run, TEST_PROGRAM);
        assert_eq!(
            (again.report.session_start, again.report.written),
            ("already-present", false)
        );
        let removed = uninstall_hook(&mut run);
        assert_eq!(removed.report.session_start, "removed");
        assert!(!file.exists(), "the owned file is deleted");
        assert!(!file.parent().unwrap().exists(), "and its emptied folder");
        assert!(
            run.backups.is_empty(),
            "nothing of the person's was changed"
        );
    }

    #[test]
    fn a_hook_a_person_added_to_the_kiro_file_keeps_it() {
        let tmp = tempfile::tempdir().unwrap();
        let mut run = run_in(HarnessKind::Kiro, tmp.path());
        let file = tmp.path().join(".kiro/hooks/crystalline.json");
        install_hook(&mut run, TEST_PROGRAM);
        let text = std::fs::read_to_string(&file).unwrap();
        let mut value: Value = jsonc_edit::parse_value(&text).unwrap();
        value["hooks"].as_array_mut().unwrap().push(serde_json::json!(
            {"name": "mine", "trigger": "AgentStop", "action": {"type": "command", "command": "echo hi"}}
        ));
        std::fs::write(&file, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        uninstall_hook(&mut run);
        let left = jsonc_edit::parse_value(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(left["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(left["hooks"][0]["name"], "mine");
    }

    /// The steering file is written whole, and only a file carrying our
    /// marker is ever replaced or removed without `--force`.
    #[test]
    fn the_steering_file_is_written_and_removed_by_its_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let mut run = run_in(HarnessKind::Kiro, tmp.path());
        let file = tmp.path().join(".kiro/steering/crystalline.md");
        assert_eq!(install_pointer(&mut run).unwrap().status, "added");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            pointer::steering_file_text()
        );
        assert_eq!(install_pointer(&mut run).unwrap().status, "already-present");
        assert_eq!(
            uninstall_pointer(&mut run, false).unwrap().status,
            "removed"
        );
        assert!(!file.exists());
        assert_eq!(uninstall_pointer(&mut run, false).unwrap().status, "absent");

        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "my own notes\n").unwrap();
        assert_eq!(install_pointer(&mut run).unwrap().status, "kept-modified");
        assert_eq!(
            uninstall_pointer(&mut run, false).unwrap().status,
            "kept-modified"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "my own notes\n");
        assert_eq!(
            uninstall_pointer(&mut run, true).unwrap().status,
            "removed-forced"
        );
        assert!(!file.exists());
    }

    #[test]
    fn a_harness_without_a_pointer_reports_none() {
        let tmp = tempfile::tempdir().unwrap();
        let mut run = run_in(HarnessKind::Cursor, tmp.path());
        assert!(install_pointer(&mut run).is_none());
        assert!(uninstall_pointer(&mut run, true).is_none());
    }

    /// Gemini CLI keeps the MCP entry and the hook in one settings file:
    /// each part reads it fresh, so the second write keeps the first.
    #[test]
    fn two_parts_in_one_settings_file_both_land_and_both_leave() {
        let tmp = tempfile::tempdir().unwrap();
        let mut run = run_in(HarnessKind::Gemini, tmp.path());
        let file = tmp.path().join(".gemini/settings.json");
        let original = "{\n  \"theme\": \"dark\"\n}\n";
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, original).unwrap();
        assert_eq!(install_mcp(&mut run, "crystalline").status, "registered");
        assert!(install_hook(&mut run, "crystalline").installed);
        let value = jsonc_edit::parse_value(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(value["mcpServers"]["crystalline"].is_object(), "{value}");
        assert!(value["hooks"]["SessionStart"].is_array(), "{value}");
        assert_eq!(run.backups.len(), 1, "one backup per path");
        assert_eq!(uninstall_mcp(&mut run, false).status, "removed");
        assert_eq!(uninstall_hook(&mut run).report.session_start, "removed");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    }
}
