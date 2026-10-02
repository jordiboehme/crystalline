//! End-to-end `apply`/`status` orchestration: a `DomainEntry.provision`
//! decision, the installed-harness gate and the two entry points every
//! surface (cli, MCP tool, session-start reconcile) will call through,
//! wired together against a real receipt file and a temp `HOME`.
//!
//! Unix only: several tests redirect `HOME` to a scratch directory, the same
//! env-isolation technique `tests/reconcile.rs` uses (a shared lock plus
//! preserve-and-restore), since `apply`'s file reconcile resolves a harness's
//! config directory from `HOME`. `apply`/`status` calls that never touch a
//! harness (an empty harness list, or a check that stops before reconciling)
//! do not need it, but every test guards `HOME` anyway for uniformity and to
//! keep the shared lock's ordering obvious.
#![cfg(unix)]

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::Mutex;

use crystalline_core::HarnessKind;
use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_core::provision::reconcile::{ActionStatus, McpOutcome, McpRunner};
use crystalline_core::provision::{self, Decision};

/// No env-defined domains: the exclusion set every `apply`/`status` call in
/// this file passes, since none of these fixtures use an environment
/// overlay.
fn no_env() -> HashSet<&'static str> {
    HashSet::new()
}

/// Serializes every `HOME`-mutating test in this binary.
static HOME_LOCK: Mutex<()> = Mutex::new(());

// --- fixture helpers ---------------------------------------------------------

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn write_manifest(dir: &Path, bullets: &str) {
    let source = format!(
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\n---\n\n\
         # harbor\n\n\
         ## Scope\n\n- Coastal navigation knowledge\n\n\
         ## When to Use\n\n- When docking\n\n\
         ## Provisioning\n\n{bullets}\n"
    );
    std::fs::write(dir.join("MANIFEST.md"), source).unwrap();
}

/// The harbor domain fixture from `tests/provision.rs`: a `tide-tables` skill
/// (`SKILL.md` plus `scripts/chart.sh`), a `charts/plot-route.md` command, a
/// `quartermaster.md` agent and a `lighthouse.json` mcp - four file artifacts
/// and one mcp.
fn write_harbor(dir: &Path) {
    write_manifest(
        dir,
        "- skills: skills\n- commands: commands\n- agents: agents\n- mcps: mcps\n",
    );
    write(
        dir,
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n",
    );
    write(
        dir,
        "skills/tide-tables/scripts/chart.sh",
        "#!/bin/sh\necho charting\n",
    );
    write(
        dir,
        "commands/charts/plot-route.md",
        "Plot a route between two buoys.\n",
    );
    write(
        dir,
        "agents/quartermaster.md",
        "# Quartermaster\n\nKeeps the manifest of stores.\n",
    );
    write(
        dir,
        "mcps/lighthouse.json",
        r#"{"name": "lighthouse", "server": {"type": "http", "url": "https://example.test/mcp"}}"#,
    );
}

fn set_home(path: &Path) -> Option<std::ffi::OsString> {
    let previous = std::env::var_os("HOME");
    // SAFETY: guarded by HOME_LOCK; restored via restore_home before the test
    // returns.
    unsafe {
        std::env::set_var("HOME", path);
    }
    previous
}

