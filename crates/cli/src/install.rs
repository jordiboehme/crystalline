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
//! Every install run records what it did in the install receipt
//! (`receipt.rs`, `<state_dir>/installs.json`): binary version, chosen
//! parts and each skill's content hash as written. The receipt is what a
//! later binary reconciles against - updating old untouched skills in
//! place, preserving edited ones as `SKILL.md.bak` and retiring skills the
//! newer version no longer ships - and what the session-start auto-update
//! in `prompt system` replays install options from.
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

use crate::receipt;

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

/// Skill folder names shipped by past releases and no longer managed. When a
/// release drops or renames a managed skill, its old folder name is appended
/// here in the same change and never leaves the list, so install and the
/// session-start auto-reconcile can retire a leftover even when no receipt
/// records it (a zip-unpacked install, a lost state directory).
pub(crate) const RETIRED_SKILLS: &[&str] = &[];

/// How a reconcile treats a managed skill whose file is missing: an explicit
/// `crystalline install` writes it (the user asked for a full install), the
/// session-start auto-reconcile respects what it reads as a deliberate
/// deletion and drops the skill from the receipt instead.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReconcileMode {
    Install,
    Auto,
}

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

/// The version-skew guard for the hooks: they run whatever `crystalline` the
/// PATH resolves at hook time, which is not necessarily the binary running
/// this install. An older PATH binary without the `hook` verb exits 2 on
/// every Stop event, which a harness reads as "block the stop" - a real
/// footgun, so a mismatch earns a notice. `None` when the PATH binary
/// matches this one; a notice string otherwise (missing, unresponsive or a
/// different version).
fn path_binary_notice() -> Option<String> {
    let mine = env!("CARGO_PKG_VERSION");
    match std::process::Command::new("crystalline")
        .arg("--version")
        .output()
    {
        Err(_) => Some(
            "The hooks run `crystalline` from your PATH, but none was found there. Put this binary on the PATH or the hooks will fail."
                .to_string(),
        ),
        Ok(out) => {
            let version = String::from_utf8_lossy(&out.stdout);
            let version = version.trim().strip_prefix("crystalline ").unwrap_or("");
            if !out.status.success() || version.is_empty() {
                Some(
                    "The hooks run `crystalline` from your PATH, but it did not answer --version. Check which binary the PATH resolves."
                        .to_string(),
                )
            } else if version != mine {
                Some(format!(
                    "The hooks run `crystalline` from your PATH, which is version {version}, not this binary's {mine}. Upgrade the installed crystalline (or adjust the PATH) so the hooks find the verbs they need."
                ))
            } else {
                None
            }
        }
    }
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

/// Reconcile one skills folder against [`MANAGED_SKILLS`]: install or update
/// every current skill, retire leftovers named by the receipt or by
/// [`RETIRED_SKILLS`] and return the per-skill report plus fresh receipt
/// records for everything now on disk.
pub(crate) fn reconcile_skills(
    dir: &Path,
    prior: &[receipt::RecordedSkill],
    mode: ReconcileMode,
) -> anyhow::Result<(SkillsReport, Vec<receipt::RecordedSkill>)> {
    reconcile_skill_set(dir, MANAGED_SKILLS, RETIRED_SKILLS, prior, mode)
}

/// The engine behind [`reconcile_skills`], parameterized over the current
/// and retired skill sets so tests can exercise retirement while the real
/// retired list is still empty.
///
/// The receipt hash decides how a differing file is treated: a file whose
/// hash matches what an earlier install recorded is an old untouched copy
/// and is overwritten in place; anything else was edited by a person and is
/// preserved as `SKILL.md.bak` first. With no receipt every mismatch takes
/// the backup path, so losing the receipt never loses user content.
fn reconcile_skill_set(
    dir: &Path,
    current: &[(&str, &str)],
    retired: &[&str],
    prior: &[receipt::RecordedSkill],
    mode: ReconcileMode,
) -> anyhow::Result<(SkillsReport, Vec<receipt::RecordedSkill>)> {
    let prior_hash: std::collections::HashMap<&str, &str> = prior
        .iter()
        .map(|r| (r.name.as_str(), r.sha256.as_str()))
        .collect();
    let mut skills = Vec::new();
    let mut records = Vec::new();

    for &(name, content) in current {
        let path = dir.join(name).join("SKILL.md");
        let status = match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match mode {
                ReconcileMode::Install => {
                    config::save_bytes(&path, content.as_bytes())?;
                    records.push(record_of(name, content));
                    "installed"
                }
                ReconcileMode::Auto => "user-removed",
            },
            Err(e) => return Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
            Ok(existing) if existing == content.as_bytes() => {
                records.push(record_of(name, content));
                "already-current"
            }
            Ok(existing) => {
                let file_hash = receipt::sha256_hex(&existing);
                let clean = prior_hash.get(name).copied() == Some(file_hash.as_str());
                if !clean {
                    config::save_bytes(&path.with_file_name("SKILL.md.bak"), &existing)?;
                }
                config::save_bytes(&path, content.as_bytes())?;
                records.push(record_of(name, content));
                if clean { "updated" } else { "updated-backup" }
            }
        };
        skills.push(SkillReport {
            name: name.to_string(),
            status,
        });
    }

    // Leftovers: names an earlier install recorded that the current set no
    // longer carries, then the static retired list for receipt-less
    // leftovers. `seen` keeps a name that appears in both from being retired
    // twice.
    let current_names: std::collections::HashSet<&str> = current.iter().map(|&(n, _)| n).collect();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for r in prior {
        if current_names.contains(r.name.as_str()) || !seen.insert(r.name.as_str()) {
            continue;
        }
        // The receipt is attacker-writable local state; a hostile name is
        // skipped outright, silently, before it ever reaches `retire`'s
        // `dir.join(name)`. See `is_plain_skill_name`.
        if !is_plain_skill_name(&r.name) {
            continue;
        }
        if let Some(status) = retire(dir, &r.name, Some(&r.sha256))? {
            skills.push(SkillReport {
                name: r.name.clone(),
                status,
            });
        }
    }
    for &name in retired {
        if current_names.contains(name) || !seen.insert(name) {
            continue;
        }
        if let Some(status) = retire(dir, name, None)? {
            skills.push(SkillReport {
                name: name.to_string(),
                status,
            });
        }
    }

    Ok((
        SkillsReport {
            dir: dir.display().to_string(),
            skills,
        },
        records,
    ))
}

