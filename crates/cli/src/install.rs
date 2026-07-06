//! `crystalline install` / `crystalline uninstall`: one idempotent step that
//! wires a coding harness up to Crystalline, and its exact reverse.
//!
//! A full setup is three parts: register the MCP server, install the
//! `SessionStart` routing hook and the `Stop` capture-nudge hook and copy the
//! four topical skills into the harness's skill folder. `install` does all
//! three (each skippable with `--skip-mcp`/`--skip-hooks`/`--skip-skills`);
//! `uninstall` takes them back out. Both are static: no database, service or
//! daemon connection, and no Tokio runtime, exactly like `verify`, `prompt`
//! and `hook`.
//!
//! The one hard rule everything here bends to is that foreign data is sacred.
//! A harness settings file is the user's, shared with every other tool that
//! writes hooks into it. Any hook entry, key or structure that is not exactly
//! Crystalline's own must survive both install and uninstall unchanged in
//! meaning. So the JSON edit parses the whole file to a value, merges or
//! removes only the managed entries, and writes back only when something
//! actually changed: a second install re-runs to a byte-identical no-op, and
//! a file this process cannot parse is a hard error naming the path, never an
//! overwrite. When a write is genuinely needed the output is pretty-printed
//! with a trailing newline, so foreign keys are preserved semantically even
//! though their original byte formatting is not.
//!
//! Presence is decided by command string, ignoring the hook group's matcher,
//! so the README's hand-written `startup` recipe counts as already installed
//! (no duplicate is added) and is removed on uninstall like any managed
//! entry. The MCP registration shells out to the harness's own CLI (`claude`
//! or `codex`); a missing or failing CLI is never fatal - it prints the
//! command to run by hand and the rest of the install still proceeds.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crystalline_core::config;

/// The command the `SessionStart` hook runs: re-inject the knowledge routing
/// prompt at session start.
pub(crate) const SESSION_START_COMMAND: &str = "crystalline prompt system";

/// The command the `Stop` hook runs: the once-per-session capture nudge
/// decided by [`crate::hook`].
pub(crate) const STOP_COMMAND: &str = "crystalline hook stop";

/// The `SessionStart` matcher: re-route on a fresh start, after `/clear` and
/// after a compaction. `resume` is deliberately excluded, since a resumed
/// transcript already carries the earlier routing block.
const SESSION_START_MATCHER: &str = "startup|clear|compact";

/// How long, in seconds, a harness waits for either hook command before
/// giving up on it. Both hooks finish in tens of milliseconds, so ten seconds
/// is generous headroom, not a real budget.
const HOOK_TIMEOUT_SECS: u64 = 10;

/// The topical skills copied into a harness for both Claude Code and Codex,
/// each as `(folder name, embedded SKILL.md)`. Embedded with `include_str!`
/// so the binary is self-contained and an install from a downloaded release
/// carries the same skills a clone would. `crystalline-memory` is deliberately
/// absent: it is the single consolidated skill for Claude Desktop, which has
/// no hooks and installs one skill at a time.
pub(crate) const MANAGED_SKILLS: &[(&str, &str)] = &[
    (
        "crystalline-routing",
        include_str!("../../../skills/crystalline-routing/SKILL.md"),
    ),
    (
        "crystalline-capture",
        include_str!("../../../skills/crystalline-capture/SKILL.md"),
    ),
    (
        "crystalline-schema",
        include_str!("../../../skills/crystalline-schema/SKILL.md"),
    ),
    (
        "crystalline-collaboration",
        include_str!("../../../skills/crystalline-collaboration/SKILL.md"),
    ),
];

/// Which harness `install`/`uninstall` targets. The `clap::ValueEnum`
/// spellings are `claude-code` and `codex`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum HarnessKind {
    /// Anthropic's Claude Code CLI: hooks in `settings.json`, skills in a
    /// `skills` folder under `.claude`.
    ClaudeCode,
    /// The Codex CLI: hooks in a dedicated `hooks.json`, skills under
    /// `.agents/skills`.
    Codex,
}

impl HarnessKind {
    /// The stable identifier used in machine-readable output, and reused by
    /// `doctor` to label its harnesses section with the same spelling
    /// `crystalline install <name>` takes.
    pub(crate) fn id(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude-code",
            HarnessKind::Codex => "codex",
        }
    }

    /// The human-facing product name.
    fn display_name(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "Claude Code",
            HarnessKind::Codex => "Codex",
        }
    }

    /// The CLI binary that owns this harness's MCP registration.
    fn cli(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude",
            HarnessKind::Codex => "codex",
        }
    }
}

/// The knobs `install` reads. `uninstall` takes its two flags directly, since
/// it has no skip options.
pub struct InstallOptions {
    /// Which harness to wire up.
    pub harness: HarnessKind,
    /// Target the current repository's harness config instead of this user's
    /// global one.
    pub project: bool,
    /// Skip the MCP registration.
    pub skip_mcp: bool,
    /// Skip the `SessionStart` and `Stop` hooks.
    pub skip_hooks: bool,
    /// Skip copying the topical skills.
    pub skip_skills: bool,
}

