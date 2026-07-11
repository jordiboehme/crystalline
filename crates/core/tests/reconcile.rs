//! End-to-end reconcile through the public [`reconcile_harness`] entry point,
//! proving the wiring from a desired key to a real on-disk path: a Claude Code
//! reconcile must route a `skills/...` key under `~/.claude/skills` and a
//! `commands/...` key under `~/.claude/commands`; a Codex reconcile must land
//! a flattened prompt under `~/.codex/prompts` and a rendered agent TOML under
//! `~/.codex/agents`; a GitHub Copilot reconcile must land a rendered agent
//! markdown under its home's `agents` folder - all resolved from the same
//! `HOME` (and `COPILOT_HOME`) the rest of the crate reads.
//!
//! Unix only: every test here redirects `HOME` to a scratch directory, the
//! same env-isolation technique the crate's own `config` tests use (a shared
//! lock plus preserve-and-restore), and `HOME` is the home source on unix.
#![cfg(unix)]

use std::path::Path;
use std::sync::Mutex;

use crystalline_core::manifest::ArtifactType;
use crystalline_core::provision::receipt::sha256_hex;
use crystalline_core::{
    ActionStatus, DesiredFile, DesiredPayload, DesiredSet, DomainArtifacts, HarnessKind,
    HarnessState, McpOutcome, McpRunner, desired_set, reconcile_harness, scan_domain,
};

/// Serializes every `HOME`-mutating test in this binary, so a concurrent test
/// never observes a half-set environment.
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// A runner that must never be called: this test provisions no MCP servers.
struct NoMcp;
impl McpRunner for NoMcp {
    fn add(&mut self, _h: HarnessKind, _n: &str, _s: &str) -> McpOutcome {
        panic!("no MCP work expected in this test");
    }
    fn remove(&mut self, _h: HarnessKind, _n: &str) -> McpOutcome {
        panic!("no MCP work expected in this test");
    }
}

fn desired_file(source_dir: &std::path::Path, name: &str, content: &str) -> DesiredFile {
    let src = source_dir.join(name);
    std::fs::create_dir_all(src.parent().unwrap()).unwrap();
    std::fs::write(&src, content).unwrap();
    DesiredFile {
        domain: "harbor".to_string(),
        payload: DesiredPayload::File(src),
        sha256: sha256_hex(content.as_bytes()),
    }
}

#[test]
fn claude_code_reconcile_routes_skills_and_commands_under_dot_claude() {
    let _guard = HOME_LOCK.lock().unwrap();
    let previous = std::env::var_os("HOME");
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();

    let mut desired = DesiredSet::default();
    desired.files.insert(
        "skills/tide-tables/SKILL.md".to_string(),
        desired_file(src.path(), "skill", "SKILL\n"),
    );
    desired.files.insert(
        "commands/charts/plot-route.md".to_string(),
        desired_file(src.path(), "command", "ROUTE\n"),
    );
    let mut state = HarnessState::default();

    // SAFETY: guarded by HOME_LOCK; restored below before the test returns.
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let result = reconcile_harness(HarnessKind::ClaudeCode, &desired, &mut state, &mut NoMcp);
    // Restore HOME before any assertion, so a failing assertion never leaks the
    // scratch HOME into the rest of the binary.
    match previous {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    let (actions, notices) = result.unwrap();
    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(actions.len(), 2);
    assert!(actions.iter().all(|a| a.status == ActionStatus::Installed));

    // The skills key landed under ~/.claude/skills and the commands key under
    // ~/.claude/commands: two different bases, routed by kind.
    assert_eq!(
        std::fs::read_to_string(home.path().join(".claude/skills/tide-tables/SKILL.md")).unwrap(),
        "SKILL\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join(".claude/commands/charts/plot-route.md")).unwrap(),
        "ROUTE\n"
    );
}

// --- the newly wired Codex and Copilot file pairs, end to end ----------------
//
// Each test drives the full pipeline - scan_domain -> desired_set ->
// reconcile_harness - against a scratch HOME, proving the translated key
// resolves onto the right harness directory and the bytes on disk are exactly
// the desired payload's, with the receipt row recorded at that payload's hash.

/// Write `content` at `rel` under `dir`, creating parents.
fn write_fixture(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
}

/// Scan `dir`'s `commands` and `agents` folders as the `harbor` domain.
fn scan_fixture(dir: &Path) -> DomainArtifacts {
    let roots = vec![
        (ArtifactType::Commands, dir.join("commands")),
        (ArtifactType::Agents, dir.join("agents")),
    ];
    let (artifacts, _notices) = scan_domain("harbor", &roots);
    artifacts
}

/// Run one reconcile with `HOME` redirected to `home` and, when given,
/// `COPILOT_HOME` redirected too - both mutated and restored under the shared
/// lock, before any assertion can fail - returning what `reconcile_harness`
/// returned.
fn reconcile_with_home(
    harness: HarnessKind,
    home: &Path,
    copilot_home: Option<&Path>,
    desired: &DesiredSet,
    state: &mut HarnessState,
) -> anyhow::Result<(Vec<crystalline_core::ArtifactAction>, Vec<String>)> {
    let _guard = HOME_LOCK.lock().unwrap();
    let previous_home = std::env::var_os("HOME");
    let previous_copilot = std::env::var_os("COPILOT_HOME");
    // SAFETY: guarded by HOME_LOCK; restored below before this returns.
    unsafe {
        std::env::set_var("HOME", home);
        if let Some(dir) = copilot_home {
            std::env::set_var("COPILOT_HOME", dir);
        }
    }
    let result = reconcile_harness(harness, desired, state, &mut NoMcp);
    // SAFETY: still under HOME_LOCK; restores exactly what was saved above.
    unsafe {
        match previous_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        if copilot_home.is_some() {
            match previous_copilot {
                Some(v) => std::env::set_var("COPILOT_HOME", v),
                None => std::env::remove_var("COPILOT_HOME"),
            }
        }
    }
    result
}

#[test]
fn codex_reconcile_writes_the_flattened_prompt_under_dot_codex() {
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    const COMMAND: &str = "---\ndescription: Plot a route\n---\n\nPlot a route.\n";
    write_fixture(src.path(), "commands/charts/plot-route.md", COMMAND);
    let artifacts = scan_fixture(src.path());

    let (desired, _notices) = desired_set(HarnessKind::Codex, std::slice::from_ref(&artifacts));
    let mut state = HarnessState::default();
    let (actions, notices) =
        reconcile_with_home(HarnessKind::Codex, home.path(), None, &desired, &mut state).unwrap();

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].status, ActionStatus::Installed);

    // The nested command flattened and landed in Codex's flat prompt dir, byte
    // identical to its source, and the receipt owns it at the source hash.
    let target = home.path().join(".codex/prompts/charts-plot-route.md");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), COMMAND);
    assert_eq!(
        state.files["commands/charts-plot-route.md"].sha256,
        sha256_hex(COMMAND.as_bytes())
    );
}

