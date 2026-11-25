//! A decision that picks single artifacts (`except`, `also`, `only`) and the
//! optional mark, end to end through `apply`, `status` and the session path.
//!
//! Every test runs once for each entry of [`HarnessKind::ALL`], each in a
//! sandbox of its own: one harness per apply, so no shared skills folder lets
//! one harness cover another and hide what it would have installed. Where a
//! harness has no surface for an artifact kind ([`harness_supports`]), the
//! expected set leaves that kind out, so the assertion still says something
//! for that harness: the artifact is absent because the harness cannot carry
//! it, and its siblings of the kinds it can carry are checked as usual.
//!
//! The installed set is read from the receipt rows, and every file row is
//! checked to exist on disk under the harness's own folder, so a row and the
//! file it stands for cannot disagree unnoticed.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::*;
use crystalline_core::ArtifactType;
use crystalline_core::config::Provision;
use crystalline_core::harness::artifact_base;
use crystalline_core::provision::{harness_supports, parse_artifact_key};

// --- sandbox --------------------------------------------------------------------

/// A temp `HOME` (with `COPILOT_HOME` unset, so Copilot resolves under it too)
/// and a temp receipt folder, held under [`HOME_LOCK`]. Dropping it restores
/// both variables before the lock is released, also when an assertion fails.
struct Sandbox {
    _home: tempfile::TempDir,
    receipt_dir: tempfile::TempDir,
    previous_home: Option<OsString>,
    previous_copilot: Option<OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Sandbox {
    fn new() -> Sandbox {
        let guard = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = tempfile::tempdir().unwrap();
        let receipt_dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("HOME");
        let previous_copilot = std::env::var_os("COPILOT_HOME");
        // SAFETY: guarded by HOME_LOCK; restored in Drop before the guard goes.
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::remove_var("COPILOT_HOME");
        }
        Sandbox {
            _home: home,
            receipt_dir,
            previous_home,
            previous_copilot,
            _guard: guard,
        }
    }

    fn receipt(&self) -> PathBuf {
        self.receipt_dir.path().join("provisions.json")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // SAFETY: still under HOME_LOCK (the guard field drops after this).
        unsafe {
            match self.previous_home.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match self.previous_copilot.take() {
                Some(v) => std::env::set_var("COPILOT_HOME", v),
                None => std::env::remove_var("COPILOT_HOME"),
            }
        }
    }
}

// --- fixture ----------------------------------------------------------------------

/// Every artifact the `harbor` fleet domain ships: two of each kind, so a
/// test can leave one out and check that its sibling still installs.
const FLEET: &[&str] = &[
    "skills/tide-tables",
    "skills/storm-warnings",
    "commands/charts/plot-route",
    "commands/log-entry",
    "agents/quartermaster",
    "agents/navigator",
    "mcps/lighthouse",
    "mcps/foghorn",
];

fn mark(optional: bool) -> &'static str {
    if optional {
        "metadata:\n  provision: optional\n"
    } else {
        ""
    }
}

/// Write the source of the artifact `key` into the domain at `dir`, marked
/// optional or not. A skill also gets a script beside its `SKILL.md` when it
/// is `tide-tables`, so one key covers two files.
fn write_artifact(dir: &Path, key: &str, optional: bool) {
    let (kind, name) = key.split_once('/').unwrap();
    match kind {
        "skills" => {
            write(
                dir,
                &format!("skills/{name}/SKILL.md"),
                &format!(
                    "---\nname: {name}\ndescription: The {name} skill.\n{}---\n\nBody of {name}.\n",
                    mark(optional)
                ),
            );
            if name == "tide-tables" {
                write(
                    dir,
                    "skills/tide-tables/scripts/chart.sh",
                    "#!/bin/sh\necho charting\n",
                );
            }
        }
        "commands" => write(
            dir,
            &format!("commands/{name}.md"),
            &format!(
                "---\ndescription: The {name} command.\n{}---\n\nRun {name}.\n",
                mark(optional)
            ),
        ),
        "agents" => write(
            dir,
            &format!("agents/{name}.md"),
            &format!(
                "---\nname: {name}\ndescription: The {name} agent.\n{}---\n\nYou are the {name}.\n",
                mark(optional)
            ),
        ),
        "mcps" => {
            let metadata = if optional {
                r#""metadata": {"provision": "optional"}, "#
            } else {
                ""
            };
            write(
                dir,
                &format!("mcps/{name}.json"),
                &format!(
                    r#"{{"name": "{name}", {metadata}"server": {{"type": "http", "url": "https://example.test/{name}"}}}}"#
                ),
            );
        }
        other => panic!("unknown kind {other}"),
    }
}

