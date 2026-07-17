//! End-to-end smoke test of the domain and data subcommands against a temp
//! config and a temp index. Search itself is covered by the index crate's
//! Store-API tests; the CLI search command lands in M5 with the data commands.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

/// Register a domain `eng` holding two engrams for the human-render tests:
/// `alpha` (which `depends_on` `beta`, and carries a multi-line body marker) and
/// `beta`, both mentioning the token `zephyrtoken` so a search matches both.
/// Returns the config and db paths the data commands read.
fn seed_two_engrams(work: &Path) -> (PathBuf, PathBuf) {
    let domain_dir = work.join("kb");
    let config = work.join("config.yaml");
    let db = work.join("state/index.db");

    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();
    write(
        &domain_dir,
        "alpha.md",
        "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nAlpha body mentions zephyrtoken.\nfirst-line-marker\nsecond-line-marker\n\n- depends_on [[Beta]]\n",
    );
    write(
        &domain_dir,
        "beta.md",
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBeta body mentions zephyrtoken.\n",
    );
    bin()
        .args(["domain", "add", "eng"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    (config, db)
}

/// The sorted top-level keys of a JSON object, for shape assertions.
fn object_keys(v: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = v
        .as_object()
        .expect("top-level JSON is an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

#[test]
fn read_human_output_prints_url_header_and_verbatim_content() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let out = bin()
        .args(["read", "alpha", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The first thing a human sees is the engram address.
    assert!(
        stdout.contains("crystalline://eng/alpha"),
        "read prints the crystalline:// address header: {stdout}"
    );
    // The body is written verbatim: a real newline between the two markers, not
    // the `\n` escape that a JSON string would carry.
    assert!(
        stdout.contains("first-line-marker\nsecond-line-marker"),
        "read prints the content verbatim with real newlines: {stdout:?}"
    );
    assert!(
        !stdout.contains("\\n"),
        "human read output must not contain escaped newlines: {stdout:?}"
    );
}

#[test]
fn search_human_output_lists_hits_with_footer() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let out = bin()
        .args(["search", "zephyrtoken", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    for needle in [
        "Alpha",
        "Beta",
        "crystalline://eng/alpha",
        "crystalline://eng/beta",
        "showing 2 of 2 (page 1)",
    ] {
        assert!(
            stdout.contains(needle),
            "search human output missing {needle:?}: {stdout}"
        );
    }
}

#[test]
fn recent_human_output_lists_engrams_with_footer() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let out = bin()
        .args(["recent", "--timeframe", "10y", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    for needle in [
        "Alpha",
        "Beta",
        "crystalline://eng/alpha",
        "crystalline://eng/beta",
        "showing",
    ] {
        assert!(
            stdout.contains(needle),
            "recent human output missing {needle:?}: {stdout}"
        );
    }
}

#[test]
fn context_human_output_lists_related_engrams() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let out = bin()
        .args(["context", "crystalline://eng/alpha", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    // A header naming the anchor, then the related engram reached over the
    // `depends_on` relation.
    assert!(
        stdout.contains("crystalline://eng/alpha"),
        "context header names the anchor: {stdout}"
    );
    for needle in ["depends_on", "Beta", "crystalline://eng/beta"] {
        assert!(
            stdout.contains(needle),
            "context human output missing {needle:?}: {stdout}"
        );
    }
}

#[test]
fn write_human_output_confirms_url() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let out = bin()
        .args([
            "write",
            "eng",
            "Zeta",
            "--content",
            "- [fact] a zeta fact #eng",
        ])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("crystalline://eng/zeta"),
        "write confirms the new engram's address: {stdout}"
    );
    assert!(
        stdout.contains("created"),
        "write reports the create action: {stdout}"
    );
}

#[test]
fn json_shapes_unchanged() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let run = |args: &[&str]| -> serde_json::Value {
        let out = bin()
            .arg("--json")
            .args(args)
            .args(["--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .output()
            .unwrap();
        assert!(out.status.success(), "command {args:?} succeeds");
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("command {args:?} emits valid JSON: {e}"))
    };

    let search = run(&["search", "zephyrtoken"]);
    assert_eq!(
        object_keys(&search),
        ["count", "hits", "limit", "mode", "page", "total"],
        "search JSON shape unchanged: {search}"
    );

    let read = run(&["read", "alpha"]);
    assert_eq!(
        object_keys(&read),
        [
            "checksum",
            "content",
            "domain",
            "frontmatter",
            "observations",
            "path",
            "permalink",
            "relations",
            "status",
            "title",
            "type",
            "url",
        ],
        "read JSON shape unchanged: {read}"
    );

    let recent = run(&["recent", "--timeframe", "10y"]);
    assert_eq!(
        object_keys(&recent),
        ["count", "engrams", "timeframe"],
        "recent JSON shape unchanged: {recent}"
    );

    let context = run(&["context", "crystalline://eng/alpha"]);
    assert_eq!(
        object_keys(&context),
        ["anchor", "depth", "edges", "nodes", "timeframe"],
        "context JSON shape unchanged: {context}"
    );

    let written = run(&["write", "eng", "Omega", "--content", "- [fact] omega #eng"]);
    assert_eq!(
        object_keys(&written),
        [
            "action",
            "domain",
            "path",
            "permalink",
            "status",
            "title",
            "type"
        ],
        "write JSON shape unchanged: {written}"
    );
}

#[test]
fn init_add_sync_status_end_to_end() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb");
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    // domain init scaffolds a MANIFEST.md.
    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();
    assert!(
        domain_dir.join("MANIFEST.md").exists(),
        "manifest scaffolded"
    );

    // Add an engram to index alongside the manifest.
    write(
        &domain_dir,
        "alpha.md",
        "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nAlpha body with a searchable token.\n",
    );

    // domain add refuses without a manifest, then registers the domain.
    let no_manifest = work.path().join("empty");
    std::fs::create_dir_all(&no_manifest).unwrap();
    bin()
        .args(["domain", "add", "bad"])
        .arg(&no_manifest)
        .args(["--config"])
        .arg(&config)
        .assert()
        .failure();

    // domain add registers the domain and indexes its existing files immediately.
    let out = bin()
        .args(["--json", "domain", "add", "eng"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(config.exists(), "config written");
    let add_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        add_report["sync"]["added"],
        serde_json::json!(2),
        "manifest and engram indexed on add: {add_report}"
    );

    // A search finds the engram right away, with no explicit sync in between.
    let out = bin()
        .args(["--json", "search", "searchable token", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        search["total"].as_u64().unwrap() >= 1,
        "search finds the engram indexed on add: {search}"
    );

    // An explicit sync afterward is a no-op: both files are already indexed.
    let out = bin()
        .args(["--json", "sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let reports: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reports[0]["unchanged"].as_u64().unwrap(),
        2,
        "domain add already indexed both files: {reports}"
    );

    // status reports the counts and the active fts path.
    let out = bin()
        .args(["--json", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["indexed"], serde_json::json!(true));
    assert_eq!(status["fts_mode"], serde_json::json!("candidate-scan"));
    let engrams = status["domains"][0]["engrams"].as_i64().unwrap();
    assert_eq!(engrams, 2);

    // domain list shows the engram count.
    let out = bin()
        .args(["--json", "domain", "list", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let list: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(list["domains"][0]["name"], serde_json::json!("eng"));
    assert_eq!(list["domains"][0]["engrams"], serde_json::json!(2));

    // reindex --full rebuilds and still reports two engrams.
    let out = bin()
        .args(["--json", "reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let reindex: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reindex["reports"][0]["added"], serde_json::json!(2));

    // domain remove drops it from the config but leaves files.
    bin()
        .args(["domain", "remove", "eng", "--config"])
        .arg(&config)
        .assert()
        .success();
    assert!(domain_dir.join("alpha.md").exists(), "files untouched");
}

#[test]
fn domain_add_indexes_pre_existing_files_without_an_explicit_sync() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb-docs");
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "docs"])
        .assert()
        .success();
    write(
        &domain_dir,
        "beta.md",
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBeta body with a distinct findable marker.\n",
    );

    bin()
        .args(["domain", "add", "docs"])
        .arg(&domain_dir)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "search", "distinct findable marker", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        search["total"].as_u64().unwrap() >= 1,
        "search finds the pre-existing file without a sync command: {search}"
    );
}

#[test]
fn read_only_config_refuses_a_cli_write() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb");
    let db = work.path().join("state/index.db");
    let config = work.path().join("config.yaml");

    // A registered domain with a manifest, and a config that serves read-only.
    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();
    std::fs::write(
        &config,
        format!(
            "domains:\n  eng:\n    path: {}\nservice:\n  read_only: true\n",
            domain_dir.display()
        ),
    )
    .unwrap();

    // A standalone `crystalline write` (no daemon) refuses over the engine
    // guard, with the friendly read-only message on stderr, and writes nothing.
    bin()
        .args(["write", "eng", "Blocked", "--content", "- [fact] nope #eng"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .stderr(predicates::str::contains("read-only"));
    assert!(
        !domain_dir.join("blocked.md").exists(),
        "the refused write left no file"
    );
}

#[test]
fn domain_add_no_sync_registers_without_indexing() {
    let work = tempfile::tempdir().unwrap();
    let domain_dir = work.path().join("kb-later");
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "later"])
        .assert()
        .success();
    write(
        &domain_dir,
        "gamma.md",
        "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nGamma body with an unindexed marker.\n",
    );

    let out = bin()
        .args(["--json", "domain", "add", "later"])
        .arg(&domain_dir)
        .arg("--no-sync")
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let add_report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(add_report["synced"], serde_json::json!(false));

    let out = bin()
        .args(["--json", "search", "unindexed marker", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"].as_u64().unwrap(),
        0,
        "--no-sync leaves the domain unindexed: {search}"
    );
}

#[test]
fn status_human_output_with_no_domains_points_at_domain_add() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("state/index.db");

    // Neither the config nor the index exists yet: a first run against a
    // clean machine before any domain has been registered.
    let out = bin()
        .args(["status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("No domains registered yet. Run: crystalline domain add"),
        "status with nothing registered points at domain add: {stdout}"
    );
    assert!(
        !stdout.contains("Run: crystalline sync"),
        "status with nothing registered must not send a first-time user to sync: {stdout}"
    );
}

#[test]
fn delete_without_force_still_deletes_when_stdin_is_not_a_terminal() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    // assert_cmd spawns the child with a closed (non-terminal) stdin, so the
    // confirmation prompt `delete` would otherwise print is skipped entirely
    // here - a script piping into `delete` must never block on it. This is
    // the load-bearing case: without the terminal check, a naive prompt
    // would read EOF and hang or misbehave under a real pipe.
    bin()
        .args(["delete", "alpha", "eng", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "search", "zephyrtoken", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"].as_u64().unwrap(),
        1,
        "alpha was deleted with no hang and no confirmation block: {search}"
    );
}

#[test]
fn delete_force_deletes_without_prompting() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    bin()
        .args(["delete", "beta", "eng", "--force", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let out = bin()
        .args(["--json", "search", "zephyrtoken", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"].as_u64().unwrap(),
        1,
        "beta was deleted with --force and no prompt: {search}"
    );
}

#[test]
fn domain_add_without_a_path_registers_at_the_default_domains_root() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domains_root = work.path().join("root");
    std::fs::write(
        &config,
        format!("domains_root: {}\n", domains_root.display()),
    )
    .unwrap();

    // `domain add` never auto-scaffolds a MANIFEST.md, even at the default
    // root: it needs the same pre-existing one an explicit path would, via
    // `domain init`.
    let default_path = domains_root.join("eng");
    bin()
        .args(["domain", "init"])
        .arg(&default_path)
        .args(["--name", "eng"])
        .assert()
        .success();

    let db = work.path().join("state/index.db");
    let out = bin()
        .args(["domain", "add", "eng", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let canonical_default = std::fs::canonicalize(&default_path).unwrap();
    assert!(
        stdout.contains(&canonical_default.display().to_string()),
        "domain add without a path prints the resolved default root: {stdout}"
    );

    let saved = std::fs::read_to_string(&config).unwrap();
    assert!(
        saved.contains(&canonical_default.display().to_string()),
        "config persists the domain rooted at the default: {saved}"
    );
}