/// The settings file and skills folder for a harness at a given scope. Reused
/// by `doctor`, which reads these same two paths to report each harness's
/// onboarding trace without ever writing to them.
pub(crate) struct HarnessPaths {
    /// The JSON file the hooks live in (`settings.json` for Claude Code, a
    /// dedicated `hooks.json` for Codex).
    pub(crate) settings: PathBuf,
    /// The folder each skill's `<name>/SKILL.md` is copied under.
    pub(crate) skills_dir: PathBuf,
}

/// Resolve the settings file and skills folder for a harness and scope. User
/// scope expands `~` through [`config::expand_tilde`]; `--project` scope is
/// relative to the current working directory, so it lands in the repository
/// the command is run from.
pub(crate) fn harness_paths(harness: HarnessKind, project: bool) -> HarnessPaths {
    match (harness, project) {
        (HarnessKind::ClaudeCode, false) => HarnessPaths {
            settings: config::expand_tilde("~/.claude/settings.json"),
            skills_dir: config::expand_tilde("~/.claude/skills"),
        },
        (HarnessKind::ClaudeCode, true) => HarnessPaths {
            settings: PathBuf::from(".claude/settings.json"),
            skills_dir: PathBuf::from(".claude/skills"),
        },
        (HarnessKind::Codex, false) => HarnessPaths {
            settings: config::expand_tilde("~/.codex/hooks.json"),
            skills_dir: config::expand_tilde("~/.agents/skills"),
        },
        (HarnessKind::Codex, true) => HarnessPaths {
            settings: PathBuf::from(".codex/hooks.json"),
            skills_dir: PathBuf::from(".agents/skills"),
        },
    }
}

// --- hook JSON entries -------------------------------------------------------

/// The managed `SessionStart` group: the routing command with the widened
/// matcher and the shared timeout.
fn session_start_group() -> Value {
    json!({
        "matcher": SESSION_START_MATCHER,
        "hooks": [ { "type": "command", "command": SESSION_START_COMMAND, "timeout": HOOK_TIMEOUT_SECS } ],
    })
}

/// The managed `Stop` group: the nudge command with the shared timeout. A
/// `Stop` group carries no matcher (the event has no matchable subject).
fn stop_group() -> Value {
    json!({
        "hooks": [ { "type": "command", "command": STOP_COMMAND, "timeout": HOOK_TIMEOUT_SECS } ],
    })
}

// --- pure merge / remove algorithm -------------------------------------------

/// Whether a single hook object is one Crystalline manages, by command string
/// alone: either the routing command or the nudge command.
fn is_managed_hook(hook: &Value) -> bool {
    matches!(
        hook.get("command").and_then(Value::as_str),
        Some(c) if c == SESSION_START_COMMAND || c == STOP_COMMAND
    )
}

/// Whether a hook group's `hooks` array runs `command`, ignoring the group's
/// matcher. This is the matcher-insensitive presence test that lets a
/// hand-written recipe count as already installed.
fn group_runs_command(group: &Value, command: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|h| h.get("command").and_then(Value::as_str) == Some(command))
        })
}

/// Whether a hook group contains any managed hook at all.
fn group_has_managed(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(is_managed_hook))
}

/// Whether the parsed settings root already runs `command` under `event`,
/// matcher-insensitively. The filesystem-free presence predicate that both
/// the install report and (in a later milestone) the doctor read from.
pub(crate) fn hook_present(root: &Map<String, Value>, event: &str, command: &str) -> bool {
    root.get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .is_some_and(|groups| groups.iter().any(|g| group_runs_command(g, command)))
}

/// Append the managed group for one event when it is not already present.
/// Returns whether the root changed. Never reorders foreign entries: the new
/// group is pushed onto the end of the event's array. A `hooks` value or an
/// event value of an unexpected JSON type is left entirely untouched (foreign
/// data is never coerced), reported as no change.
fn ensure_group(
    root: &mut Map<String, Value>,
    event: &str,
    command: &str,
    group: fn() -> Value,
) -> bool {
    let hooks_entry = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks_entry.as_object_mut() else {
        return false;
    };
    let event_entry = hooks
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(groups) = event_entry.as_array_mut() else {
        return false;
    };
    if groups.iter().any(|g| group_runs_command(g, command)) {
        return false;
    }
    groups.push(group());
    true
}

/// Merge both managed hook groups into the settings root, returning whether
/// anything changed. Idempotent: a root that already carries both commands
/// (under any matcher) is returned unchanged.
pub(crate) fn add_managed_hooks(root: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    changed |= ensure_group(
        root,
        "SessionStart",
        SESSION_START_COMMAND,
        session_start_group,
    );
    changed |= ensure_group(root, "Stop", STOP_COMMAND, stop_group);
    changed
}