/// The fleet domain, with the keys in `optional` marked optional.
fn fleet_domain(optional: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path(),
        "- skills: skills\n- commands: commands\n- agents: agents\n- mcps: mcps\n",
    );
    for key in FLEET {
        write_artifact(dir.path(), key, optional.contains(key));
    }
    dir
}

fn names(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

fn except(keys: &[&str]) -> Provision {
    Provision::All {
        except: names(keys),
        also: Vec::new(),
    }
}

fn also(keys: &[&str]) -> Provision {
    Provision::All {
        except: Vec::new(),
        also: names(keys),
    }
}

fn only(keys: &[&str]) -> Provision {
    Provision::Only(names(keys))
}

/// A one-domain config: `harbor` at `dir` with `decision`.
fn harbor(dir: &Path, decision: Provision) -> GlobalConfig {
    let mut entry = DomainEntry::file(dir);
    entry.provision = Some(decision);
    config_with("harbor", entry)
}

fn set_decision(global: &mut GlobalConfig, domain: &str, decision: Provision) {
    global.domains.get_mut(domain).unwrap().provision = Some(decision);
}

fn apply_one(
    global: &GlobalConfig,
    sandbox: &Sandbox,
    harness: HarnessKind,
    runner: &mut RecordingRunner,
) -> provision::ApplyReport {
    provision::apply(global, &sandbox.receipt(), &[harness], runner, &no_env()).unwrap()
}

// --- reading what is installed ----------------------------------------------------

/// The artifact keys the receipt records as installed for `harness`, spelled
/// the way a row can be read back: a skill by its folder, a command or agent
/// by its rel without the dialect extension (Codex's flat command names stay
/// flat), an MCP by its name. Every file row must exist on disk.
fn installed(receipt: &Path, harness: HarnessKind) -> BTreeSet<String> {
    let Ok(loaded) = provision::load(receipt) else {
        return BTreeSet::new();
    };
    let Some(state) = loaded.harnesses.get(harness.id()) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    for row in state.files.keys() {
        let (kind, rel) = row.split_once('/').unwrap();
        let (kind_type, _) = parse_artifact_key(&format!("{kind}/x")).unwrap();
        let base = artifact_base(harness, kind_type).unwrap().unwrap();
        let path = rel.split('/').fold(base, |p, c| p.join(c));
        assert!(
            path.is_file(),
            "{harness:?}: the row {row} has no file at {}",
            path.display()
        );
        let name = match kind {
            "skills" => rel.split('/').next().unwrap(),
            _ => rel
                .strip_suffix(".md")
                .or_else(|| rel.strip_suffix(".toml"))
                .unwrap_or(rel),
        };
        out.insert(format!("{kind}/{name}"));
    }
    for name in state.mcps.keys() {
        out.insert(format!("mcps/{name}"));
    }
    out
}

/// The keys of `keys` that `harness` installs, spelled as [`installed`] reads
/// them: a file kind the harness has no surface for is left out (MCP servers
/// always reach the runner), and Codex flattens a nested command.
fn expected(harness: HarnessKind, keys: &[&str]) -> BTreeSet<String> {
    keys.iter()
        .filter_map(|key| {
            let (kind, name) = parse_artifact_key(key).unwrap();
            if kind != ArtifactType::Mcps && !harness_supports(harness, kind) {
                return None;
            }
            if kind == ArtifactType::Commands && harness == HarnessKind::Codex {
                return Some(format!("commands/{}", name.replace('/', "-")));
            }
            Some(key.to_string())
        })
        .collect()
}

/// Where `harness` keeps the installed skill `name`.
fn skill_dir(harness: HarnessKind, name: &str) -> PathBuf {
    artifact_base(harness, ArtifactType::Skills)
        .unwrap()
        .unwrap()
        .join(name)
}

fn newly_optional(report: &provision::ApplyReport) -> Vec<&String> {
    report
        .notices
        .iter()
        .filter(|n| n.contains("is now optional"))
        .collect()
}

fn receipt_inode(receipt: &Path) -> u64 {
    std::fs::metadata(receipt).unwrap().ino()
}

fn all_but<'a>(left_out: &[&str]) -> Vec<&'a str> {
    FLEET
        .iter()
        .copied()
        .filter(|k| !left_out.contains(k))
        .collect()
}

// --- the filter -------------------------------------------------------------------

#[test]
fn except_leaves_out_the_named_artifacts_of_every_kind() {
    let skipped = [
        "skills/tide-tables",
        "commands/charts/plot-route",
        "agents/quartermaster",
        "mcps/lighthouse",
    ];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let global = harbor(dir.path(), except(&skipped));
        let mut runner = RecordingRunner::new();
        apply_one(&global, &sandbox, harness, &mut runner);

        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&skipped)),
            "{harness:?}"
        );
        assert!(!skill_dir(harness, "tide-tables").exists(), "{harness:?}");
        assert!(skill_dir(harness, "storm-warnings").is_dir(), "{harness:?}");
        assert_eq!(runner.calls, vec!["add:foghorn"], "{harness:?}");
    }
}