fn restore_home(previous: Option<std::ffi::OsString>) {
    match previous {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

/// A single-domain config with `name` opted in or out at `entry_dir`.
fn config_with(name: &str, entry: DomainEntry) -> GlobalConfig {
    let mut global = GlobalConfig::default();
    global.domains.insert(name.to_string(), entry);
    global
}

/// An [`McpRunner`] that records every call and returns scripted outcomes, in
/// order - the same shape `reconcile.rs`'s own test-only runner takes.
struct RecordingRunner {
    calls: Vec<String>,
    add_outcomes: VecDeque<McpOutcome>,
    remove_outcomes: VecDeque<McpOutcome>,
}

impl RecordingRunner {
    fn new() -> RecordingRunner {
        RecordingRunner {
            calls: Vec::new(),
            add_outcomes: VecDeque::new(),
            remove_outcomes: VecDeque::new(),
        }
    }

    fn on_add(mut self, outcome: McpOutcome) -> RecordingRunner {
        self.add_outcomes.push_back(outcome);
        self
    }

    fn on_remove(mut self, outcome: McpOutcome) -> RecordingRunner {
        self.remove_outcomes.push_back(outcome);
        self
    }
}

impl McpRunner for RecordingRunner {
    fn add(&mut self, _harness: HarnessKind, name: &str, _server_json: &str) -> McpOutcome {
        self.calls.push(format!("add:{name}"));
        self.add_outcomes
            .pop_front()
            .unwrap_or_else(McpOutcome::applied)
    }

    fn remove(&mut self, _harness: HarnessKind, name: &str) -> McpOutcome {
        self.calls.push(format!("remove:{name}"));
        self.remove_outcomes
            .pop_front()
            .unwrap_or_else(McpOutcome::applied)
    }
}

/// A runner that must never be called: for tests that provision no MCP
/// servers.
struct NoMcp;
impl McpRunner for NoMcp {
    fn add(&mut self, _h: HarnessKind, _n: &str, _s: &str) -> McpOutcome {
        panic!("no MCP work expected in this test");
    }
    fn remove(&mut self, _h: HarnessKind, _n: &str) -> McpOutcome {
        panic!("no MCP work expected in this test");
    }
}

// --- apply --------------------------------------------------------------------

#[test]
fn apply_opts_in_domain_installs_files_and_stamps_sources() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_harbor(domain_dir.path());

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let global = config_with("harbor", entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let mut runner = RecordingRunner::new().on_add(McpOutcome::applied());

    let previous = set_home(home.path());
    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut runner,
        &no_env(),
    );
    restore_home(previous);
    let report = report.unwrap();

    assert_eq!(report.harnesses.len(), 1);
    let (harness, actions) = &report.harnesses[0];
    assert_eq!(*harness, HarnessKind::ClaudeCode);
    assert_eq!(
        actions
            .iter()
            .filter(|a| a.status == ActionStatus::Installed)
            .count(),
        4,
        "{actions:?}"
    );
    assert!(
        actions
            .iter()
            .any(|a| a.status == ActionStatus::McpAdded && a.target == "lighthouse")
    );
    assert_eq!(runner.calls, vec!["add:lighthouse"]);
    assert!(report.pending.is_empty());

    assert!(
        home.path()
            .join(".claude/skills/tide-tables/SKILL.md")
            .exists()
    );
    assert!(
        home.path()
            .join(".claude/commands/charts/plot-route.md")
            .exists()
    );
    assert!(home.path().join(".claude/agents/quartermaster.md").exists());

    let receipt = provision::receipt::load(&receipt_path).unwrap();
    let harness_state = receipt.harnesses.get("claude-code").unwrap();
    assert_eq!(harness_state.files.len(), 4);
    assert_eq!(harness_state.mcps.len(), 1);

    let sources = receipt.sources.get("harbor").unwrap();
    assert_eq!(sources.files.len(), 4, "{:?}", sources.files);
    assert!(sources.files.contains_key("skills/tide-tables/SKILL.md"));
    assert!(sources.files.contains_key("commands/charts/plot-route.md"));
    assert!(sources.files.contains_key("agents/quartermaster.md"));
}

#[test]
fn apply_opt_out_removes_files_and_drops_source_stamps() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_harbor(domain_dir.path());

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let mut runner = RecordingRunner::new()
        .on_add(McpOutcome::applied())
        .on_remove(McpOutcome::applied());

    let previous = set_home(home.path());

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let allowed = config_with("harbor", entry.clone());
    provision::apply(
        &allowed,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut runner,
        &no_env(),
    )
    .unwrap();
    assert!(
        home.path()
            .join(".claude/skills/tide-tables/SKILL.md")
            .exists()
    );

    entry.provision = Some(false);
    let denied = config_with("harbor", entry);
    let report = provision::apply(
        &denied,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut runner,
        &no_env(),
    )
    .unwrap();

    restore_home(previous);

    assert!(
        report
            .harnesses
            .iter()
            .any(|(_, actions)| actions.iter().any(|a| a.status == ActionStatus::Removed))
    );
    assert!(!home.path().join(".claude/skills/tide-tables").exists());
    assert!(!home.path().join(".claude/commands/charts").exists());
    assert!(!home.path().join(".claude/agents/quartermaster.md").exists());
    assert_eq!(runner.calls, vec!["add:lighthouse", "remove:lighthouse"]);

    let receipt = provision::receipt::load(&receipt_path).unwrap();
    assert!(!receipt.sources.contains_key("harbor"));
    let harness_state = receipt.harnesses.get("claude-code").unwrap();
    assert!(harness_state.files.is_empty());
    assert!(harness_state.mcps.is_empty());
}