/// Remove every managed hook from the settings root, returning whether
/// anything changed. Managed hook objects are dropped from every event's
/// groups; a group we empty of its last hook is pruned, an event array we
/// empty of its last group is pruned and the `hooks` object is pruned once it
/// holds nothing. A group that keeps a foreign hook survives with only that
/// foreign hook, and an untouched event or foreign empty structure is left
/// exactly as it was.
pub(crate) fn remove_managed_hooks(root: &mut Map<String, Value>) -> bool {
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        // Confirm the event value is an array carrying a managed hook right
        // here, before any removal or mutation, so a foreign event value (a
        // non-array, or an array of only foreign groups, even an empty one) is
        // never disturbed and the safety check sits at the mutation site.
        let Some(groups) = hooks.get(&event).and_then(Value::as_array) else {
            continue;
        };
        if !groups.iter().any(group_has_managed) {
            continue;
        }
        changed = true;
        let Some(Value::Array(groups)) = hooks.remove(&event) else {
            continue;
        };
        let mut kept = Vec::with_capacity(groups.len());
        for mut group in groups {
            if !group_has_managed(&group) {
                kept.push(group);
                continue;
            }
            if let Some(hooklist) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                hooklist.retain(|h| !is_managed_hook(h));
                if !hooklist.is_empty() {
                    kept.push(group);
                }
            }
        }
        if !kept.is_empty() {
            hooks.insert(event, Value::Array(kept));
        }
    }
    let hooks_now_empty = hooks.is_empty();
    if changed && hooks_now_empty {
        root.remove("hooks");
    }
    changed
}

// --- settings file IO --------------------------------------------------------

/// Read a settings file into its top-level object. A missing file is the empty
/// object (the merge builds it from scratch); a present file that is not valid
/// JSON, or is valid JSON but not an object, is a hard error naming the path,
/// so a file this process does not understand is never overwritten. Reused by
/// `doctor`, which reads the same file read-only to check for a parse error
/// and to test hook presence via [`hook_present`], never duplicating this
/// parsing logic.
pub(crate) fn read_settings(path: &Path) -> anyhow::Result<Map<String, Value>> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|e| {
                anyhow::anyhow!(
                    "{} is not valid JSON and was left unchanged: {e}",
                    path.display()
                )
            })?;
            match value {
                Value::Object(map) => Ok(map),
                _ => Err(anyhow::anyhow!(
                    "{} does not contain a JSON object and was left unchanged",
                    path.display()
                )),
            }
        }
    }
}

/// Write a settings object back atomically, pretty-printed with a trailing
/// newline. Only ever called when the merge or removal actually changed the
/// object, so a re-run that changed nothing never rewrites (and never
/// reformats) the file.
fn write_settings(path: &Path, root: &Map<String, Value>) -> anyhow::Result<()> {
    let mut text = serde_json::to_string_pretty(&Value::Object(root.clone()))?;
    text.push('\n');
    config::save_bytes(path, text.as_bytes())?;
    Ok(())
}

// --- MCP registration (shell-outs, never fatal) ------------------------------

/// The outcome of running a harness CLI once.
enum CliRun {
    /// The command exited zero.
    Ok,
    /// The command ran but exited non-zero.
    Failed,
    /// The command could not be spawned because the binary is not on PATH.
    NotFound,
}

/// Run a harness CLI, discarding its stdout and stderr so nothing leaks into
/// this command's own output, and never surfacing an error: a spawn failure
/// for a missing binary is reported as [`CliRun::NotFound`], any other spawn
/// or wait failure as [`CliRun::Failed`].
fn run_cli(program: &str, args: &[&str]) -> CliRun {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => CliRun::Ok,
        Ok(_) => CliRun::Failed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CliRun::NotFound,
        Err(_) => CliRun::Failed,
    }
}

/// The `mcp add` argument vector for a harness. Claude Code takes an explicit
/// `--scope`; Codex registers MCP servers per user only, so `--project` still
/// lands globally (called out in the printed notice).
fn mcp_add_args(harness: HarnessKind, project: bool) -> Vec<String> {
    let args: Vec<&str> = match harness {
        HarnessKind::ClaudeCode => {
            let scope = if project { "project" } else { "user" };
            vec![
                "mcp",
                "add",
                "crystalline",
                "--scope",
                scope,
                "crystalline",
                "mcp",
            ]
        }
        HarnessKind::Codex => vec!["mcp", "add", "crystalline", "--", "crystalline", "mcp"],
    };
    args.into_iter().map(String::from).collect()
}