#[test]
fn only_installs_just_the_named_artifacts() {
    let picked = [
        "skills/storm-warnings",
        "commands/charts/plot-route",
        "agents/navigator",
        "mcps/lighthouse",
    ];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let global = harbor(dir.path(), only(&picked));
        let mut runner = RecordingRunner::new();
        apply_one(&global, &sandbox, harness, &mut runner);

        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &picked),
            "{harness:?}"
        );
        assert!(!skill_dir(harness, "tide-tables").exists(), "{harness:?}");
        assert_eq!(runner.calls, vec!["add:lighthouse"], "{harness:?}");
    }
}

#[test]
fn a_new_artifact_installs_under_except_and_not_under_only() {
    let new = ["skills/fog-signals", "mcps/bell"];
    for harness in HarnessKind::ALL {
        // Under except, an artifact the domain ships later is installed.
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let global = harbor(dir.path(), except(&["skills/storm-warnings"]));
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        for key in new {
            write_artifact(dir.path(), key, false);
        }
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        let mut want = all_but(&["skills/storm-warnings"]);
        want.extend(new);
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &want),
            "{harness:?} under except"
        );
        drop(sandbox);

        // Under only, it is not.
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let global = harbor(dir.path(), only(&["skills/tide-tables", "mcps/foghorn"]));
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        for key in new {
            write_artifact(dir.path(), key, false);
        }
        let mut runner = RecordingRunner::new();
        apply_one(&global, &sandbox, harness, &mut runner);
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &["skills/tide-tables", "mcps/foghorn"]),
            "{harness:?} under only"
        );
        assert!(!skill_dir(harness, "fog-signals").exists(), "{harness:?}");
        assert!(runner.calls.is_empty(), "{harness:?}: {:?}", runner.calls);
    }
}

#[test]
fn an_optional_artifact_installs_only_when_taken() {
    let optional = [
        "skills/storm-warnings",
        "commands/log-entry",
        "agents/navigator",
        "mcps/foghorn",
    ];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&optional);
        let mut global = harbor(dir.path(), Provision::all());
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&optional)),
            "{harness:?} under plain true"
        );
        assert!(
            !skill_dir(harness, "storm-warnings").exists(),
            "{harness:?}"
        );
        assert!(newly_optional(&report).is_empty(), "{harness:?}");

        set_decision(&mut global, "harbor", also(&optional));
        let mut runner = RecordingRunner::new();
        apply_one(&global, &sandbox, harness, &mut runner);
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, FLEET),
            "{harness:?} with every optional artifact taken"
        );
        assert_eq!(runner.calls, vec!["add:foghorn"], "{harness:?}");
    }
}

#[test]
fn optional_changes_nothing_under_only() {
    let optional = ["skills/storm-warnings", "agents/navigator", "mcps/foghorn"];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&optional);
        let picked = [
            "skills/storm-warnings",
            "skills/tide-tables",
            "agents/navigator",
            "mcps/foghorn",
        ];
        let global = harbor(dir.path(), only(&picked));
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &picked),
            "{harness:?}: a named optional artifact installs like a required one"
        );
    }
}

// --- retirement -------------------------------------------------------------------