#[test]
fn apply_rename_at_source_removes_old_target_and_installs_new() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_manifest(domain_dir.path(), "- skills: skills\n");
    write(domain_dir.path(), "skills/tide-tables/SKILL.md", "tides\n");

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let global = config_with("harbor", entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");

    let previous = set_home(home.path());
    provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();
    assert!(
        home.path()
            .join(".claude/skills/tide-tables/SKILL.md")
            .exists()
    );

    std::fs::rename(
        domain_dir.path().join("skills/tide-tables"),
        domain_dir.path().join("skills/tide-charts"),
    )
    .unwrap();

    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();
    restore_home(previous);

    assert!(!home.path().join(".claude/skills/tide-tables").exists());
    assert!(
        home.path()
            .join(".claude/skills/tide-charts/SKILL.md")
            .exists()
    );
    let (_, actions) = &report.harnesses[0];
    assert!(actions.iter().any(|a| a.status == ActionStatus::Removed));
    assert!(actions.iter().any(|a| a.status == ActionStatus::Installed));
}

#[test]
fn apply_with_no_installed_harnesses_writes_nothing_and_suggests_install() {
    let domain_dir = tempfile::tempdir().unwrap();
    write_manifest(domain_dir.path(), "- skills: skills\n");
    write(domain_dir.path(), "skills/tide-tables/SKILL.md", "tides\n");

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let global = config_with("harbor", entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");

    let report = provision::apply(&global, &receipt_path, &[], &mut NoMcp, &no_env()).unwrap();

    assert!(report.harnesses.is_empty());
    assert!(
        report
            .notices
            .iter()
            .any(|n| n.contains("crystalline install")),
        "{:?}",
        report.notices
    );
    assert!(!receipt_path.exists(), "nothing written");
}

#[test]
fn apply_undecided_declaring_domain_is_pending_and_not_installed() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_harbor(domain_dir.path());

    // provision left at its default None: undecided.
    let global = config_with("harbor", DomainEntry::file(domain_dir.path()));

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");

    let previous = set_home(home.path());
    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();
    restore_home(previous);

    assert_eq!(report.harnesses.len(), 1);
    assert!(
        report.harnesses[0].1.is_empty(),
        "nothing installed for an undecided domain: {:?}",
        report.harnesses[0].1
    );
    assert_eq!(report.pending.len(), 1);
    let pending = &report.pending[0];
    assert_eq!(pending.domain, "harbor");
    assert_eq!(pending.counts.get("skills").copied(), Some(1));
    assert_eq!(pending.counts.get("commands").copied(), Some(1));
    assert_eq!(pending.counts.get("agents").copied(), Some(1));
    assert_eq!(pending.counts.get("mcps").copied(), Some(1));
}

#[test]
fn apply_virtual_domain_opted_in_produces_a_notice_and_nothing_else() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut entry = DomainEntry::virtual_domain();
    entry.provision = Some(true);
    let global = config_with("notes", entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");

    let previous = set_home(home.path());
    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();
    restore_home(previous);

    assert!(report.harnesses[0].1.is_empty());
    assert!(
        report
            .notices
            .iter()
            .any(|n| n.contains("notes") && n.contains("virtual")),
        "{:?}",
        report.notices
    );
    assert!(report.pending.is_empty());
}