/// Register the MCP server: check presence with `mcp get` first, then `mcp
/// add`. A missing CLI or a failing add never aborts the install; it records
/// the command to run by hand instead.
fn install_mcp(harness: HarnessKind, project: bool) -> McpReport {
    let program = harness.cli();
    let add_args = mcp_add_args(harness, project);
    let manual = format!("{program} {}", add_args.join(" "));

    match run_cli(program, &["mcp", "get", "crystalline"]) {
        CliRun::NotFound => return McpReport::new("cli-missing", Some(manual)),
        CliRun::Ok => return McpReport::new("already-present", None),
        CliRun::Failed => {}
    }

    let add_ref: Vec<&str> = add_args.iter().map(String::as_str).collect();
    match run_cli(program, &add_ref) {
        CliRun::NotFound => McpReport::new("cli-missing", Some(manual)),
        CliRun::Ok => McpReport::new("registered", None),
        CliRun::Failed => McpReport::new("failed", Some(manual)),
    }
}

/// Deregister the MCP server, tolerantly: a missing CLI records the manual
/// command, a non-zero exit is read as already gone.
fn uninstall_mcp(harness: HarnessKind) -> McpReport {
    let program = harness.cli();
    let manual = format!("{program} mcp remove crystalline");
    match run_cli(program, &["mcp", "remove", "crystalline"]) {
        CliRun::NotFound => McpReport::new("cli-missing", Some(manual)),
        CliRun::Ok => McpReport::new("removed", None),
        CliRun::Failed => McpReport::new("not-present", None),
    }
}

// --- hooks install / uninstall -----------------------------------------------

/// Merge the managed hooks into the settings file, writing only on change.
fn install_hooks(path: &Path) -> anyhow::Result<HooksReport> {
    let mut root = read_settings(path)?;
    let had_session_start = hook_present(&root, "SessionStart", SESSION_START_COMMAND);
    let had_stop = hook_present(&root, "Stop", STOP_COMMAND);
    let changed = add_managed_hooks(&mut root);
    if changed {
        write_settings(path, &root)?;
    }
    Ok(HooksReport {
        path: path.display().to_string(),
        session_start: if had_session_start {
            "already-present"
        } else {
            "added"
        },
        stop: if had_stop { "already-present" } else { "added" },
        written: changed,
    })
}

/// Remove the managed hooks from the settings file, writing only on change.
fn uninstall_hooks(path: &Path) -> anyhow::Result<HooksReport> {
    let mut root = read_settings(path)?;
    let had_session_start = hook_present(&root, "SessionStart", SESSION_START_COMMAND);
    let had_stop = hook_present(&root, "Stop", STOP_COMMAND);
    let changed = remove_managed_hooks(&mut root);
    if changed {
        write_settings(path, &root)?;
    }
    Ok(HooksReport {
        path: path.display().to_string(),
        session_start: if had_session_start {
            "removed"
        } else {
            "absent"
        },
        stop: if had_stop { "removed" } else { "absent" },
        written: changed,
    })
}

// --- skills install / uninstall ----------------------------------------------

/// Copy each managed skill into place: written when absent (`installed`) or
/// when its content differs from the embedded copy (`updated`), left alone
/// when it already matches (`already-current`).
fn install_skills(dir: &Path) -> anyhow::Result<SkillsReport> {
    let mut skills = Vec::with_capacity(MANAGED_SKILLS.len());
    for &(name, content) in MANAGED_SKILLS {
        let path = dir.join(name).join("SKILL.md");
        let status = match std::fs::read(&path) {
            Ok(existing) if existing == content.as_bytes() => "already-current",
            Ok(_) => {
                config::save_bytes(&path, content.as_bytes())?;
                "updated"
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                config::save_bytes(&path, content.as_bytes())?;
                "installed"
            }
            Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
        };
        skills.push(SkillReport { name, status });
    }
    Ok(SkillsReport {
        dir: dir.display().to_string(),
        skills,
    })
}

/// Remove each managed skill: dropped when its content matches the embedded
/// copy (`removed`), kept when a person edited it (`kept-modified`) unless
/// `force` is set (`removed-forced`), reported `absent` when it was never
/// there. Never touches a folder that is not one of the four managed skills.
fn uninstall_skills(dir: &Path, force: bool) -> anyhow::Result<SkillsReport> {
    let mut skills = Vec::with_capacity(MANAGED_SKILLS.len());
    for &(name, content) in MANAGED_SKILLS {
        let skill_dir = dir.join(name);
        let path = skill_dir.join("SKILL.md");
        let status = match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent",
            Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
            Ok(existing) if existing == content.as_bytes() => {
                remove_skill(&path, &skill_dir)?;
                "removed"
            }
            Ok(_) if force => {
                remove_skill(&path, &skill_dir)?;
                "removed-forced"
            }
            Ok(_) => "kept-modified",
        };
        skills.push(SkillReport { name, status });
    }
    Ok(SkillsReport {
        dir: dir.display().to_string(),
        skills,
    })
}

/// Delete a skill's `SKILL.md`, then remove its folder when that leaves it
/// empty. The folder removal is best-effort and only ever runs on an empty
/// managed-skill folder, so nothing a person added alongside is disturbed.
fn remove_skill(path: &Path, skill_dir: &Path) -> anyhow::Result<()> {
    std::fs::remove_file(path)
        .map_err(|e| anyhow::anyhow!("could not remove {}: {e}", path.display()))?;
    if let Ok(mut entries) = std::fs::read_dir(skill_dir)
        && entries.next().is_none()
    {
        let _ = std::fs::remove_dir(skill_dir);
    }
    Ok(())
}

