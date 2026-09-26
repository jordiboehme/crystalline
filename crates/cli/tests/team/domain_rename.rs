//! End to end for `crystalline domain rename`, standalone (no daemon in the
//! picture: every command below runs with `--config`, which
//! `crystalline_service::client::use_daemon` always treats as an override
//! that bypasses a running one), and the `domain list` columns a rename's
//! aftermath reads from: NAME as `local (canonical)` when they differ, the
//! ALIASES column, and the `shadowed` marker.

use assert_cmd::Command;

use crate::common::isolate;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// An isolated home with its own config file; nothing is registered until a
/// test calls [`Fixture::register`].
struct Fixture {
    home: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.yaml");
        Fixture { home, config }
    }

    fn config_str(&self) -> &str {
        self.config.to_str().unwrap()
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = bin();
        isolate(&mut cmd, self.home.path());
        cmd.args(args);
        cmd.output().unwrap()
    }

    /// Run a command that must succeed, returning stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Scaffold a MANIFEST declaring `canonical` and register the folder
    /// under the local name `local`, explicitly (so the two may differ).
    fn register(&self, dir: &std::path::Path, local: &str, canonical: &str) {
        self.ok(&["domain", "init", dir.to_str().unwrap(), "--name", canonical]);
        self.ok(&[
            "domain",
            "add",
            local,
            dir.to_str().unwrap(),
            "--config",
            self.config_str(),
        ]);
    }

    /// Append `aliases:` under `local`'s entry in the config file, the way an
    /// earlier rename would leave it, without going through a rename to get
    /// there.
    fn add_alias(&self, local: &str, alias: &str) {
        let text = std::fs::read_to_string(&self.config).unwrap();
        let needle = format!("  {local}:\n");
        assert!(text.contains(&needle), "{text}");
        let replacement = format!("  {local}:\n    aliases:\n      - {alias}\n");
        std::fs::write(&self.config, text.replacen(&needle, &replacement, 1)).unwrap();
    }
}

#[test]
fn domain_rename_moves_the_manifest_and_prints_the_new_name() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("eng");
    fx.register(&dir, "eng", "eng");

    let out = fx.run(&[
        "domain",
        "rename",
        "eng",
        "platform",
        "--config",
        fx.config_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Renamed 'eng' to 'platform'."), "{stdout}");
    assert!(stdout.contains("Former names still work: eng"), "{stdout}");

    let manifest = std::fs::read_to_string(dir.join("MANIFEST.md")).unwrap();
    assert!(
        manifest.contains("domain_name: platform"),
        "the MANIFEST carries the new name: {manifest}"
    );

    // The old local name is gone from the registry; the new one answers, and
    // its index row moved with it rather than starting over: `engrams` is a
    // number, not null (`domain add` already indexed the MANIFEST engram
    // before the rename, so a null here would mean the index row did not
    // follow the rename to its new name).
    let listed = fx.ok(&["--json", "domain", "list", "--config", fx.config_str()]);
    let listed: serde_json::Value = serde_json::from_str(listed.trim()).unwrap();
    let names: Vec<&str> = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["platform"]);
    assert!(
        listed["domains"][0]["engrams"].is_number(),
        "the index followed the rename: {listed}"
    );
}

/// Renaming onto a name another domain already holds refuses instead of
/// clobbering it, with the engine's own words on stderr and a non-zero exit.
#[test]
fn domain_rename_onto_an_existing_domain_refuses() {
    let fx = Fixture::new();
    let eng_dir = fx.home.path().join("eng");
    let ops_dir = fx.home.path().join("ops");
    fx.register(&eng_dir, "eng", "eng");
    fx.register(&ops_dir, "ops", "ops");

    let out = fx.run(&[
        "domain",
        "rename",
        "eng",
        "ops",
        "--config",
        fx.config_str(),
    ]);
    assert!(
        !out.status.success(),
        "renaming onto a live domain name must refuse"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("'ops' is already a domain here"),
        "the engine's own refusal reaches stderr: {stderr}"
    );

    // Neither domain moved: both are still registered under their own names.
    let listed = fx.ok(&["--json", "domain", "list", "--config", fx.config_str()]);
    let listed: serde_json::Value = serde_json::from_str(listed.trim()).unwrap();
    let mut names: Vec<&str> = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, vec!["eng", "ops"]);
}