#[test]
fn apply_corrupt_receipt_is_regenerated_with_a_notice() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_manifest(domain_dir.path(), "- skills: skills\n");
    write(domain_dir.path(), "skills/tide-tables/SKILL.md", "tides\n");

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let global = config_with("harbor", entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    std::fs::write(&receipt_path, "{ not json").unwrap();

    let previous = set_home(home.path());
    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut NoMcp,
        &no_env(),
    );
    restore_home(previous);
    let report = report.unwrap();

    assert!(
        report
            .notices
            .iter()
            .any(|n| n.contains("rebuilt from empty")),
        "{:?}",
        report.notices
    );
    assert!(
        home.path()
            .join(".claude/skills/tide-tables/SKILL.md")
            .exists()
    );
    let receipt = provision::receipt::load(&receipt_path).unwrap();
    assert!(receipt.harnesses.contains_key("claude-code"));
}

// --- status ---------------------------------------------------------------

#[test]
fn status_reflects_decisions_counts_and_edits_without_writing() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_harbor(domain_dir.path());

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let global = config_with("harbor", entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let mut runner = RecordingRunner::new().on_add(McpOutcome::applied());

    let previous = set_home(home.path());
    provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut runner,
        &no_env(),
    )
    .unwrap();

    let before_bytes = std::fs::read(&receipt_path).unwrap();
    let before_mtime = std::fs::metadata(&receipt_path)
        .unwrap()
        .modified()
        .unwrap();

    // A user edit to an installed file.
    std::fs::write(
        home.path().join(".claude/agents/quartermaster.md"),
        "EDITED\n",
    )
    .unwrap();

    let report = provision::status(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &no_env(),
    )
    .unwrap();
    restore_home(previous);

    let domain_status = report
        .domains
        .iter()
        .find(|d| d.domain == "harbor")
        .unwrap();
    assert_eq!(domain_status.decision, Decision::Allowed);
    assert!(domain_status.declares);
    assert!(!domain_status.is_virtual);
    assert_eq!(domain_status.parse_problems, 0);
    assert_eq!(domain_status.counts.get("skills").copied(), Some(1));
    assert_eq!(domain_status.counts.get("commands").copied(), Some(1));
    assert_eq!(domain_status.counts.get("agents").copied(), Some(1));
    assert_eq!(domain_status.counts.get("mcps").copied(), Some(1));

    assert_eq!(report.harnesses.len(), 1);
    let harness_status = &report.harnesses[0];
    assert_eq!(harness_status.harness, HarnessKind::ClaudeCode);
    assert_eq!(harness_status.installed_files, 4);
    assert_eq!(harness_status.installed_mcps, 1);
    assert_eq!(harness_status.edited, 1);
    assert_eq!(harness_status.missing, 0);

    assert!(report.pending.is_empty());
    assert!(report.virtual_with_decision.is_empty());

    let after_bytes = std::fs::read(&receipt_path).unwrap();
    let after_mtime = std::fs::metadata(&receipt_path)
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before_bytes, after_bytes, "status must never write");
    assert_eq!(before_mtime, after_mtime, "status must never write");
}

#[test]
fn status_reports_missing_files_and_virtual_decisions() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let domain_dir = tempfile::tempdir().unwrap();
    write_manifest(domain_dir.path(), "- skills: skills\n");
    write(domain_dir.path(), "skills/tide-tables/SKILL.md", "tides\n");

    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let mut global = config_with("harbor", entry);
    let mut virtual_entry = DomainEntry::virtual_domain();
    virtual_entry.provision = Some(false);
    global.domains.insert("notes".to_string(), virtual_entry);

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");

    let previous = set_home(home.path());
    provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();

    // Delete the installed file locally.
    std::fs::remove_file(home.path().join(".claude/skills/tide-tables/SKILL.md")).unwrap();

    let report = provision::status(
        &global,
        &receipt_path,
        &[HarnessKind::ClaudeCode],
        &no_env(),
    )
    .unwrap();
    restore_home(previous);

    let harness_status = &report.harnesses[0];
    assert_eq!(harness_status.missing, 1);
    assert_eq!(harness_status.edited, 0);

    let notes_status = report.domains.iter().find(|d| d.domain == "notes").unwrap();
    assert!(notes_status.is_virtual);
    assert_eq!(notes_status.decision, Decision::Denied);
    assert!(notes_status.counts.is_empty());
    assert_eq!(report.virtual_with_decision, vec!["notes".to_string()]);
}