// --- reports -----------------------------------------------------------------

/// The MCP registration outcome. `manual_command` is present exactly when the
/// automatic path did not complete, and carries the command to run by hand.
#[derive(Serialize)]
struct McpReport {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    manual_command: Option<String>,
}

impl McpReport {
    fn new(status: &'static str, manual_command: Option<String>) -> McpReport {
        McpReport {
            status,
            manual_command,
        }
    }
}

/// The hooks outcome: which file, what happened to each managed hook and
/// whether the file was rewritten at all.
#[derive(Serialize)]
struct HooksReport {
    path: String,
    session_start: &'static str,
    stop: &'static str,
    written: bool,
}

/// The skills outcome: the target folder and the per-skill result.
#[derive(Serialize)]
struct SkillsReport {
    dir: String,
    skills: Vec<SkillReport>,
}

/// One skill's result within a [`SkillsReport`].
#[derive(Serialize)]
struct SkillReport {
    name: &'static str,
    status: &'static str,
}

/// The full result of an `install`. A skipped part is `None`, omitted from the
/// JSON entirely; `notices` carries harness-specific follow-up (Codex's trust
/// step, for one).
#[derive(Serialize)]
struct InstallReport {
    harness: &'static str,
    scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<McpReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hooks: Option<HooksReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills: Option<SkillsReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
}

/// The full result of an `uninstall`. Same shape as [`InstallReport`]; every
/// part always runs, since `uninstall` has no skip options.
#[derive(Serialize)]
struct UninstallReport {
    harness: &'static str,
    scope: &'static str,
    mcp: McpReport,
    hooks: HooksReport,
    skills: SkillsReport,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notices: Vec<String>,
}

// --- human rendering ---------------------------------------------------------

/// The human-readable MCP line for a report status.
fn mcp_line(m: &McpReport) -> String {
    let manual = m.manual_command.as_deref().unwrap_or("");
    match m.status {
        "already-present" => "already registered".to_string(),
        "registered" => "registered".to_string(),
        "removed" => "removed".to_string(),
        "not-present" => "not registered (nothing to remove)".to_string(),
        "cli-missing" => {
            format!("harness CLI not found. Register it yourself with: {manual}")
        }
        "failed" => format!("registration failed. Register it yourself with: {manual}"),
        other => other.to_string(),
    }
}

/// The human-readable label for a per-hook status.
fn hook_label(status: &str) -> &str {
    match status {
        "already-present" => "already present",
        "absent" => "not present",
        other => other,
    }
}

/// The human-readable label for a per-skill status.
fn skill_label(status: &str) -> &str {
    match status {
        "already-current" => "already current",
        "kept-modified" => "kept (locally modified)",
        "removed-forced" => "removed (was locally modified)",
        other => other,
    }
}

/// Render the shared body of an install or uninstall report under a header
/// line. Skipped parts render as `skipped`; notices trail after a blank line.
fn render_human(
    header: String,
    mcp: Option<&McpReport>,
    hooks: Option<&HooksReport>,
    skills: Option<&SkillsReport>,
    notices: &[String],
) -> String {
    let mut out = header;
    out.push('\n');
    match mcp {
        None => out.push_str("  MCP server: skipped\n"),
        Some(m) => out.push_str(&format!("  MCP server: {}\n", mcp_line(m))),
    }
    match hooks {
        None => out.push_str("  Hooks: skipped\n"),
        Some(h) => {
            out.push_str(&format!(
                "  SessionStart hook: {}\n",
                hook_label(h.session_start)
            ));
            out.push_str(&format!("  Stop hook: {}\n", hook_label(h.stop)));
            out.push_str(&format!("  Settings file: {}\n", h.path));
        }
    }
    match skills {
        None => out.push_str("  Skills: skipped\n"),
        Some(s) => {
            out.push_str(&format!("  Skills folder: {}\n", s.dir));
            for sk in &s.skills {
                out.push_str(&format!("    {}: {}\n", sk.name, skill_label(sk.status)));
            }
        }
    }
    for note in notices {
        out.push('\n');
        out.push_str(note);
        out.push('\n');
    }
    out
}

// --- entry points ------------------------------------------------------------