#[test]
fn skipping_an_installed_artifact_removes_it_and_keeps_an_edit_as_bak() {
    let skipped = [
        "skills/tide-tables",
        "commands/log-entry",
        "agents/navigator",
    ];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), Provision::all());
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, FLEET),
            "{harness:?}"
        );
        // Every file the skipped keys installed, before the skip.
        let receipt = provision::load(&sandbox.receipt()).unwrap();
        let skipped_paths: Vec<PathBuf> = receipt.harnesses[harness.id()]
            .files
            .keys()
            .filter(|row| {
                row.starts_with("skills/tide-tables/")
                    || row.contains("log-entry")
                    || row.contains("navigator")
            })
            .map(|row| {
                let (kind, rel) = row.split_once('/').unwrap();
                let (kind, _) = parse_artifact_key(&format!("{kind}/x")).unwrap();
                let base = artifact_base(harness, kind).unwrap().unwrap();
                rel.split('/').fold(base, |p, c| p.join(c))
            })
            .collect();
        assert!(!skipped_paths.is_empty(), "{harness:?}");
        let skill_md = skill_dir(harness, "tide-tables").join("SKILL.md");
        std::fs::write(&skill_md, "my own tides\n").unwrap();

        set_decision(&mut global, "harbor", except(&skipped));
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());

        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&skipped)),
            "{harness:?}"
        );
        for path in &skipped_paths {
            assert!(!path.exists(), "{harness:?}: {} is left", path.display());
        }
        let bak = skill_dir(harness, "tide-tables").join("SKILL.md.bak");
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "my own tides\n",
            "{harness:?}"
        );
        let actions = statuses(&report, harness);
        assert!(
            actions.contains(&ActionStatus::RetiredBackup),
            "{harness:?}: {actions:?}"
        );
        assert!(
            actions.contains(&ActionStatus::Removed),
            "{harness:?}: {actions:?}"
        );
    }
}

#[test]
fn skipping_an_installed_mcp_deregisters_it() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), Provision::all());
        let mut runner = RecordingRunner::new();
        apply_one(&global, &sandbox, harness, &mut runner);
        assert_eq!(
            runner.calls,
            vec!["add:foghorn", "add:lighthouse"],
            "{harness:?}"
        );

        set_decision(&mut global, "harbor", except(&["mcps/foghorn"]));
        let mut runner = RecordingRunner::new();
        let report = apply_one(&global, &sandbox, harness, &mut runner);

        assert_eq!(runner.calls, vec!["remove:foghorn"], "{harness:?}");
        assert_eq!(
            statuses(&report, harness),
            vec![ActionStatus::McpRemoved],
            "{harness:?}"
        );
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&["mcps/foghorn"])),
            "{harness:?}"
        );
    }
}

// --- the newly optional notice -----------------------------------------------------

#[test]
fn a_newly_optional_installed_artifact_is_removed_with_one_notice() {
    let made_optional = ["skills/storm-warnings", "mcps/foghorn"];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let global = harbor(dir.path(), Provision::all());
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert!(skill_dir(harness, "storm-warnings").is_dir(), "{harness:?}");

        for key in made_optional {
            write_artifact(dir.path(), key, true);
        }
        let mut runner = RecordingRunner::new();
        let report = apply_one(&global, &sandbox, harness, &mut runner);

        assert_eq!(
            newly_optional(&report),
            vec![
                "`mcps/foghorn` from the `harbor` domain is now optional, so it is no longer installed - run `crystalline provision take harbor mcps/foghorn` to keep it.",
                "`skills/storm-warnings` from the `harbor` domain is now optional, so it is no longer installed - run `crystalline provision take harbor skills/storm-warnings` to keep it.",
            ],
            "{harness:?}"
        );
        assert_eq!(runner.calls, vec!["remove:foghorn"], "{harness:?}");
        assert!(
            !skill_dir(harness, "storm-warnings").exists(),
            "{harness:?}"
        );
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&made_optional)),
            "{harness:?}"
        );

        let again = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert!(
            newly_optional(&again).is_empty(),
            "{harness:?}: the notice fires once: {:?}",
            again.notices
        );
    }
}

#[test]
fn a_new_optional_artifact_raises_no_newly_optional_notice() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        // A fresh opt-in to a domain that already marks an artifact optional.
        let dir = fleet_domain(&["skills/storm-warnings", "mcps/foghorn"]);
        let global = harbor(dir.path(), Provision::all());
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert!(
            newly_optional(&report).is_empty(),
            "{harness:?}: {:?}",
            report.notices
        );

        // A brand-new artifact that arrives optional.
        write_artifact(dir.path(), "skills/fog-signals", true);
        write_artifact(dir.path(), "mcps/bell", true);
        let mut runner = RecordingRunner::new();
        let report = apply_one(&global, &sandbox, harness, &mut runner);
        assert!(
            newly_optional(&report).is_empty(),
            "{harness:?}: {:?}",
            report.notices
        );
        assert!(runner.calls.is_empty(), "{harness:?}: {:?}", runner.calls);
        assert!(!skill_dir(harness, "fog-signals").exists(), "{harness:?}");
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(
                harness,
                &all_but(&["skills/storm-warnings", "mcps/foghorn"])
            ),
            "{harness:?}"
        );
    }
}