// --- shared skills folders: one owner ------------------------------------------

fn skills_only_domain() -> (tempfile::TempDir, GlobalConfig) {
    let domain_dir = tempfile::tempdir().unwrap();
    write_manifest(domain_dir.path(), "- skills: skills\n");
    write(
        domain_dir.path(),
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nReads the tides.\n",
    );
    let mut entry = DomainEntry::file(domain_dir.path());
    entry.provision = Some(true);
    let global = config_with("harbor", entry);
    (domain_dir, global)
}

#[test]
fn codex_and_gemini_share_one_skills_folder_with_one_owner() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let both = [HarnessKind::Codex, HarnessKind::Gemini];

    let previous = set_home(home.path());
    let report = provision::apply(&global, &receipt_path, &both, &mut NoMcp, &no_env());
    let status = provision::status(&global, &receipt_path, &both, &no_env());
    restore_home(previous);
    let report = report.unwrap();
    let status = status.unwrap();

    assert!(
        home.path()
            .join(".agents/skills/tide-tables/SKILL.md")
            .is_file()
    );
    let installed: Vec<_> = report
        .harnesses
        .iter()
        .map(|(h, a)| {
            (
                *h,
                a.iter()
                    .filter(|a| a.status == ActionStatus::Installed)
                    .count(),
            )
        })
        .collect();
    assert_eq!(
        installed,
        vec![(HarnessKind::Codex, 1), (HarnessKind::Gemini, 0)]
    );

    let codex = &status.harnesses[0];
    assert_eq!(codex.covered_by, None);
    assert_eq!(codex.installed_files, 1);
    let gemini = &status.harnesses[1];
    assert_eq!(gemini.covered_by, Some(HarnessKind::Codex));
    assert_eq!(
        (
            gemini.installed_files,
            gemini.installed_mcps,
            gemini.edited,
            gemini.missing,
            gemini.drift,
            gemini.orphaned
        ),
        (0, 0, 0, 0, 0, 0)
    );
}

#[test]
fn when_the_owner_leaves_the_next_harness_adopts_the_identical_files() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let skill = home.path().join(".agents/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::Codex, HarnessKind::Gemini],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();
    let before = std::fs::read(&skill).unwrap();
    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::Gemini],
        &mut NoMcp,
        &no_env(),
    );
    let status = provision::status(&global, &receipt_path, &[HarnessKind::Gemini], &no_env());
    restore_home(previous);
    let report = report.unwrap();

    assert_eq!(std::fs::read(&skill).unwrap(), before, "nothing rewritten");
    let (harness, actions) = &report.harnesses[0];
    assert_eq!(*harness, HarnessKind::Gemini);
    assert!(
        actions.is_empty(),
        "records handed over, nothing to do: {actions:?}"
    );
    let status = status.unwrap();
    assert_eq!(status.harnesses[0].covered_by, None);
    assert_eq!(status.harnesses[0].installed_files, 1);

    // Without a record to hand over, the identical file is adopted.
    std::fs::remove_file(&receipt_path).unwrap();
    let previous = set_home(home.path());
    let report = provision::apply(
        &global,
        &receipt_path,
        &[HarnessKind::Gemini],
        &mut NoMcp,
        &no_env(),
    )
    .unwrap();
    restore_home(previous);
    assert_eq!(report.harnesses[0].1.len(), 1);
    assert_eq!(report.harnesses[0].1[0].status, ActionStatus::Adopted);
    assert_eq!(std::fs::read(&skill).unwrap(), before);
}