#[test]
fn domain_rename_reports_the_engine_shape_over_json() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("eng");
    fx.register(&dir, "eng", "eng");

    let out = fx.ok(&[
        "--json",
        "domain",
        "rename",
        "eng",
        "platform",
        "--config",
        fx.config_str(),
    ]);
    let report: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(report["domain"], "platform", "{report}");
    assert_eq!(report["previous"], "eng", "{report}");
    assert_eq!(report["local_only"], false, "{report}");
    assert_eq!(report["manifest_written"], true, "{report}");
    assert!(
        report["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "eng"),
        "{report}"
    );
}

#[test]
fn domain_rename_local_leaves_the_manifest_and_links_untouched() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("eng");
    fx.register(&dir, "eng", "eng");
    let before = std::fs::read_to_string(dir.join("MANIFEST.md")).unwrap();

    let out = fx.run(&[
        "domain",
        "rename",
        "eng",
        "platform",
        "--local",
        "--config",
        fx.config_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "Renamed 'eng' to 'platform' on this machine only; its MANIFEST and links are unchanged.",
        "{stdout}"
    );

    let after = std::fs::read_to_string(dir.join("MANIFEST.md")).unwrap();
    assert_eq!(before, after, "a local rename must not touch the MANIFEST");
}

/// The daemon's own ctl handler resolves a canonical name or an alias to the
/// local name before it ever reaches `Engine::rename_domain`
/// (`control::localized_request`). The standalone path has no such pre-pass
/// of its own, but `Engine::rename_domain` localizes its `domain` argument
/// against the name table as the very first thing it does, so the same
/// spellings work here too - this is the carry from Task 8 this task closes
/// for the rename entry point specifically.
#[test]
fn domain_rename_standalone_accepts_a_canonical_name_like_the_daemon_path() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("wiki");
    fx.register(&dir, "wiki", "knowledge-base");

    let out = fx.ok(&[
        "--json",
        "domain",
        "rename",
        "knowledge-base",
        "platform",
        "--local",
        "--config",
        fx.config_str(),
    ]);
    let report: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        report["previous"], "wiki",
        "the canonical name resolved to the local registration: {report}"
    );
    assert_eq!(report["domain"], "platform", "{report}");
}

/// The same, for a machine-local alias rather than a canonical name.
#[test]
fn domain_rename_standalone_accepts_an_alias_like_the_daemon_path() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("wiki");
    fx.register(&dir, "wiki", "wiki");
    fx.add_alias("wiki", "old-wiki");

    let out = fx.ok(&[
        "--json",
        "domain",
        "rename",
        "old-wiki",
        "platform",
        "--local",
        "--config",
        fx.config_str(),
    ]);
    let report: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        report["previous"], "wiki",
        "the alias resolved to the local registration: {report}"
    );
    assert_eq!(report["domain"], "platform", "{report}");
}

/// The same standalone resolution, for `domain remove`: `unregister_domain`
/// takes only a local name, so the client's standalone branch must localize
/// `name` itself before calling it - the fix this task carries for that
/// entry point too.
#[test]
fn domain_remove_standalone_accepts_a_canonical_name_like_the_daemon_path() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("wiki");
    fx.register(&dir, "wiki", "knowledge-base");

    let out = fx.ok(&[
        "--json",
        "domain",
        "remove",
        "knowledge-base",
        "--config",
        fx.config_str(),
    ]);
    // `print_domain_remove --json` prints the engine's own report, which does
    // not name the domain; what matters is that the removal did not refuse
    // "no such domain 'knowledge-base'" and the registration is gone.
    let report: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert!(report.is_object(), "{report}");
    let listed = fx.ok(&["--json", "domain", "list", "--config", fx.config_str()]);
    let listed: serde_json::Value = serde_json::from_str(listed.trim()).unwrap();
    assert!(listed["domains"].as_array().unwrap().is_empty(), "{listed}");
}