// --- collisions and notices ------------------------------------------------------

#[test]
fn a_skipped_skill_lets_another_domains_skill_win() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let harbor_dir = fleet_domain(&[]);
        let cove_dir = tempfile::tempdir().unwrap();
        write_manifest(cove_dir.path(), "- skills: skills\n");
        write(
            cove_dir.path(),
            "skills/tide-tables/SKILL.md",
            "---\nname: tide-tables\ndescription: The cove's tides.\n---\n\nCove tides.\n",
        );
        let mut global = harbor(harbor_dir.path(), Provision::all());
        let mut cove = DomainEntry::file(cove_dir.path());
        cove.provision = Some(Provision::all());
        global.domains.insert("cove".to_string(), cove);
        let skill_md = skill_dir(harness, "tide-tables").join("SKILL.md");

        // Control: harbor comes first in the config, so it wins the key.
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert!(
            report.notices.iter().any(|n| n.contains("both provision")),
            "{harness:?}: {:?}",
            report.notices
        );
        assert!(
            std::fs::read_to_string(&skill_md)
                .unwrap()
                .contains("Body of tide-tables"),
            "{harness:?}"
        );

        set_decision(&mut global, "harbor", except(&["skills/tide-tables"]));
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert!(
            !report.notices.iter().any(|n| n.contains("both provision")),
            "{harness:?}: {:?}",
            report.notices
        );
        assert!(
            std::fs::read_to_string(&skill_md)
                .unwrap()
                .contains("Cove tides."),
            "{harness:?}"
        );
        let receipt = provision::load(&sandbox.receipt()).unwrap();
        assert_eq!(
            receipt.harnesses[harness.id()].files["skills/tide-tables/SKILL.md"].domain,
            "cove",
            "{harness:?}"
        );
    }
}

#[test]
fn skipping_every_command_silences_the_copilot_commands_notice() {
    let commands = ["commands/charts/plot-route", "commands/log-entry"];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), Provision::all());
        let notice = format!(
            "domain `harbor` ships commands, but {} provides no commands surface",
            harness.display_name()
        );
        let raised =
            |report: &provision::ApplyReport| report.notices.iter().any(|n| n.starts_with(&notice));

        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert_eq!(
            raised(&report),
            !harness_supports(harness, ArtifactType::Commands),
            "{harness:?}: {:?}",
            report.notices
        );

        set_decision(&mut global, "harbor", except(&commands));
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert!(!raised(&report), "{harness:?}: {:?}", report.notices);
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&commands)),
            "{harness:?}"
        );
    }
}

#[test]
fn a_removed_artifact_named_in_except_is_harmless() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let global = harbor(dir.path(), except(&["skills/old-charts", "mcps/gone"]));
        let report = apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, FLEET),
            "{harness:?}"
        );
        assert!(
            !report
                .notices
                .iter()
                .any(|n| n.contains("old-charts") || n.contains("gone")),
            "{harness:?}: {:?}",
            report.notices
        );
    }
}

// --- the session path ---------------------------------------------------------------

#[test]
fn a_hand_edited_except_list_reconciles_at_session_start() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), Provision::all());
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        let before = receipt_inode(&sandbox.receipt());
        let quiet = provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
        assert!(quiet.is_empty(), "{harness:?}: {quiet:?}");
        assert_eq!(receipt_inode(&sandbox.receipt()), before, "{harness:?}");

        // A hand edit of config.yaml: no source file and no stamp moves.
        set_decision(&mut global, "harbor", except(&["skills/tide-tables"]));
        let notices =
            provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
        assert!(!notices.is_empty(), "{harness:?}: the session reconciled");
        assert!(!skill_dir(harness, "tide-tables").exists(), "{harness:?}");
        assert_eq!(
            installed(&sandbox.receipt(), harness),
            expected(harness, &all_but(&["skills/tide-tables"])),
            "{harness:?}"
        );
        let receipt = provision::load(&sandbox.receipt()).unwrap();
        assert_eq!(
            receipt.sources["harbor"].decision.as_deref(),
            Some("except=skills/tide-tables"),
            "{harness:?}"
        );

        let after = receipt_inode(&sandbox.receipt());
        let quiet = provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
        assert!(quiet.is_empty(), "{harness:?}: {quiet:?}");
        assert_eq!(
            receipt_inode(&sandbox.receipt()),
            after,
            "{harness:?}: the next session does no work"
        );
    }
}