/// Whether `name` is safe to use as a single path component under a skills
/// folder, i.e. whether `dir.join(name)` can only ever land inside `dir`.
///
/// [`MANAGED_SKILLS`] and [`RETIRED_SKILLS`] are compiled into this binary,
/// so their names are trusted outright. A name read back from the install
/// receipt is not: the receipt is attacker-writable local state (plain JSON
/// under `<state_dir>/installs.json`, editable by anything that can write
/// there), yet the leftover-retirement loops join a receipt name straight
/// onto the skills directory. `Path::join` treats an absolute second
/// argument as a full replacement of the first, so a recorded name of
/// `/etc/passwd` (or, on Windows, a drive-rooted path) would target that
/// path outright, and a name of `../../elsewhere` climbs out of the skills
/// folder through plain relative segments. A name is only usable once it is
/// a single, plain path component: non-empty, neither `.` nor `..`, and
/// free of both `/` and `\` (checked on every platform, since a receipt
/// written by one platform's binary can end up read back on another).
/// Anything else must be skipped outright wherever it would otherwise reach
/// the filesystem, never sanitized or truncated into something safe-looking.
pub(crate) fn is_plain_skill_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

/// A fresh receipt record for a skill at its embedded content.
fn record_of(name: &str, content: &str) -> receipt::RecordedSkill {
    receipt::RecordedSkill {
        name: name.to_string(),
        sha256: receipt::sha256_hex(content.as_bytes()),
    }
}

/// Retire one leftover skill folder. A file matching its recorded hash is
/// provably ours and is deleted (folder pruned when emptied); anything else
/// present was edited or cannot be proven ours, so the live `SKILL.md` is
/// preserved as `SKILL.md.bak` instead - retired either way, destroyed
/// never. `None` when nothing was there to retire.
fn retire(
    dir: &Path,
    name: &str,
    recorded_sha256: Option<&str>,
) -> anyhow::Result<Option<&'static str>> {
    let skill_dir = dir.join(name);
    let path = skill_dir.join("SKILL.md");
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
        Ok(existing) => {
            let file_hash = receipt::sha256_hex(&existing);
            if recorded_sha256 == Some(file_hash.as_str()) {
                remove_skill(&path, &skill_dir)?;
                Ok(Some("removed-retired"))
            } else {
                let bak = skill_dir.join("SKILL.md.bak");
                // Windows refuses a rename onto an existing file; the old
                // backup loses to the newer divergence either way.
                let _ = std::fs::remove_file(&bak);
                std::fs::rename(&path, &bak)
                    .map_err(|e| anyhow::anyhow!("could not move {} aside: {e}", path.display()))?;
                Ok(Some("retired-backup"))
            }
        }
    }
}