#[test]
fn codex_reconcile_writes_the_rendered_agent_toml_under_dot_codex() {
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_fixture(
        src.path(),
        "agents/quartermaster.md",
        "---\nname: quartermaster\ndescription: Keeps the stores\nmodel: opus\n---\n\nKeep the stores in order.\n",
    );
    let artifacts = scan_fixture(src.path());

    let (desired, _notices) = desired_set(HarnessKind::Codex, std::slice::from_ref(&artifacts));
    let want = &desired.files["agents/quartermaster.toml"];
    let DesiredPayload::Rendered(expected) = &want.payload else {
        panic!("a markdown agent renders for Codex, got {:?}", want.payload);
    };

    let mut state = HarnessState::default();
    let (actions, notices) =
        reconcile_with_home(HarnessKind::Codex, home.path(), None, &desired, &mut state).unwrap();

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].status, ActionStatus::Installed);

    // The rendered TOML landed under ~/.codex/agents, byte identical to the
    // desired payload, and the receipt owns it at the rendered hash.
    let target = home.path().join(".codex/agents/quartermaster.toml");
    assert_eq!(&std::fs::read(&target).unwrap(), expected);
    assert_eq!(state.files["agents/quartermaster.toml"].sha256, want.sha256);
}

#[test]
fn copilot_reconcile_writes_the_rendered_agent_md_under_copilot_home() {
    let home = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    write_fixture(
        src.path(),
        "agents/reviewer.toml",
        "name = \"reviewer\"\ndescription = \"Reviews code\"\ndeveloper_instructions = '''\nBe thorough.\n'''\n",
    );
    let artifacts = scan_fixture(src.path());

    let (desired, _notices) = desired_set(HarnessKind::Copilot, std::slice::from_ref(&artifacts));
    let want = &desired.files["agents/reviewer.md"];
    let DesiredPayload::Rendered(expected) = &want.payload else {
        panic!("a TOML agent renders for Copilot, got {:?}", want.payload);
    };

    let mut state = HarnessState::default();
    // Copilot resolves its home through COPILOT_HOME first, so pin it to the
    // scratch home rather than relying on the ~/.copilot fallback.
    let copilot_home = home.path().join(".copilot");
    let (actions, notices) = reconcile_with_home(
        HarnessKind::Copilot,
        home.path(),
        Some(&copilot_home),
        &desired,
        &mut state,
    )
    .unwrap();

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].status, ActionStatus::Installed);

    // The rendered markdown agent landed under Copilot's agents folder, byte
    // identical to the desired payload, and the receipt owns it at that hash.
    let target = copilot_home.join("agents/reviewer.md");
    assert_eq!(&std::fs::read(&target).unwrap(), expected);
    assert_eq!(state.files["agents/reviewer.md"].sha256, want.sha256);
}