/// Run `crystalline install`: register the MCP server, install the hooks and
/// copy the skills for one harness, each part skippable. Never returns an
/// error for a missing or failing harness CLI (that degrades to a printed
/// manual command); a genuinely unreadable or unparseable settings file, or a
/// filesystem error writing a skill, does surface as an error.
pub fn run_install(opts: InstallOptions, json: bool) -> anyhow::Result<()> {
    let paths = harness_paths(opts.harness, opts.project);
    let scope = if opts.project { "project" } else { "user" };

    let mcp = if opts.skip_mcp {
        None
    } else {
        Some(install_mcp(opts.harness, opts.project))
    };
    let hooks = if opts.skip_hooks {
        None
    } else {
        Some(install_hooks(&paths.settings)?)
    };
    let skills = if opts.skip_skills {
        None
    } else {
        Some(install_skills(&paths.skills_dir)?)
    };

    let mut notices = Vec::new();
    if opts.harness == HarnessKind::Codex {
        if !opts.skip_mcp && opts.project {
            notices.push(
                "Codex registers MCP servers for your user, so the crystalline server was added globally even with --project."
                    .to_string(),
            );
        }
        if !opts.skip_hooks {
            notices.push(
                "Codex loads hooks only after you trust them. Run /hooks inside Codex to review and trust the crystalline hooks."
                    .to_string(),
            );
            if opts.project {
                notices.push(
                    "The project hooks in .codex/hooks.json need trusting in this repository specifically."
                        .to_string(),
                );
            }
        }
    }

    let report = InstallReport {
        harness: opts.harness.id(),
        scope,
        mcp,
        hooks,
        skills,
        notices,
    };

    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        let header = format!(
            "Installed Crystalline for {} ({scope} scope).",
            opts.harness.display_name()
        );
        print!(
            "{}",
            render_human(
                header,
                report.mcp.as_ref(),
                report.hooks.as_ref(),
                report.skills.as_ref(),
                &report.notices,
            )
        );
    }
    Ok(())
}