/// Remove each managed skill plus any receipt-recorded or list-retired
/// leftover. A file matching this binary's embedded copy or its own receipt
/// hash is untouched by the user and is dropped; a genuinely edited file is
/// kept (`kept-modified`) unless `force` is set. The receipt hash is what
/// keeps an old-but-clean skill from being mistaken for an edited one.
fn uninstall_skills(
    dir: &Path,
    prior: &[receipt::RecordedSkill],
    force: bool,
) -> anyhow::Result<SkillsReport> {
    let prior_hash: std::collections::HashMap<&str, &str> = prior
        .iter()
        .map(|r| (r.name.as_str(), r.sha256.as_str()))
        .collect();
    let mut skills = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    let drop_one = |name: &str, embedded: Option<&str>| -> anyhow::Result<Option<&'static str>> {
        let skill_dir = dir.join(name);
        let path = skill_dir.join("SKILL.md");
        match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(if embedded.is_some() {
                Some("absent")
            } else {
                None
            }),
            Err(e) => Err(anyhow::anyhow!("could not read {}: {e}", path.display())),
            Ok(existing) => {
                let file_hash = receipt::sha256_hex(&existing);
                let clean = embedded.is_some_and(|c| existing == c.as_bytes())
                    || prior_hash.get(name).copied() == Some(file_hash.as_str());
                if clean {
                    remove_skill(&path, &skill_dir)?;
                    Ok(Some("removed"))
                } else if force {
                    remove_skill(&path, &skill_dir)?;
                    Ok(Some("removed-forced"))
                } else {
                    Ok(Some("kept-modified"))
                }
            }
        }
    };

    for &(name, content) in MANAGED_SKILLS {
        seen.insert(name);
        if let Some(status) = drop_one(name, Some(content))? {
            skills.push(SkillReport {
                name: name.to_string(),
                status,
            });
        }
    }
    // Leftovers behind the current set: receipt names first (they carry a
    // hash), then the static retired list. Reported only when present.
    for r in prior {
        if !seen.insert(r.name.as_str()) {
            continue;
        }
        // The receipt is attacker-writable local state; a hostile name is
        // skipped outright, silently, before it ever reaches `drop_one`'s
        // `dir.join(name)`. See `is_plain_skill_name`.
        if !is_plain_skill_name(&r.name) {
            continue;
        }
        if let Some(status) = drop_one(&r.name, None)? {
            skills.push(SkillReport {
                name: r.name.clone(),
                status,
            });
        }
    }
    for &name in RETIRED_SKILLS {
        if !seen.insert(name) {
            continue;
        }
        if let Some(status) = drop_one(name, None)? {
            skills.push(SkillReport {
                name: name.to_string(),
                status,
            });
        }
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
pub(crate) struct SkillsReport {
    dir: String,
    skills: Vec<SkillReport>,
}

/// One skill's result within a [`SkillsReport`].
#[derive(Serialize)]
struct SkillReport {
    name: String,
    status: &'static str,
}

/// The receipt outcome: where it lives and whether this run rewrote it.
#[derive(Serialize)]
struct ReceiptReport {
    path: String,
    written: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<ReceiptReport>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt: Option<ReceiptReport>,
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
        "updated-backup" => "updated (your edited copy kept as SKILL.md.bak)",
        "removed-retired" => "removed (retired in this version)",
        "retired-backup" => "retired (your copy kept as SKILL.md.bak)",
        "user-removed" => "not reinstalled (removed by you)",
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
    receipt: Option<&ReceiptReport>,
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
    if let Some(r) = receipt {
        let state = if r.written { "" } else { " (not written)" };
        out.push_str(&format!("  Install receipt: {}{state}\n", r.path));
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

    // Receipt context first: the prior record's hashes feed the skills
    // reconcile, so it must be in hand before any part runs. A corrupt
    // receipt is disposable state and is regenerated from empty.
    let receipt_path = receipt::receipt_path().ok();
    let mut book = receipt_path
        .as_deref()
        .map(|p| receipt::load(p).unwrap_or_default())
        .unwrap_or_default();
    let project_path = if opts.project {
        let cwd = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("could not resolve the working directory: {e}"))?;
        Some(cwd.canonicalize().unwrap_or(cwd).display().to_string())
    } else {
        None
    };
    let prior = book
        .find(opts.harness.id(), scope, project_path.as_deref())
        .cloned();

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
    let mut new_records = None;
    let skills = if opts.skip_skills {
        None
    } else {
        let prior_skills = prior.as_ref().map(|p| p.skills.as_slice()).unwrap_or(&[]);
        let (report, records) =
            reconcile_skills(&paths.skills_dir, prior_skills, ReconcileMode::Install)?;
        new_records = Some(records);
        Some(report)
    };

    let mut notices = Vec::new();
    if !opts.skip_hooks
        && let Some(notice) = path_binary_notice()
    {
        notices.push(notice);
    }
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

    // The receipt records the union of everything ever installed here: a
    // part skipped on this run keeps its earlier record, since skipping
    // means "do not touch", not "undo".
    let prior_parts = prior.as_ref().map(|p| p.parts).unwrap_or(receipt::Parts {
        mcp: false,
        hooks: false,
        skills: false,
    });
    let merged_parts = receipt::Parts {
        mcp: prior_parts.mcp || !opts.skip_mcp,
        hooks: prior_parts.hooks || !opts.skip_hooks,
        skills: prior_parts.skills || !opts.skip_skills,
    };

    // The version stamped here is what the session-start auto-reconcile
    // compares against: a match means "nothing to do". Stamping the current
    // binary's version unconditionally would disarm that check for a part
    // this run skipped but `merged_parts` still carries as true - hooks or
    // skills installed by an earlier run and left untouched here, not
    // reinstalled fresh. mcp is excluded from this decision on purpose: it
    // is version-independent and the auto-reconcile never replays it, so a
    // skipped MCP registration has nothing that could go stale. Only when
    // this run actually skipped a hooks or skills part that is recorded true
    // do we carry the prior entry's version forward instead, so a later
    // session start still sees the mismatch and reconciles the part this run
    // left alone.
    let skipped_a_recorded_part =
        (opts.skip_hooks && merged_parts.hooks) || (opts.skip_skills && merged_parts.skills);
    let version = if skipped_a_recorded_part {
        prior
            .as_ref()
            .map(|p| p.version.clone())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
    } else {
        env!("CARGO_PKG_VERSION").to_string()
    };

    book.upsert(receipt::InstallRecord {
        harness: opts.harness.id().to_string(),
        scope: scope.to_string(),
        project_path,
        version,
        parts: merged_parts,
        skills: match new_records {
            Some(records) => records,
            None => prior.map(|p| p.skills).unwrap_or_default(),
        },
    });
    let receipt_report = match &receipt_path {
        Some(path) => match receipt::save(path, &book) {
            Ok(()) => Some(ReceiptReport {
                path: path.display().to_string(),
                written: true,
            }),
            Err(e) => {
                notices.push(format!(
                    "Could not write the install receipt ({e}). Session-start auto-updates will not cover this install until a later `crystalline install` succeeds."
                ));
                Some(ReceiptReport {
                    path: path.display().to_string(),
                    written: false,
                })
            }
        },
        None => {
            notices.push(
                "Could not resolve the state directory for the install receipt; session-start auto-updates will not cover this install."
                    .to_string(),
            );
            None
        }
    };

    let report = InstallReport {
        harness: opts.harness.id(),
        scope,
        mcp,
        hooks,
        skills,
        receipt: receipt_report,
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
                report.receipt.as_ref(),
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

    // Receipt context first, mirroring run_install: the prior record's skill
    // hashes are what let uninstall recognize an old-but-clean skill that
    // predates this binary's embedded copy.
    let receipt_path = receipt::receipt_path().ok();
    let mut book = receipt_path
        .as_deref()
        .map(|p| receipt::load(p).unwrap_or_default())
        .unwrap_or_default();
    let project_path = if project {
        let cwd = std::env::current_dir()
            .map_err(|e| anyhow::anyhow!("could not resolve the working directory: {e}"))?;
        Some(cwd.canonicalize().unwrap_or(cwd).display().to_string())
    } else {
        None
    };
    let prior = book
        .find(harness.id(), scope, project_path.as_deref())
        .cloned();

    let mcp = uninstall_mcp(harness);
    let hooks = uninstall_hooks(&paths.settings)?;
    let prior_skills = prior.as_ref().map(|p| p.skills.as_slice()).unwrap_or(&[]);
    let skills = uninstall_skills(&paths.skills_dir, prior_skills, force)?;

    let mut notices = Vec::new();
    if harness == HarnessKind::Codex {
        notices.push(
            "If you trusted the crystalline hooks in Codex, revisit /hooks now that they are gone."
                .to_string(),
        );
    }

    // Prune this target's entry from the receipt, saving only when something
    // was actually there to remove: an uninstall of a target the receipt
    // never knew about must never touch the file.
    let removed = book.remove(harness.id(), scope, project_path.as_deref());
    let receipt_report = if removed {
        match &receipt_path {
            Some(path) => match receipt::save(path, &book) {
                Ok(()) => Some(ReceiptReport {
                    path: path.display().to_string(),
                    written: true,
                }),
                Err(e) => {
                    notices.push(format!(
                        "Could not update the install receipt ({e}). It may still list this install as present."
                    ));
                    Some(ReceiptReport {
                        path: path.display().to_string(),
                        written: false,
                    })
                }
            },
            None => {
                notices.push(
                    "Could not resolve the state directory for the install receipt; it may still list this install as present."
                        .to_string(),
                );
                None
            }
        }
    } else {
        None
    };

    let report = UninstallReport {
        harness: harness.id(),
        scope,
        mcp,
        hooks,
        skills,
        receipt: receipt_report,
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
                report.receipt.as_ref(),
                &report.notices,
            )
        );
    }
    Ok(())
}

