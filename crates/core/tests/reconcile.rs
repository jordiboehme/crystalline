//! End-to-end reconcile through the public [`reconcile_harness`] entry point,
//! proving the wiring from a desired key to a real on-disk path: a Claude Code
//! reconcile must route a `skills/...` key under `~/.claude/skills` and a
//! `commands/...` key under `~/.claude/commands`, resolved from the same
//! `HOME` the rest of the crate reads.
//!
//! Unix only: the one test here redirects `HOME` to a scratch directory, the
//! same env-isolation technique the crate's own `config` tests use (a shared
//! lock plus preserve-and-restore), and `HOME` is the home source on unix.
#![cfg(unix)]

use std::sync::Mutex;

use crystalline_core::provision::receipt::sha256_hex;
use crystalline_core::{
    ActionStatus, DesiredFile, DesiredSet, HarnessKind, HarnessState, McpOutcome, McpRunner,
    reconcile_harness,
};

/// Serializes every `HOME`-mutating test in this binary, so a concurrent test
/// never observes a half-set environment. This file has one such test today;
/// the lock keeps that true if more are added.
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
        source: src,
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