#[test]
fn a_skipped_artifact_does_not_rerun_apply_every_session() {
    let skipped = ["skills/tide-tables", "mcps/foghorn"];
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), except(&skipped));
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());

        // The stamps cover the skipped skill's files too, so the stat walk
        // matches them; the MCP drift check leaves the skipped server out.
        let receipt = provision::load(&sandbox.receipt()).unwrap();
        let stamps = &receipt.sources["harbor"].files;
        assert!(
            stamps.contains_key("skills/tide-tables/SKILL.md")
                && stamps.contains_key("skills/tide-tables/scripts/chart.sh"),
            "{harness:?}: {stamps:?}"
        );

        for session in 1..=2 {
            let before = receipt_inode(&sandbox.receipt());
            let notices =
                provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
            assert!(
                notices.is_empty(),
                "{harness:?} session {session}: {notices:?}"
            );
            assert_eq!(
                receipt_inode(&sandbox.receipt()),
                before,
                "{harness:?} session {session}: apply ran"
            );
        }

        // Control: once the pick goes back to plain true, the session works.
        set_decision(&mut global, "harbor", Provision::all());
        let before = receipt_inode(&sandbox.receipt());
        provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
        assert_ne!(receipt_inode(&sandbox.receipt()), before, "{harness:?}");
        assert!(skill_dir(harness, "tide-tables").is_dir(), "{harness:?}");
    }
}

#[test]
fn an_old_receipt_without_a_decision_field_matches_plain_true() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), Provision::all());
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());

        // Write the receipt back the way a 0.36 binary left it: no
        // `decision` and no `optional` under the domain's sources.
        let text = std::fs::read_to_string(sandbox.receipt()).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let sources = json["sources"]["harbor"].as_object_mut().unwrap();
        sources.remove("decision");
        sources.remove("optional");
        assert!(sources.contains_key("files"));
        std::fs::write(
            sandbox.receipt(),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();

        let before = receipt_inode(&sandbox.receipt());
        let notices =
            provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
        assert!(notices.is_empty(), "{harness:?}: {notices:?}");
        assert_eq!(
            receipt_inode(&sandbox.receipt()),
            before,
            "{harness:?}: plain true matches a receipt with no fingerprint"
        );

        // A pick does not match it.
        set_decision(&mut global, "harbor", except(&["skills/tide-tables"]));
        provision::session_notices(&global, &sandbox.receipt(), &[harness], &no_env());
        assert_ne!(receipt_inode(&sandbox.receipt()), before, "{harness:?}");
        assert!(!skill_dir(harness, "tide-tables").exists(), "{harness:?}");
    }
}

// --- status -----------------------------------------------------------------------

#[test]
fn status_drift_and_orphans_use_the_filtered_set() {
    for harness in HarnessKind::ALL {
        let sandbox = Sandbox::new();
        let dir = fleet_domain(&[]);
        let mut global = harbor(dir.path(), Provision::all());
        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());

        // One kept and one skipped skill change upstream.
        write(
            dir.path(),
            "skills/storm-warnings/SKILL.md",
            "---\nname: storm-warnings\ndescription: Newer.\n---\n\nNewer storms.\n",
        );
        write(
            dir.path(),
            "skills/tide-tables/SKILL.md",
            "---\nname: tide-tables\ndescription: Newer.\n---\n\nNewer tides.\n",
        );
        let counts = |global: &GlobalConfig| {
            let report =
                provision::status(global, &sandbox.receipt(), &[harness], &no_env()).unwrap();
            let h = &report.harnesses[0];
            (h.drift, h.orphaned)
        };

        // Control: under plain true both changed skills drift.
        assert_eq!(counts(&global), (2, 0), "{harness:?} under plain true");

        // Skipping tide-tables (two files) and foghorn orphans their three
        // rows; the changed tide-tables is no longer drift.
        set_decision(
            &mut global,
            "harbor",
            except(&["skills/tide-tables", "mcps/foghorn"]),
        );
        assert_eq!(counts(&global), (1, 3), "{harness:?} under the pick");

        apply_one(&global, &sandbox, harness, &mut RecordingRunner::new());
        assert_eq!(counts(&global), (0, 0), "{harness:?} after apply");
    }
}