// --- session-start auto-reconcile ---------------------------------------------

/// The session-start auto-update: called by `crystalline prompt system`
/// before it emits the routing prompt. Every receipt entry that applies here
/// (all user-scope installs, plus project-scope installs recorded for
/// exactly this working directory) and was last reconciled by a different
/// binary version gets its hooks and skills parts re-run with the options
/// that install recorded. Never the MCP part: registration is
/// version-independent and shelling out to a harness CLI from inside a
/// running hook is slow and fragile.
///
/// Fast by requirement: the no-op path (matching versions, or no receipt at
/// all) costs one small-file read and string compares - no hashing, no
/// skill IO and no cwd canonicalize unless a version-mismatched
/// project-scope entry exists.
///
/// Best-effort by design: this runs inside a SessionStart hook, so nothing
/// here may ever break the routing prompt. Every outcome, success or
/// failure, is at most a one-line notice in the returned list.
pub(crate) fn auto_reconcile(current_version: &str, cwd: &Path) -> Vec<String> {
    let Ok(path) = receipt::receipt_path() else {
        return Vec::new();
    };
    let Ok(mut book) = receipt::load(&path) else {
        return Vec::new();
    };

    // Resolved lazily: the canonicalize syscall only happens once a
    // version-mismatched project entry actually needs matching.
    let mut cwd_key: Option<String> = None;
    let mut notices = Vec::new();
    let mut changed = false;
    for entry in &mut book.installs {
        if entry.version == current_version {
            continue;
        }
        let applies = match entry.scope.as_str() {
            "user" => true,
            "project" => {
                let key = cwd_key.get_or_insert_with(|| {
                    cwd.canonicalize()
                        .unwrap_or_else(|_| cwd.to_path_buf())
                        .display()
                        .to_string()
                });
                entry.project_path.as_deref() == Some(key.as_str())
            }
            _ => false,
        };
        if !applies {
            continue;
        }
        let Some(harness) = harness_from_id(&entry.harness) else {
            continue;
        };
        let paths = harness_paths(harness, entry.scope == "project");
        match reconcile_entry(entry, &paths) {
            Ok(()) => {
                notices.push(format!(
                    "[crystalline] Refreshed the {} install ({} -> {current_version}); updated skills load fresh next session.",
                    entry.harness, entry.version
                ));
                entry.version = current_version.to_string();
                changed = true;
            }
            Err(e) => notices.push(format!(
                "[crystalline] Refreshing the {} install to {current_version} failed: {e}. Run `crystalline install {}` by hand.",
                entry.harness, entry.harness
            )),
        }
    }
    if changed && receipt::save(&path, &book).is_err() {
        notices.push(
            "[crystalline] Could not rewrite the install receipt; the refresh may re-run next session."
                .to_string(),
        );
    }
    notices
}