/// The carry from Task 8 reaches every standalone domain command whose
/// daemon path resolves a canonical name or alias
/// (`control::DOMAIN_REFERENCE_COMMANDS`), not only `domain remove` and
/// `domain rename` themselves: `domain review` is one more of them, and the
/// representative test for the rest of the sweep client.rs's
/// `localize_standalone` now covers (`scaffold_virtual_manifest`,
/// `domain_import`, `domain_export`, `tags_retag`, every `origin_*` verb, and
/// `provision` through the config-only `localize_in_config`).
#[test]
fn domain_review_standalone_accepts_a_canonical_name_like_the_daemon_path() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("wiki");
    fx.register(&dir, "wiki", "knowledge-base");

    let out = fx.ok(&[
        "--json",
        "domain",
        "review",
        "knowledge-base",
        "direct",
        "--config",
        fx.config_str(),
    ]);
    let report: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(
        report["domain"], "wiki",
        "the canonical name resolved to the local registration: {report}"
    );
}

#[test]
fn domain_rename_lists_a_domain_whose_link_was_rewritten() {
    let fx = Fixture::new();
    let eng_dir = fx.home.path().join("eng");
    let ops_dir = fx.home.path().join("ops");
    fx.register(&eng_dir, "eng", "eng");
    fx.register(&ops_dir, "ops", "ops");
    std::fs::write(
        ops_dir.join("note.md"),
        "---\ntype: engram\ntitle: Note\npermalink: note\ntags:\n  - ops\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Note\n\nSee crystalline://eng/manifest for the on-call rota.\n",
    )
    .unwrap();
    fx.ok(&["sync", "--config", fx.config_str()]);

    let out = fx.run(&[
        "domain",
        "rename",
        "eng",
        "platform",
        "--config",
        fx.config_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ops: 1 link in 1 engram"), "{stdout}");

    let note = std::fs::read_to_string(ops_dir.join("note.md")).unwrap();
    assert!(
        note.contains("crystalline://platform/manifest"),
        "the link was respelled to the new name: {note}"
    );
}

#[test]
fn domain_list_shows_the_canonical_name_and_aliases_when_they_differ() {
    let fx = Fixture::new();
    let dir = fx.home.path().join("wiki");
    fx.register(&dir, "wiki", "knowledge-base");
    fx.add_alias("wiki", "old-wiki");

    let listed = fx.ok(&["--json", "domain", "list", "--config", fx.config_str()]);
    let listed: serde_json::Value = serde_json::from_str(listed.trim()).unwrap();
    let row = &listed["domains"][0];
    assert_eq!(row["name"], "wiki", "{row}");
    assert_eq!(row["canonical_name"], "knowledge-base", "{row}");
    assert_eq!(row["aliases"], serde_json::json!(["old-wiki"]), "{row}");
    assert_eq!(row["shadowed"], false, "{row}");

    let human = fx.ok(&["domain", "list", "--config", fx.config_str()]);
    assert!(
        human.contains("wiki (knowledge-base)"),
        "the NAME column shows both when they differ: {human}"
    );
    assert!(human.contains("old-wiki"), "{human}");
}

#[test]
fn domain_list_marks_a_shadowed_canonical_name() {
    let fx = Fixture::new();
    // "a" declares domain_name "b", but "b" is ALSO registered here under its
    // own local name: "a"'s canonical name is shadowed and never wins it.
    let dir_a = fx.home.path().join("a");
    let dir_b = fx.home.path().join("b");
    fx.register(&dir_a, "a", "b");
    fx.register(&dir_b, "b", "b");

    let listed = fx.ok(&["--json", "domain", "list", "--config", fx.config_str()]);
    let listed: serde_json::Value = serde_json::from_str(listed.trim()).unwrap();
    let row_a = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "a")
        .unwrap();
    assert_eq!(row_a["shadowed"], true, "{row_a}");

    let human = fx.ok(&["domain", "list", "--config", fx.config_str()]);
    assert!(human.contains("shadowed"), "{human}");
}