/// Run `crystalline uninstall`: deregister the MCP server, remove the managed
/// hooks and drop the managed skills for one harness, leaving every foreign
/// hook, key and skill untouched. A locally edited skill is kept unless
/// `force` is set.
pub fn run_uninstall(
    harness: HarnessKind,
    project: bool,
    force: bool,
    json: bool,
) -> anyhow::Result<()> {
    let paths = harness_paths(harness, project);
    let scope = if project { "project" } else { "user" };

    let mcp = uninstall_mcp(harness);
    let hooks = uninstall_hooks(&paths.settings)?;
    let skills = uninstall_skills(&paths.skills_dir, force)?;

    let mut notices = Vec::new();
    if harness == HarnessKind::Codex {
        notices.push(
            "If you trusted the crystalline hooks in Codex, revisit /hooks now that they are gone."
                .to_string(),
        );
    }

    let report = UninstallReport {
        harness: harness.id(),
        scope,
        mcp,
        hooks,
        skills,
        notices,
    };

    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        let header = format!(
            "Removed Crystalline from {} ({scope} scope).",
            harness.display_name()
        );
        print!(
            "{}",
            render_human(
                header,
                Some(&report.mcp),
                Some(&report.hooks),
                Some(&report.skills),
                &report.notices,
            )
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a settings object from a JSON literal, panicking if it is not an
    /// object (a test-only convenience).
    fn root(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn add_creates_both_groups_and_is_idempotent() {
        let mut root = Map::new();
        assert!(add_managed_hooks(&mut root), "first add changes the root");
        assert!(hook_present(&root, "SessionStart", SESSION_START_COMMAND));
        assert!(hook_present(&root, "Stop", STOP_COMMAND));
        // The exact managed shape, matcher and timeout included.
        assert_eq!(
            root["hooks"]["SessionStart"][0]["matcher"],
            SESSION_START_MATCHER
        );
        assert_eq!(
            root["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            SESSION_START_COMMAND
        );
        assert_eq!(root["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 10);
        assert_eq!(
            root["hooks"]["Stop"][0]["hooks"][0]["command"],
            STOP_COMMAND
        );
        // A Stop group carries no matcher.
        assert!(root["hooks"]["Stop"][0].get("matcher").is_none());
        // Second add is a no-op.
        assert!(
            !add_managed_hooks(&mut root),
            "second add must not change the root"
        );
    }

    #[test]
    fn add_is_matcher_insensitive_for_an_existing_recipe() {
        // The README's hand-written recipe: the same command under a plain
        // "startup" matcher, no timeout.
        let mut root = root(json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "startup", "hooks": [ { "type": "command", "command": SESSION_START_COMMAND } ] }
                ]
            }
        }));
        let changed = add_managed_hooks(&mut root);
        // Only the Stop hook is new; SessionStart is already present.
        assert!(changed);
        let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            session_start.len(),
            1,
            "no duplicate SessionStart group is added"
        );
        assert_eq!(session_start[0]["matcher"], "startup");
        assert!(hook_present(&root, "Stop", STOP_COMMAND));
    }

    #[test]
    fn add_preserves_foreign_events_and_appends_after_them() {
        let mut root = root(json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [ { "type": "command", "command": "echo hi" } ] }
                ],
                "Stop": [
                    { "hooks": [ { "type": "command", "command": "my-own-stop" } ] }
                ]
            }
        }));
        assert!(add_managed_hooks(&mut root));
        // Foreign PreToolUse survives verbatim.
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo hi"
        );
        // The foreign Stop group stays first, ours is appended after it.
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "my-own-stop");
        assert_eq!(stop[1]["hooks"][0]["command"], STOP_COMMAND);
        assert!(hook_present(&root, "SessionStart", SESSION_START_COMMAND));
    }

    #[test]
    fn remove_deletes_managed_and_prunes_the_hooks_object() {
        let mut root = Map::new();
        add_managed_hooks(&mut root);
        assert!(remove_managed_hooks(&mut root));
        assert!(
            !root.contains_key("hooks"),
            "a hooks object holding only managed groups is pruned entirely"
        );
    }

    #[test]
    fn remove_preserves_foreign_events_and_drops_only_ours() {
        let mut root = root(json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "echo hi" } ] }
                ]
            }
        }));
        add_managed_hooks(&mut root);
        assert!(remove_managed_hooks(&mut root));
        assert!(!hook_present(&root, "SessionStart", SESSION_START_COMMAND));
        assert!(!hook_present(&root, "Stop", STOP_COMMAND));
        assert!(
            !root["hooks"]
                .as_object()
                .unwrap()
                .contains_key("SessionStart"),
            "the emptied SessionStart event is pruned"
        );
        // Foreign event untouched.
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "echo hi"
        );
    }

    #[test]
    fn remove_strips_only_the_managed_hook_from_a_shared_group() {
        // A single group whose hooks array holds both our command and a
        // foreign one: only ours is stripped, the group survives.
        let mut root = root(json!({
            "hooks": {
                "Stop": [
                    { "hooks": [
                        { "type": "command", "command": STOP_COMMAND },
                        { "type": "command", "command": "other" }
                    ] }
                ]
            }
        }));
        assert!(remove_managed_hooks(&mut root));
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "the group is kept for its foreign hook");
        let hooks = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "other");
    }

    #[test]
    fn remove_is_matcher_insensitive_for_a_hand_written_recipe() {
        let mut root = root(json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "startup", "hooks": [ { "type": "command", "command": SESSION_START_COMMAND } ] }
                ]
            }
        }));
        assert!(remove_managed_hooks(&mut root));
        assert!(
            !root.contains_key("hooks"),
            "the hand-written recipe is recognized and removed"
        );
    }

    #[test]
    fn remove_on_a_root_without_our_hooks_is_a_no_op() {
        let mut root = root(json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "echo hi" } ] }
                ]
            }
        }));
        let before = root.clone();
        assert!(
            !remove_managed_hooks(&mut root),
            "nothing managed to remove"
        );
        assert_eq!(root, before, "a no-op removal changes nothing");
    }

    #[test]
    fn add_leaves_unrelated_top_level_keys_alone() {
        let mut root = root(json!({
            "model": "opus",
            "permissions": { "allow": ["Bash"] }
        }));
        assert!(add_managed_hooks(&mut root));
        assert_eq!(root["model"], "opus");
        assert_eq!(root["permissions"]["allow"][0], "Bash");
        assert!(hook_present(&root, "SessionStart", SESSION_START_COMMAND));
    }

    // --- hostile shapes: the foreign-data-preservation guarantee ------------
    //
    // Every case below feeds the merge and removal a settings shape a person or
    // another tool could plausibly leave behind that is not the shape the code
    // expects. None of these foreign values may ever be coerced, dropped or
    // otherwise disturbed. These lock that in against future regressions.

    #[test]
    fn add_and_remove_leave_a_scalar_hooks_value_untouched() {
        // `hooks` itself is a scalar, a shape ensure_group refuses to coerce.
        // Both directions no-op and the foreign value survives verbatim.
        let mut root = root(json!({ "hooks": 5 }));
        let before = root.clone();
        assert!(
            !add_managed_hooks(&mut root),
            "a scalar hooks value blocks the add"
        );
        assert_eq!(
            root, before,
            "add leaves the foreign scalar hooks value alone"
        );
        assert!(
            !remove_managed_hooks(&mut root),
            "nothing managed lives under a scalar hooks value"
        );
        assert_eq!(
            root, before,
            "remove leaves the foreign scalar hooks value alone"
        );
    }

    #[test]
    fn add_and_remove_leave_a_foreign_scalar_event_value_untouched() {
        // A scalar under a known event name: ensure_group will not coerce it,
        // so the foreign string survives even though a fresh Stop group lands
        // beside it, and remove strips that Stop group again untouched.
        let mut root = root(json!({ "hooks": { "SessionStart": "not-an-array" } }));
        assert!(add_managed_hooks(&mut root));
        assert_eq!(
            root["hooks"]["SessionStart"], "not-an-array",
            "add never coerces the foreign scalar event value"
        );
        assert!(remove_managed_hooks(&mut root));
        assert_eq!(
            root["hooks"]["SessionStart"], "not-an-array",
            "remove never coerces the foreign scalar event value"
        );
    }

    #[test]
    fn add_and_remove_preserve_foreign_non_group_entries_in_an_event_array() {
        // An event array salted with a string, a number and a null beside a
        // foreign object group: every one is a foreign entry ours must survive.
        let foreign = json!([
            "a-string",
            42,
            null,
            { "matcher": "x", "hooks": [ { "type": "command", "command": "foreign" } ] }
        ]);
        let mut root = root(json!({ "hooks": { "SessionStart": foreign } }));
        assert!(add_managed_hooks(&mut root));
        let groups = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups[0], "a-string");
        assert_eq!(groups[1], 42);
        assert!(groups[2].is_null());
        assert_eq!(groups[3]["hooks"][0]["command"], "foreign");
        assert_eq!(groups[4]["hooks"][0]["command"], SESSION_START_COMMAND);
        // Remove strips only our appended group; the four foreign entries stay
        // in place and in order.
        assert!(remove_managed_hooks(&mut root));
        assert_eq!(
            root["hooks"]["SessionStart"],
            json!([
                "a-string",
                42,
                null,
                { "matcher": "x", "hooks": [ { "type": "command", "command": "foreign" } ] }
            ])
        );
    }

    #[test]
    fn add_and_remove_preserve_groups_with_a_missing_or_non_array_hooks_key() {
        // Two malformed groups: one with no `hooks` key, one whose `hooks` is a
        // number. Neither is read as running our command and neither is
        // disturbed by add or by remove.
        let mut root = root(json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "x" },
                    { "hooks": 7 }
                ]
            }
        }));
        assert!(add_managed_hooks(&mut root));
        let groups = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups[0], json!({ "matcher": "x" }));
        assert_eq!(groups[1], json!({ "hooks": 7 }));
        assert_eq!(groups[2]["hooks"][0]["command"], SESSION_START_COMMAND);
        assert!(remove_managed_hooks(&mut root));
        assert_eq!(
            root["hooks"]["SessionStart"],
            json!([ { "matcher": "x" }, { "hooks": 7 } ]),
            "both malformed groups are treated as foreign and kept verbatim"
        );
    }

    #[test]
    fn remove_preserves_a_hook_whose_command_is_not_a_string() {
        // A hook object whose `command` is a number is not one of ours, so a
        // group holding it beside our managed hook keeps the odd hook when ours
        // is stripped.
        let mut root = root(json!({
            "hooks": {
                "Stop": [
                    { "hooks": [
                        { "type": "command", "command": 42 },
                        { "type": "command", "command": STOP_COMMAND }
                    ] }
                ]
            }
        }));
        assert!(remove_managed_hooks(&mut root));
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "the group survives for its foreign hook");
        let hooks = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], 42, "the numeric-command hook is kept");
    }

    #[test]
    fn a_command_that_only_contains_ours_is_never_matched() {
        // Presence is an exact string test, not a substring one: a foreign
        // command with our routing command embedded in it is neither counted as
        // present by add nor pulled out by remove.
        let foreign_cmd = "run crystalline prompt system please";
        let mut root = root(json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "x", "hooks": [ { "type": "command", "command": foreign_cmd } ] }
                ]
            }
        }));
        let before = root.clone();
        assert!(
            !remove_managed_hooks(&mut root),
            "a substring command is not ours to remove"
        );
        assert_eq!(root, before, "remove leaves the substring command in place");
        // Add does not read it as already present, so our own group is appended
        // beside it.
        assert!(add_managed_hooks(&mut root));
        let groups = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 2, "ours is added beside the foreign group");
        assert_eq!(groups[0]["hooks"][0]["command"], foreign_cmd);
        assert_eq!(groups[1]["hooks"][0]["command"], SESSION_START_COMMAND);
        // Remove drops only ours; the substring command is still there.
        assert!(remove_managed_hooks(&mut root));
        let groups = root["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], foreign_cmd);
    }

    #[test]
    fn read_settings_reads_a_missing_file_as_empty_and_parses_an_object() {
        let dir = tempfile::tempdir().unwrap();
        // A missing file is the empty object the merge builds onto.
        let missing = dir.path().join("missing.json");
        assert!(read_settings(&missing).unwrap().is_empty());
        // A present object round-trips into its top-level map.
        let object = dir.path().join("settings.json");
        std::fs::write(&object, r#"{ "model": "opus" }"#).unwrap();
        let map = read_settings(&object).unwrap();
        assert_eq!(map["model"], "opus");
    }

    #[test]
    fn read_settings_rejects_valid_json_that_is_not_an_object() {
        // Valid JSON that is not an object (a bare array, a bare scalar) is a
        // hard error naming the path, never a silent overwrite.
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in [("array.json", "[]"), ("scalar.json", "5")] {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            let err = read_settings(&path).unwrap_err().to_string();
            assert!(
                err.contains("does not contain a JSON object"),
                "non-object JSON is rejected: {err}"
            );
            assert!(err.contains(name), "the error names the path: {err}");
        }
    }
}