#[test]
fn cursor_is_covered_by_claude_code_and_apply_leaves_its_state_empty() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let both = [HarnessKind::ClaudeCode, HarnessKind::Cursor];

    let previous = set_home(home.path());
    provision::apply(&global, &receipt_path, &both, &mut NoMcp, &no_env()).unwrap();
    let status = provision::status(&global, &receipt_path, &both, &no_env());
    restore_home(previous);
    let status = status.unwrap();

    assert!(
        home.path()
            .join(".claude/skills/tide-tables/SKILL.md")
            .is_file()
    );
    assert!(!home.path().join(".agents/skills").exists());
    assert_eq!(
        status.harnesses[1].covered_by,
        Some(HarnessKind::ClaudeCode)
    );
    assert_eq!(status.harnesses[1].installed_files, 0);
}

// --- shared skills folders: ownership moves ------------------------------------

use HarnessKind::{ClaudeCode, Codex, Cursor, Gemini};

fn run_apply(
    global: &GlobalConfig,
    receipt: &Path,
    harnesses: &[HarnessKind],
) -> provision::ApplyReport {
    provision::apply(global, receipt, harnesses, &mut NoMcp, &no_env()).unwrap()
}

fn deny(global: &mut GlobalConfig) {
    global.domains.get_mut("harbor").unwrap().provision = Some(false);
}

fn statuses(report: &provision::ApplyReport, h: HarnessKind) -> Vec<ActionStatus> {
    report
        .harnesses
        .iter()
        .filter(|(k, _)| *k == h)
        .flat_map(|(_, a)| a.iter().map(|a| a.status))
        .collect()
}

#[test]
fn a_cursor_copy_is_retired_when_claude_code_starts_covering_it() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, mut global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let own = home.path().join(".agents/skills/tide-tables/SKILL.md");
    let cover = home.path().join(".claude/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[Cursor]);
    assert!(own.is_file());
    let report = run_apply(&global, &receipt, &[ClaudeCode, Cursor]);
    let (own_after, cover_after) = (own.exists(), cover.is_file());
    assert_eq!(statuses(&report, Cursor), vec![ActionStatus::Removed]);
    deny(&mut global);
    run_apply(&global, &receipt, &[ClaudeCode, Cursor]);
    let (own_denied, cover_denied) = (own.exists(), cover.exists());
    restore_home(previous);

    assert!(!own_after, "no duplicate copy in the folder Cursor writes");
    assert!(cover_after);
    assert!(!own_denied && !cover_denied, "deny leaves nothing behind");
}

#[test]
fn deny_right_after_gemini_gets_a_new_owner_leaves_no_orphan() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, mut global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let skill = home.path().join(".agents/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[Gemini]);
    assert!(skill.is_file());
    deny(&mut global);
    run_apply(&global, &receipt, &[Codex, Gemini]);
    let left = skill.exists();
    restore_home(previous);

    assert!(!left, "the new owner retired what the covered one wrote");
}

#[test]
fn deny_right_after_the_owner_leaves_removes_the_shared_copy() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, mut global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let skill = home.path().join(".agents/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[Codex, Gemini]);
    deny(&mut global);
    run_apply(&global, &receipt, &[Gemini]);
    let left = skill.exists();
    restore_home(previous);

    assert!(!left);
}

#[test]
fn a_returning_owner_reports_no_false_drift_and_writes_no_backup() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let both = [Codex, Gemini];
    let skill_dir = home.path().join(".agents/skills/tide-tables");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &both);
    run_apply(&global, &receipt, &[Gemini]);
    write(dir.path(), "skills/tide-tables/SKILL.md", "newer tides\n");
    run_apply(&global, &receipt, &[Gemini]);
    let status = provision::status(&global, &receipt, &both, &no_env()).unwrap();
    let report = run_apply(&global, &receipt, &both);
    let backup = skill_dir.join("SKILL.md.bak").exists();
    let content = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    restore_home(previous);

    let codex = &status.harnesses[0];
    assert_eq!((codex.edited, codex.drift, codex.orphaned), (0, 0, 0));
    assert_eq!(codex.installed_files, 1);
    assert!(statuses(&report, Codex).is_empty(), "{report:?}");
    assert!(!backup);
    assert_eq!(content, "newer tides\n");
}