/// Re-run one receipt entry's hooks and skills parts. Project-scope paths
/// from `harness_paths` are relative to the working directory, which is
/// exactly the recorded project directory whenever this is called.
fn reconcile_entry(entry: &mut receipt::InstallRecord, paths: &HarnessPaths) -> anyhow::Result<()> {
    if entry.parts.hooks {
        install_hooks(&paths.settings)?;
    }
    if entry.parts.skills {
        let (_, records) = reconcile_skills(&paths.skills_dir, &entry.skills, ReconcileMode::Auto)?;
        entry.skills = records;
    }
    Ok(())
}

/// The `HarnessKind` for a receipt-recorded id; `None` for an id a future
/// binary wrote that this one does not know.
fn harness_from_id(id: &str) -> Option<HarnessKind> {
    match id {
        "claude-code" => Some(HarnessKind::ClaudeCode),
        "codex" => Some(HarnessKind::Codex),
        _ => None,
    }
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

    // --- skill reconcile ------------------------------------------------

    use crate::receipt::{RecordedSkill, sha256_hex};

    /// The fake current set for reconcile tests: one skill, version 2 body.
    const CUR: &[(&str, &str)] = &[("alpha", "alpha v2 body")];

    fn rec(name: &str, body: &str) -> RecordedSkill {
        RecordedSkill {
            name: name.to_string(),
            sha256: sha256_hex(body.as_bytes()),
        }
    }

    fn skill_file(dir: &Path, name: &str) -> PathBuf {
        dir.join(name).join("SKILL.md")
    }

    fn bak_file(dir: &Path, name: &str) -> PathBuf {
        dir.join(name).join("SKILL.md.bak")
    }

    fn seed(dir: &Path, name: &str, body: &str) {
        let path = skill_file(dir, name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
    }

    fn status_of<'a>(report: &'a SkillsReport, name: &str) -> &'a str {
        report
            .skills
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.status)
            .unwrap_or_else(|| panic!("no report entry for {name}"))
    }

    #[test]
    fn install_mode_installs_a_missing_skill_and_records_it() {
        let dir = tempfile::tempdir().unwrap();
        let (report, records) =
            reconcile_skill_set(dir.path(), CUR, &[], &[], ReconcileMode::Install).unwrap();
        assert_eq!(status_of(&report, "alpha"), "installed");
        assert_eq!(
            std::fs::read_to_string(skill_file(dir.path(), "alpha")).unwrap(),
            "alpha v2 body"
        );
        assert_eq!(records, vec![rec("alpha", "alpha v2 body")]);
    }

    #[test]
    fn auto_mode_respects_a_user_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let prior = [rec("alpha", "alpha v1 body")];
        let (report, records) =
            reconcile_skill_set(dir.path(), CUR, &[], &prior, ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "alpha"), "user-removed");
        assert!(
            !skill_file(dir.path(), "alpha").exists(),
            "never resurrected"
        );
        assert!(records.is_empty(), "the deletion leaves the receipt too");
    }

    #[test]
    fn an_old_clean_copy_is_updated_without_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "alpha", "alpha v1 body");
        let prior = [rec("alpha", "alpha v1 body")];
        let (report, records) =
            reconcile_skill_set(dir.path(), CUR, &[], &prior, ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "alpha"), "updated");
        assert_eq!(
            std::fs::read_to_string(skill_file(dir.path(), "alpha")).unwrap(),
            "alpha v2 body"
        );
        assert!(
            !bak_file(dir.path(), "alpha").exists(),
            "clean copies need no backup"
        );
        assert_eq!(records, vec![rec("alpha", "alpha v2 body")]);
    }

    #[test]
    fn a_user_edited_copy_is_backed_up_then_updated() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "alpha", "my customized alpha");
        let prior = [rec("alpha", "alpha v1 body")];
        let (report, _) =
            reconcile_skill_set(dir.path(), CUR, &[], &prior, ReconcileMode::Install).unwrap();
        assert_eq!(status_of(&report, "alpha"), "updated-backup");
        assert_eq!(
            std::fs::read_to_string(skill_file(dir.path(), "alpha")).unwrap(),
            "alpha v2 body"
        );
        assert_eq!(
            std::fs::read_to_string(bak_file(dir.path(), "alpha")).unwrap(),
            "my customized alpha"
        );
    }

    #[test]
    fn a_mismatch_without_any_receipt_is_backed_up_too() {
        // No receipt at all: overwrite-with-backup is the safe fallback.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "alpha", "who knows what this is");
        let (report, _) =
            reconcile_skill_set(dir.path(), CUR, &[], &[], ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "alpha"), "updated-backup");
        assert_eq!(
            std::fs::read_to_string(bak_file(dir.path(), "alpha")).unwrap(),
            "who knows what this is"
        );
    }

    #[test]
    fn a_backup_overwrites_an_earlier_backup() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "alpha", "second edit");
        std::fs::write(bak_file(dir.path(), "alpha"), "first edit").unwrap();
        let (_, _) = reconcile_skill_set(dir.path(), CUR, &[], &[], ReconcileMode::Auto).unwrap();
        assert_eq!(
            std::fs::read_to_string(bak_file(dir.path(), "alpha")).unwrap(),
            "second edit",
            "the newest divergence wins"
        );
    }

    #[test]
    fn a_receipt_retired_clean_skill_is_removed_with_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "beta", "beta v1 body");
        let prior = [rec("beta", "beta v1 body")];
        let (report, records) =
            reconcile_skill_set(dir.path(), CUR, &[], &prior, ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "beta"), "removed-retired");
        assert!(
            !dir.path().join("beta").exists(),
            "the emptied folder is pruned"
        );
        assert!(records.iter().all(|r| r.name != "beta"));
    }

    #[test]
    fn a_receipt_retired_edited_skill_is_renamed_to_bak() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "beta", "my customized beta");
        let prior = [rec("beta", "beta v1 body")];
        let (report, _) =
            reconcile_skill_set(dir.path(), CUR, &[], &prior, ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "beta"), "retired-backup");
        assert!(
            !skill_file(dir.path(), "beta").exists(),
            "the live skill is gone"
        );
        assert_eq!(
            std::fs::read_to_string(bak_file(dir.path(), "beta")).unwrap(),
            "my customized beta"
        );
    }

    #[test]
    fn a_list_retired_skill_without_a_receipt_is_renamed_to_bak() {
        // Only the static retired list knows this name; without a hash the
        // copy cannot be proven ours, so it is preserved as the backup.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "gamma", "gamma body");
        let (report, _) =
            reconcile_skill_set(dir.path(), CUR, &["gamma"], &[], ReconcileMode::Install).unwrap();
        assert_eq!(status_of(&report, "gamma"), "retired-backup");
        assert_eq!(
            std::fs::read_to_string(bak_file(dir.path(), "gamma")).unwrap(),
            "gamma body"
        );
    }

    #[test]
    fn a_list_retired_skill_with_a_receipt_hash_is_removed_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "gamma", "gamma body");
        let prior = [rec("gamma", "gamma body")];
        let (report, _) =
            reconcile_skill_set(dir.path(), CUR, &["gamma"], &prior, ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "gamma"), "removed-retired");
        assert!(!dir.path().join("gamma").exists());
    }

    #[test]
    fn an_absent_retired_skill_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let prior = [rec("beta", "beta v1 body")];
        let (report, _) =
            reconcile_skill_set(dir.path(), CUR, &["gamma"], &prior, ReconcileMode::Auto).unwrap();
        assert!(
            report
                .skills
                .iter()
                .all(|s| s.name != "beta" && s.name != "gamma")
        );
    }

    #[test]
    fn an_already_current_skill_refreshes_its_record_untouched() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "alpha", "alpha v2 body");
        let (report, records) =
            reconcile_skill_set(dir.path(), CUR, &[], &[], ReconcileMode::Auto).unwrap();
        assert_eq!(status_of(&report, "alpha"), "already-current");
        assert_eq!(records, vec![rec("alpha", "alpha v2 body")]);
    }

    // --- hostile receipt names: the path-escape guard -----------------------
    //
    // The receipt is attacker-writable local state. `is_plain_skill_name` is
    // what stands between a hostile recorded name and `dir.join(name)`; these
    // lock its classification in directly, then prove it end to end against
    // the two loops that consume receipt names.

    #[test]
    fn is_plain_skill_name_rejects_every_hostile_shape() {
        for hostile in ["/tmp/evil", "../escape", "a/b", "..", ""] {
            assert!(
                !is_plain_skill_name(hostile),
                "{hostile:?} must not be usable as a path component"
            );
        }
        assert!(is_plain_skill_name("crystalline-routing"));
    }

    #[test]
    fn reconcile_skips_hostile_receipt_names_leaving_them_untouched() {
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().join("skills");
        std::fs::create_dir_all(&dir).unwrap();

        // Canaries at the exact locations a missing guard would have
        // followed: an absolute path elsewhere in the sandbox, and the
        // sibling directory "../escape" resolves to, one level above `dir`.
        // Both carry a body whose hash matches the hostile record, so an
        // unguarded `retire` would recognize them as "ours" and delete them.
        let absolute_target = work.path().join("elsewhere");
        std::fs::create_dir_all(&absolute_target).unwrap();
        std::fs::write(absolute_target.join("SKILL.md"), "absolute canary").unwrap();
        let escape_target = work.path().join("escape");
        std::fs::create_dir_all(&escape_target).unwrap();
        std::fs::write(escape_target.join("SKILL.md"), "escape canary").unwrap();
        let nested_target = dir.join("a").join("b");

        seed(&dir, "alpha", "alpha v1 body");
        let absolute_name = absolute_target.to_str().unwrap().to_string();
        let prior = vec![
            rec(&absolute_name, "absolute canary"),
            rec("../escape", "escape canary"),
            rec("a/b", "hostile"),
            rec("..", "hostile"),
            rec("", "hostile"),
            rec("alpha", "alpha v1 body"),
        ];
        let (report, _records) =
            reconcile_skill_set(&dir, CUR, &[], &prior, ReconcileMode::Auto).unwrap();

        // The plain sibling name still reconciles normally.
        assert_eq!(status_of(&report, "alpha"), "updated");
        // None of the hostile names produced a report entry.
        for hostile in [absolute_name.as_str(), "../escape", "a/b", "..", ""] {
            assert!(
                report.skills.iter().all(|s| s.name != hostile),
                "{hostile:?} must not appear in the report"
            );
        }
        // No filesystem effect outside the tempdir's own skill folder.
        assert_eq!(
            std::fs::read_to_string(absolute_target.join("SKILL.md")).unwrap(),
            "absolute canary",
            "the absolute name was never followed"
        );
        assert_eq!(
            std::fs::read_to_string(escape_target.join("SKILL.md")).unwrap(),
            "escape canary",
            "the relative escape was never followed"
        );
        assert!(!nested_target.exists(), "the nested name was never created");
    }

    #[test]
    fn uninstall_skips_hostile_receipt_names_leaving_them_untouched() {
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().join("skills");
        std::fs::create_dir_all(&dir).unwrap();

        // A plain leftover the receipt legitimately remembers, to prove the
        // guard does not disturb an ordinary name.
        seed(&dir, "beta", "beta body");
        // A canary at the location a missing guard would have deleted.
        let escape_target = work.path().join("escape");
        std::fs::create_dir_all(&escape_target).unwrap();
        std::fs::write(escape_target.join("SKILL.md"), "escape canary").unwrap();

        let prior = vec![
            rec("beta", "beta body"),
            rec("/tmp/evil", "hostile"),
            rec("../escape", "escape canary"),
            rec("a/b", "hostile"),
            rec("..", "hostile"),
            rec("", "hostile"),
        ];
        let report = uninstall_skills(&dir, &prior, false).unwrap();

        // The plain sibling name still reconciles normally.
        assert_eq!(status_of(&report, "beta"), "removed");
        assert!(!dir.join("beta").exists());
        // None of the hostile names produced a report entry.
        for hostile in ["/tmp/evil", "../escape", "a/b", "..", ""] {
            assert!(
                report.skills.iter().all(|s| s.name != hostile),
                "{hostile:?} must not appear in the report"
            );
        }
        // No filesystem effect outside the tempdir's own skill folder.
        assert_eq!(
            std::fs::read_to_string(escape_target.join("SKILL.md")).unwrap(),
            "escape canary",
            "the relative escape was never followed"
        );
    }
}