#[test]
fn a_stale_record_never_moves_a_persons_own_file_aside() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, mut global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let skill_dir = home.path().join(".agents/skills/tide-tables");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[Codex, Gemini]);
    run_apply(&global, &receipt, &[Gemini]);
    deny(&mut global);
    run_apply(&global, &receipt, &[Gemini]);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "mine\n").unwrap();
    run_apply(&global, &receipt, &[Codex, Gemini]);
    let kept = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    let backup = skill_dir.join("SKILL.md.bak").exists();
    restore_home(previous);

    assert_eq!(kept, "mine\n");
    assert!(!backup);
}

#[test]
fn session_start_runs_the_adoption_after_the_owner_leaves() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[Codex, Gemini]);
    provision::session_notices(&global, &receipt, &[Gemini], &no_env());
    restore_home(previous);

    let state = provision::load(&receipt).unwrap();
    assert_eq!(state.harnesses["gemini"].files.len(), 1);
    assert!(
        state.harnesses["codex"]
            .files
            .keys()
            .all(|k| !k.starts_with("skills/"))
    );
}

// --- a departed cover: its copy is retired, not left for Cursor to read -------

/// Whether the receipt row of `h` still records any skills file.
fn records_skills(receipt: &Path, h: HarnessKind) -> bool {
    provision::load(receipt)
        .unwrap()
        .harnesses
        .get(h.id())
        .is_some_and(|st| st.files.keys().any(|k| k.starts_with("skills/")))
}

#[test]
fn a_departed_covers_copy_is_retired_and_a_later_deny_leaves_nothing_cursor_reads() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, mut global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let cover = home.path().join(".claude/skills/tide-tables/SKILL.md");
    let own = home.path().join(".agents/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[ClaudeCode, Cursor]);
    let covered = (cover.is_file(), own.exists());
    let before = provision::status(&global, &receipt, &[Cursor], &no_env()).unwrap();
    // Claude Code is uninstalled: Cursor still reads ~/.claude/skills.
    let report = run_apply(&global, &receipt, &[Cursor]);
    let moved = (cover.exists(), own.is_file());
    let claude_records = records_skills(&receipt, ClaudeCode);
    let after = provision::status(&global, &receipt, &[Cursor], &no_env()).unwrap();
    let again = run_apply(&global, &receipt, &[Cursor]);
    deny(&mut global);
    run_apply(&global, &receipt, &[Cursor]);
    let denied = (cover.exists(), own.exists());
    restore_home(previous);

    assert_eq!(covered, (true, false));
    assert_eq!(
        before.stranded,
        vec![provision::StrandedSkills {
            harness: ClaudeCode,
            read_by: Cursor,
            folder: home.path().join(".claude/skills"),
            files: 1,
        }]
    );
    assert_eq!(statuses(&report, ClaudeCode), vec![ActionStatus::Removed]);
    assert_eq!(statuses(&report, Cursor), vec![ActionStatus::Installed]);
    assert_eq!(moved, (false, true), "Cursor gets its own copy instead");
    assert!(!claude_records, "the departed row records no skills");
    assert!(after.stranded.is_empty());
    assert!(
        again.harnesses.iter().all(|(_, a)| a.is_empty()),
        "a second run changes nothing: {again:?}"
    );
    assert_eq!(denied, (false, false), "deny leaves nothing Cursor reads");
}

#[test]
fn a_departed_covers_edited_copy_is_kept_as_a_backup() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let cover = home.path().join(".claude/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[ClaudeCode, Cursor]);
    std::fs::write(&cover, "my own tides\n").unwrap();
    let report = run_apply(&global, &receipt, &[Cursor]);
    let backup = std::fs::read_to_string(cover.with_file_name("SKILL.md.bak"));
    let left = cover.exists();
    let claude_records = records_skills(&receipt, ClaudeCode);
    restore_home(previous);

    assert_eq!(
        statuses(&report, ClaudeCode),
        vec![ActionStatus::RetiredBackup]
    );
    assert_eq!(backup.unwrap(), "my own tides\n");
    assert!(!left);
    assert!(!claude_records);
}

#[test]
fn session_start_retires_a_departed_covers_copy() {
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let cover = home.path().join(".claude/skills/tide-tables/SKILL.md");
    let own = home.path().join(".agents/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[ClaudeCode, Cursor]);
    provision::session_notices(&global, &receipt, &[Cursor], &no_env());
    let state = (cover.exists(), own.is_file());
    restore_home(previous);

    assert_eq!(state, (false, true));
    assert!(!records_skills(&receipt, ClaudeCode));
}

#[test]
fn a_departed_harness_nobody_reads_keeps_its_records() {
    // Outside the ruling: no installed harness reads ~/.claude/skills, so the
    // departed row is left alone, as before.
    let _guard = HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let (_dir, global) = skills_only_domain();
    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt = receipt_dir.path().join("provisions.json");
    let cover = home.path().join(".claude/skills/tide-tables/SKILL.md");

    let previous = set_home(home.path());
    run_apply(&global, &receipt, &[ClaudeCode, Codex]);
    let report = run_apply(&global, &receipt, &[Codex]);
    let status = provision::status(&global, &receipt, &[Codex], &no_env()).unwrap();
    let left = cover.is_file();
    restore_home(previous);

    assert!(statuses(&report, ClaudeCode).is_empty());
    assert!(left);
    assert!(records_skills(&receipt, ClaudeCode));
    assert!(status.stranded.is_empty());
}

// --- pending: env-defined domains never nag ---------------------------------

/// An env-defined domain's `provision` field always reads back `None` (the
/// overlay re-inserts a fresh entry on every effective-config recompute), so
/// without the `env_domains` exclusion it would surface as pending forever
/// even though `allow`/`deny` both refuse it. Two undecided declaring
/// domains, one named in `env_domains`: only the other one is pending, in
/// both `status` and `apply`.
#[test]
fn env_defined_domain_never_appears_pending_while_a_plain_undecided_domain_still_does() {
    let env_dir = tempfile::tempdir().unwrap();
    write_manifest(env_dir.path(), "- skills: skills\n");
    write(env_dir.path(), "skills/tide-tables/SKILL.md", "tides\n");

    let plain_dir = tempfile::tempdir().unwrap();
    write_manifest(plain_dir.path(), "- skills: skills\n");
    write(plain_dir.path(), "skills/lookout/SKILL.md", "watch\n");

    let mut global = GlobalConfig::default();
    global
        .domains
        .insert("harbor-env".to_string(), DomainEntry::file(env_dir.path()));
    global
        .domains
        .insert("cove".to_string(), DomainEntry::file(plain_dir.path()));

    let receipt_dir = tempfile::tempdir().unwrap();
    let receipt_path = receipt_dir.path().join("provisions.json");
    let env_domains: HashSet<&str> = HashSet::from(["harbor-env"]);

    let status = provision::status(&global, &receipt_path, &[], &env_domains).unwrap();
    assert_eq!(
        status.pending.iter().map(|p| &p.domain).collect::<Vec<_>>(),
        vec!["cove"],
        "{:?}",
        status.pending
    );

    let report = provision::apply(&global, &receipt_path, &[], &mut NoMcp, &env_domains).unwrap();
    assert_eq!(
        report.pending.iter().map(|p| &p.domain).collect::<Vec<_>>(),
        vec!["cove"],
        "{:?}",
        report.pending
    );
}

// --- any_domain_declares (end-to-end alongside apply/status) ----------------

#[test]
fn any_domain_declares_true_and_false_fixtures() {
    let declaring = tempfile::tempdir().unwrap();
    write_manifest(declaring.path(), "- skills: skills\n");
    let mut declares_global = GlobalConfig::default();
    declares_global
        .domains
        .insert("harbor".to_string(), DomainEntry::file(declaring.path()));
    assert!(provision::any_domain_declares(&declares_global));

    let unreadable = tempfile::tempdir().unwrap();
    // No MANIFEST.md at all: unreadable.
    let mut plain_global = GlobalConfig::default();
    plain_global
        .domains
        .insert("empty".to_string(), DomainEntry::file(unreadable.path()));
    plain_global
        .domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    assert!(!provision::any_domain_declares(&plain_global));
}
