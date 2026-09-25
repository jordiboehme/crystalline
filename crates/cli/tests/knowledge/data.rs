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

/// The `crystalline domain export ...` command a refusal printed, as argv, with
/// `<dir>` replaced by a real destination. A remedy a person is handed has to be
/// one they can paste, so these tests run it rather than matching its prefix -
/// the wrong argument form satisfies a prefix match and exits 2 on a terminal.
fn printed_export_command(err: &str, dest: &Path) -> Vec<String> {
    let line = err
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("crystalline domain export"))
        .unwrap_or_else(|| {
            panic!("the refusal prints an export command on a line of its own: {err}")
        });
    line.split_whitespace()
        .skip(1)
        .map(|word| {
            if word == "<dir>" {
                dest.display().to_string()
            } else {
                word.to_string()
            }
        })
        .collect()
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
    // Each hit carries the engram's tags so a JSON consumer learns the vocabulary.
    let hit = &search["hits"][0];
    assert_eq!(
        hit["tags"],
        serde_json::json!(["t"]),
        "hit tags in JSON: {hit}"
    );

    let read = run(&["read", "alpha"]);
    assert_eq!(
        object_keys(&read),
        [
            "checksum",
            "content",
            "domain",
            "frontmatter",
            "links",
            "observations",
            "path",
            "permalink",
            "related",
            "relations",
            "status",
            "title",
            "type",
            "url",
        ],
        "read JSON shape: alpha resolves depends_on Beta so `related` is present, and `links` is always emitted: {read}"
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
fn vocabulary_json_shape_and_human_sections() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    // --json returns the { domain, tags, categories, relation_types, types,
    // statuses } shape with a null domain for the all-domain sweep. Every count
    // list is present unconditionally; only `clusters` and `aliases` come and go.
    let out = bin()
        .args(["--json", "vocabulary", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let vocab: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        object_keys(&vocab),
        [
            "categories",
            "domain",
            "relation_types",
            "statuses",
            "tags",
            "types"
        ],
        "vocabulary JSON shape: {vocab}"
    );
    assert!(
        vocab["domain"].is_null(),
        "an all-domain sweep reports a null domain: {vocab}"
    );
    let tag_names: Vec<&str> = vocab["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        tag_names.contains(&"t"),
        "the seeded frontmatter tag surfaces: {vocab}"
    );
    let rels: Vec<&str> = vocab["relation_types"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert!(
        rels.contains(&"depends_on"),
        "the seeded relation type surfaces: {vocab}"
    );

    // Human output shows every labelled section the envelope carries, scoped to
    // one domain, so the human view never lags the JSON.
    let human = bin()
        .args(["vocabulary", "--domain", "eng", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(human.status.success());
    let stdout = String::from_utf8_lossy(&human.stdout);
    for needle in [
        "Tags:",
        "Categories:",
        "Relation types:",
        "Types:",
        "Statuses:",
        "depends_on",
    ] {
        assert!(
            stdout.contains(needle),
            "vocabulary human output missing {needle:?}: {stdout}"
        );
    }
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

    // reindex --full re-reads and re-upserts both engrams. They come back as
    // updates, not additions: nothing is cleared first any more, so the rows
    // they replace are their own previous rows rather than nothing.
    let out = bin()
        .args(["--json", "reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let reindex: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reindex["reports"][0]["updated"], serde_json::json!(2));
    assert_eq!(reindex["reports"][0]["added"], serde_json::json!(0));
    assert_eq!(reindex["reports"][0]["deleted"], serde_json::json!(0));

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

/// The WAL sidecar path turso writes next to a local db file.
fn wal_path(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_os_string();
    s.push("-wal");
    PathBuf::from(s)
}

/// True when the WAL sidecar is either absent or truncated to 0 bytes - the
/// two shapes `PRAGMA wal_checkpoint(TRUNCATE)` can leave behind.
fn wal_is_truncated(db: &Path) -> bool {
    match std::fs::metadata(wal_path(db)) {
        Ok(meta) => meta.len() == 0,
        Err(_) => true,
    }
}

#[test]
fn incremental_reindex_truncates_the_wal() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    // seed_two_engrams already ran `domain add`, which syncs; the WAL sidecar
    // should reflect that write. A plain `sync` afterward covers change 2:
    // the direct sync path checkpoints too, even with nothing new to index.
    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    assert!(
        wal_is_truncated(&db),
        "sync (direct, non-daemon path) truncates the WAL: {:?}",
        std::fs::metadata(wal_path(&db)).map(|m| m.len())
    );

    // Add a third engram so the incremental reindex below has a real delta to
    // merge, not a no-op scan.
    write(
        &work.path().join("kb"),
        "gamma.md",
        "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nGamma body mentions zephyrtoken too.\n",
    );

    // reindex with NO --full: the incremental path. Before the fix this never
    // calls checkpoint_wal, so a snapshot pipeline shipping index.db alone
    // (sidecars deleted) would ship a stale database missing this delta.
    bin()
        .args(["reindex", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    assert!(
        wal_is_truncated(&db),
        "incremental reindex (no --full) truncates the WAL: {:?}",
        std::fs::metadata(wal_path(&db)).map(|m| m.len())
    );

    // The delta the WAL held is not lost: it was merged into the db file, not
    // discarded, before the truncate.
    let out = bin()
        .args(["--json", "search", "zephyrtoken", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"].as_u64().unwrap(),
        3,
        "the incremental delta survived the WAL truncate: {search}"
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

#[test]
fn split_moves_observations_into_a_new_engram_and_links_the_pair() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    bin()
        .args([
            "write",
            "eng",
            "Coolant Bundle",
            "--content",
            "# Coolant Bundle\n\n\
             - [decision] Run the coolant loop on glycol mix B\n\
             - [fact] The loop needs a 40 minute purge before a mix swap\n\
             - [fact] The purge pump is rated for 12 bar\n\
             - [convention] Log every mix swap in the ship register",
            "--tags",
            "coolant",
        ])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // The line numbers a caller reads off `crystalline read`, which are the
    // ones `--observation` takes.
    let read = bin()
        .args(["read", "coolant-bundle", "--json", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(read.status.success());
    let read: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    let lines: Vec<String> = read["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| {
            let text = o["content"].as_str().unwrap_or_default();
            text.contains("purge") || text.contains("12 bar")
        })
        .map(|o| o["line"].to_string())
        .collect();
    assert_eq!(lines.len(), 2, "two bullets are about the purge: {read}");

    // The flag repeats, one line per occurrence, which is what its help text
    // promises: a second --observation adds to the selection rather than
    // replacing the first.
    let out = bin()
        .args([
            "split",
            "coolant-bundle",
            "eng",
            "Purge Procedure",
            "--observation",
            &lines[0],
            "--observation",
            &lines[1],
            "--json",
            "--config",
        ])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "split failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["new"]["permalink"], "purge-procedure");
    assert_eq!(receipt["source"]["permalink"], "coolant-bundle");
    assert_eq!(
        receipt["moved_observations"], 2,
        "both occurrences of --observation moved a bullet"
    );

    // Both halves, read back through the CLI: the content moved and the pair
    // points both ways.
    let new = bin()
        .args(["read", "purge-procedure", "--json", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let new: serde_json::Value = serde_json::from_slice(&new.stdout).unwrap();
    let new_content = new["content"].as_str().unwrap();
    assert!(new_content.contains("40 minute purge"), "{new_content}");
    assert!(new_content.contains("12 bar"), "{new_content}");
    assert!(
        new_content.contains("- derived_from [[coolant-bundle]]"),
        "{new_content}"
    );

    let source = bin()
        .args(["read", "coolant-bundle", "--json", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let source: serde_json::Value = serde_json::from_slice(&source.stdout).unwrap();
    let source_content = source["content"].as_str().unwrap();
    assert!(
        !source_content.contains("40 minute purge"),
        "{source_content}"
    );
    assert!(
        source_content.contains("- split_into [[purge-procedure]]"),
        "{source_content}"
    );
    assert!(
        source_content.contains("- [decision] Run the coolant loop on glycol mix B"),
        "{source_content}"
    );
}

/// The marker outlives the process that set it, which is the whole reason it is
/// a column rather than an in-memory activity record: the incident was a
/// daemonless `reindex --full` killed partway, and nothing in that process was
/// left to say so.
///
/// A second domain whose folder has gone makes the run fail after it stamped
/// that domain and before its rebuild could commit - the same window a SIGTERM
/// lands in. Afterwards a fresh `status` process reads the stamp off disk and
/// says the rebuild did not finish, while the domain's rows are still all
/// there.
#[test]
fn an_interrupted_full_reindex_leaves_a_marker_a_later_status_reports() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    // A second domain, so the run has something to fail on after a first
    // domain has already finished its rebuild cleanly.
    let second = work.path().join("kb2");
    bin()
        .args(["domain", "init"])
        .arg(&second)
        .args(["--name", "two"])
        .assert()
        .success();
    write(
        &second,
        "gamma.md",
        "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nGamma body mentions zephyrtoken.\n",
    );
    bin()
        .args(["domain", "add", "two"])
        .arg(&second)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // The interruption: the second domain cannot be scanned any more.
    std::fs::remove_dir_all(&second).unwrap();
    let out = bin()
        .args(["reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the interrupted run fails");

    // A new process, reading the stamp the dead one left behind.
    let out = bin()
        .args(["--json", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let two = status["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == serde_json::json!("two"))
        .expect("domain two is in the report");
    assert!(
        two["rebuild_started"].is_string(),
        "the marker survived the process that set it: {status}"
    );
    assert_eq!(
        two["engrams"],
        serde_json::json!(2),
        "and its rows are the complete ones from before the rebuild: {status}"
    );
    let one = status["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == serde_json::json!("eng"))
        .expect("domain eng is in the report");
    assert!(
        one["rebuild_started"].is_null(),
        "the domain that finished its rebuild carries no marker: {status}"
    );

    // The human report says what it means and how to finish it.
    let out = bin()
        .args(["status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let human = String::from_utf8_lossy(&out.stdout);
    assert!(
        human.contains("a full rebuild of 'two' started")
            && human.contains("did not finish")
            && human.contains("Run: crystalline reindex --full"),
        "{human}"
    );

    // Re-running it once the folder is back clears the marker.
    write(
        &second,
        "gamma.md",
        "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nGamma body mentions zephyrtoken.\n",
    );
    write(
        &second,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: two\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# two\n\n## Scope\n\n- two\n\n## When to Use\n\n- two\n",
    );
    bin()
        .args(["reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    let out = bin()
        .args(["--json", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        status["domains"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d["rebuild_started"].is_null()),
        "a completed rebuild clears every marker: {status}"
    );
}

/// The two flags are different operations and say so. `--wipe` destroys the
/// index and rebuilds it from disk, which is visible in the report: every
/// engram comes back as an addition because there was nothing left to update.
/// `--full` re-reads the same files into the rows they already have. Asking for
/// both is refused by the parser rather than silently picking one.
#[test]
fn wipe_rebuilds_from_nothing_and_does_not_combine_with_full() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let out = bin()
        .args(["--json", "reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["wipe"], serde_json::json!(true));
    assert_eq!(
        report["reports"][0]["added"],
        serde_json::json!(3),
        "a wipe leaves nothing to update (two engrams and the MANIFEST): {report}"
    );

    // The search still works afterwards, so the rebuild actually landed.
    let out = bin()
        .args(["--json", "search", "zephyrtoken", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(search["total"], serde_json::json!(2));

    let out = bin()
        .args(["reindex", "--full", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the two flags do not combine");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("cannot be used with"),
        "the parser says why: {err}"
    );
}

/// A wipe destroys the database and rebuilds from the files on disk - and an
/// overlay draft is the one thing on nobody's disk, so the rebuild takes the
/// drafts back from the overlay journal under the state directory instead.
///
/// The journal entry is written by hand here, exactly as the write verbs will:
/// the mirror is a file, and this is what it looks like. The second run is what
/// proves the row really landed rather than the counter being invented - by
/// then the store holds the draft, store rows win, and nothing is restored.
#[test]
fn a_wipe_takes_the_drafts_back_from_the_overlay_journal() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let draft = "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: draft\nrecorded_at: 2026-01-02\n---\n\nGamma is alice's own draft.\n";
    // Under the state directory THIS run resolves, which is not one path per
    // platform: a hand-spelled `<home>/state/crystalline` is the XDG answer,
    // and on Windows the binary reads `APPDATA` instead, so the draft would be
    // mirrored in a folder nothing ever walks and the restore would find an
    // empty journal.
    let mirror = crate::common::isolated_state_dir(&home)
        .join("overlays/eng/alice")
        .join("gamma.md");
    std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
    std::fs::write(&mirror, draft).unwrap();

    let mut cmd = bin();
    crate::common::isolate(&mut cmd, &home);
    let out = cmd
        .args(["--json", "reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["drafts_restored"],
        serde_json::json!(1),
        "the wiped index takes alice's draft back from the journal: {report}"
    );

    let mut cmd = bin();
    crate::common::isolate(&mut cmd, &home);
    let out = cmd
        .args(["--json", "reindex", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["drafts_restored"],
        serde_json::json!(0),
        "the row is in the index now, and a store row is never overwritten by its mirror: {report}"
    );
}

/// The fixture above is planted in a folder the binary has to walk on its own,
/// so where that folder is has to be derived rather than spelled - and derived
/// per platform, because base-directory resolution is one strategy per
/// platform and only one of them reads `XDG_STATE_HOME`. This says so directly,
/// on whatever platform it runs: the state directory stands under the variable
/// that governs here, with the layout that variable's strategy gives it. It is
/// the cheap statement of what the wipe test proves the expensive way - a
/// fixture in the wrong folder is a journal with nothing in it, which reads as
/// a restore that found nothing rather than as a test looking in the wrong
/// place.
#[test]
fn an_isolated_run_keeps_its_state_under_this_platform_s_own_base_directory() {
    // Pure path arithmetic: nothing here is created, read or removed.
    let home = std::env::temp_dir().join("cq-isolated-home");
    let home = home.as_path();
    let state = crate::common::isolated_state_dir(home);

    let governing = crate::common::isolation_env(home)
        .into_iter()
        .find(|(name, _)| *name == crate::common::STATE_HOME_VAR)
        .map(|(_, dir)| dir)
        .expect("the isolation environment sets this platform's state-home variable");
    assert_eq!(
        state.parent(),
        Some(governing.as_path()),
        "the state directory stands in the folder the governing variable names"
    );
    assert_eq!(
        state.file_name().and_then(|n| n.to_str()),
        Some("crystalline"),
        "and carries the application folder under it"
    );

    let expected = if cfg!(windows) {
        // No state directory of its own on Windows: the strategy falls back to
        // the data directory, `APPDATA`.
        home.join("roaming").join("crystalline")
    } else {
        home.join("state").join("crystalline")
    };
    assert_eq!(
        state, expected,
        "which is the layout this platform's strategy resolves"
    );
}

/// A wipe destroys the database and rebuilds from the files on disk, so it is
/// only ever safe when the files are the whole truth. A virtual domain's
/// engrams live nowhere else, and no rebuild can bring them back, so the verb
/// refuses rather than quietly deleting them and exiting 0.
#[test]
fn wipe_refuses_while_a_virtual_domain_holds_the_only_copy() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    bin()
        .args(["domain", "add", "notes", "--virtual", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    bin()
        .args(["write", "notes", "Kept Note"])
        .args(["--content", "virtual body that must survive"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the wipe refuses");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("refusing to wipe")
            && err.contains("notes")
            && err.contains("crystalline reindex --full"),
        "the refusal names the domain and both ways out: {err}"
    );

    // The way out it names is run, not matched: the index opens here, so the
    // export the refusal prints has to work as printed.
    let dest = work.path().join("exported");
    let out = bin()
        .args(printed_export_command(&err, &dest))
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the printed export command runs: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let exported = std::fs::read_to_string(dest.join("kept-note.md"))
        .unwrap_or_else(|e| panic!("the export wrote the engram out: {e}"));
    assert!(
        exported.contains("virtual body that must survive"),
        "and the copy holds the engram the refusal was protecting: {exported}"
    );

    // The engram is still there, which is the whole point.
    let out = bin()
        .args([
            "--json",
            "read",
            "kept-note",
            "--domain",
            "notes",
            "--config",
        ])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success(), "the virtual engram is still readable");
    let read: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap_or_default()
            .contains("virtual body that must survive"),
        "{read}"
    );

    // The non-destructive rebuild the refusal points at works as advertised.
    bin()
        .args(["reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
}

/// The guard has to ask the database, not the config it was handed. A wipe is
/// unscoped, so what is at risk is every virtual domain the *index* holds, and
/// the two sets come apart routinely: a domain dropped from the config keeps
/// its rows, and `--db`/`--config` are the documented way to point one config
/// at another's index. A config-driven guard would wave this through and delete
/// the only copy.
#[test]
fn wipe_refuses_for_a_virtual_domain_the_config_does_not_mention() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    bin()
        .args(["domain", "add", "notes", "--virtual", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    bin()
        .args(["write", "notes", "Kept Note"])
        .args(["--content", "virtual body that must survive"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // A second config over the same index that knows only the file domain -
    // the shape a `--config` override produces, and the shape a config edit
    // leaves behind.
    let narrow = work.path().join("narrow.yaml");
    bin()
        .args(["domain", "add", "eng"])
        .arg(work.path().join("kb"))
        .args(["--config"])
        .arg(&narrow)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    let narrow_text = std::fs::read_to_string(&narrow).unwrap();
    assert!(
        !narrow_text.contains("notes"),
        "the narrow config really does not mention the virtual domain: {narrow_text}"
    );

    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&narrow)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the wipe refuses on what the index holds, not on what this config lists"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("refusing to wipe") && err.contains("notes"),
        "the refusal names the domain the index holds: {err}"
    );

    // This is the database-side guard's own wording, and its index is open, so
    // the export it prints is run rather than matched.
    let dest = work.path().join("exported-narrow");
    let out = bin()
        .args(printed_export_command(&err, &dest))
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the printed export command runs: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read_to_string(dest.join("kept-note.md"))
            .unwrap_or_default()
            .contains("virtual body that must survive"),
        "and the copy holds the engram the refusal was protecting"
    );

    // Still there, read back through the config that knows it.
    let out = bin()
        .args([
            "--json",
            "read",
            "kept-note",
            "--domain",
            "notes",
            "--config",
        ])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let read: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap_or_default()
            .contains("virtual body that must survive"),
        "{read}"
    );
}

/// A Postgres index is a shared database with no exclusive open, so nothing can
/// establish that no other instance is serving from it - and a wipe deletes
/// every instance's rows and releases their host claims. The flag has nothing
/// to offer that backend anyway (its reason for existing is a local database
/// file that will not open), so it refuses before it connects to anything.
#[test]
fn wipe_refuses_on_a_postgres_index() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("pg.yaml");
    std::fs::write(
        &config,
        "database:\n  backend: postgres\n  url: postgres://nobody@127.0.0.1:1/nothing\ndomains: {}\n",
    )
    .unwrap();

    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the wipe refuses on postgres");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("refusing to wipe")
            && err.contains("PostgreSQL")
            && err.contains("crystalline reindex --full"),
        "the refusal names the shared-database reason and the rebuild that works: {err}"
    );
    // It never opened anything: the URL above points at a port nothing listens
    // on, so a connection attempt would have failed with a different message.
    assert!(
        !err.contains("connection") && !err.contains("Connection"),
        "the refusal comes before any connection: {err}"
    );
}

/// `--full` stopped self-healing a damaged database when it moved off the
/// resilient open, which is right for the split - but the sentence a person
/// then gets points at `doctor --fix`, which clears a stale lock and does not
/// repair a database. The one command that does has to be named where they
/// meet the failure, not only in the docs.
#[test]
fn a_full_reindex_on_a_damaged_database_names_the_wipe() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    // Garbage in place of the database, the same corruption the recovery test
    // in the index crate uses.
    std::fs::write(
        &db,
        b"this is not a sqlite database, just garbage bytes \x00\x01\x02",
    )
    .unwrap();
    for sidecar in ["-wal", "-shm"] {
        let mut p = db.as_os_str().to_os_string();
        p.push(sidecar);
        let _ = std::fs::remove_file(std::path::PathBuf::from(p));
    }

    let out = bin()
        .args(["reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!out.status.success(), "a damaged database stops --full");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("crystalline reindex --wipe"),
        "the failure names the command that repairs it: {err}"
    );

    // And that command does repair it.
    bin()
        .args(["reindex", "--wipe", "--config"])
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
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"],
        serde_json::json!(2),
        "rebuilt from the files"
    );
}

/// Neither guard is virtual-only: the engine compares the checksum on a file
/// domain inside the write lock and on a virtual domain in the store's
/// compare-and-swap. The help used to say otherwise, which is the 0.17.0 field
/// report's finding 8.
#[test]
fn the_checksum_help_does_not_claim_virtual_domains_only() {
    for verb in ["edit", "split"] {
        let out = bin().args([verb, "--help"]).output().unwrap();
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(
            help.contains("whichever storage kind holds it"),
            "{verb}: {help}"
        );
        assert!(!help.contains("virtual-domain edit"), "{verb}: {help}");
    }
}

/// The virtual-domain guard has to hold in the one state `--wipe` exists for:
/// a database file that will not open. The database-side guard cannot fire
/// there - nothing can ask a corrupt file what it holds - so the config's own
/// answer is the only signal left, and it refuses before anything is opened,
/// set aside or recreated.
///
/// The corruption is two bytes in the header's page-size field, after a
/// checkpoint folded the WAL into the file: every payload page is intact and
/// greppable before and after, so what would destroy the recoverable bytes is
/// the discard and not the corruption.
#[test]
fn wipe_refuses_for_a_virtual_domain_when_the_database_will_not_open() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    bin()
        .args(["domain", "add", "notes", "--virtual", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    bin()
        .args(["write", "notes", "Only Copy"])
        .args(["--content", "virtualpayloadtoken that lives nowhere else"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    // Folds the WAL into the database file, so the payload really is in the
    // bytes this test follows.
    bin()
        .args(["reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let needle = b"virtualpayloadtoken";
    let before = std::fs::read(&db).unwrap();
    assert!(
        before.windows(needle.len()).any(|w| w == needle),
        "the virtual engram's body is in the database file to begin with"
    );

    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
        f.seek(SeekFrom::Start(16)).unwrap();
        f.write_all(b"\x0d\x0d").unwrap();
        f.flush().unwrap();
    }
    let out = bin()
        .args(["read", "only-copy", "--domain", "notes", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the database really will not open any more"
    );

    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the wipe refuses even though nothing can read the database: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("refusing to wipe") && err.contains("notes"),
        "the refusal names the domain: {err}"
    );

    // The export cannot succeed here - the database it would read is the
    // damaged one - but it must still be a command that parses, or the sentence
    // is worse than no sentence at all.
    let dest = work.path().join("exported");
    let out = bin()
        .args(printed_export_command(&err, &dest))
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let export_err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !export_err.contains("Usage:") && !export_err.contains("unexpected argument"),
        "the printed export command parses, whatever the damaged file then does to it: {export_err}"
    );

    let after = std::fs::read(&db).unwrap();
    assert!(
        after.windows(needle.len()).any(|w| w == needle),
        "the only copy of the virtual engram is still in the file, untouched"
    );
    assert!(
        !work
            .path()
            .join("state")
            .read_dir()
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("unreadable-")),
        "nothing was set aside either: the refusal comes before the open"
    );
}

/// The marker mechanism is shared by both rebuild verbs, and the two verbs are
/// opposites: a forced rebuild destroys nothing, a wipe destroys everything
/// before it starts. So the marker records which one was running, and status and
/// doctor say what that means - after an interrupted wipe the rows and the
/// embeddings really are gone, and telling a person "nothing was destroyed" is
/// the exact misreading the marker exists to prevent.
#[test]
fn an_interrupted_wipe_says_its_rows_and_embeddings_are_gone() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    let second = work.path().join("kb2");
    bin()
        .args(["domain", "init"])
        .arg(&second)
        .args(["--name", "two"])
        .assert()
        .success();
    write(
        &second,
        "gamma.md",
        "---\ntype: engram\ntitle: Gamma\npermalink: gamma\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nGamma body mentions zephyrtoken.\n",
    );
    bin()
        .args(["domain", "add", "two"])
        .arg(&second)
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // The interruption, in the same window a SIGTERM lands in: the wipe has
    // already destroyed everything and stamped this domain, and its rebuild
    // cannot run.
    std::fs::remove_dir_all(&second).unwrap();
    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the interrupted wipe fails");

    let out = bin()
        .args(["--json", "status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let two = status["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == serde_json::json!("two"))
        .expect("domain two is in the report");
    assert_eq!(
        two["rebuild_kind"],
        serde_json::json!("wipe"),
        "the marker says which kind of rebuild it was: {status}"
    );

    let out = bin()
        .args(["status", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let human = String::from_utf8_lossy(&out.stdout);
    assert!(
        human.contains("a wipe of 'two' started")
            && human.contains("did not finish")
            && human.contains("Run: crystalline reindex --full"),
        "status names the verb that was running: {human}"
    );
    assert!(
        !human.contains("the ones from before it"),
        "and never claims the rows are the ones from before a wipe: {human}"
    );

    let out = bin()
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    let human = String::from_utf8_lossy(&out.stdout);
    assert!(
        human.contains("a wipe started") && human.contains("Run: crystalline reindex --full"),
        "doctor names it too: {human}"
    );
    assert!(
        !human.contains("nothing was destroyed"),
        "and never reassures a person that nothing was destroyed: {human}"
    );
}

/// The exit the refusal prints for a damaged database, followed end to end.
///
/// This is the state the guard exists for, and it is the state in which every
/// other remedy the program knows is unreachable: the export cannot read the
/// file, `reindex --full` fails and points back at `--wipe`, and `domain remove`
/// needs the index open too. So the refusal names the config file, and what it
/// names has to work - copy the database somewhere safe, delete the virtual
/// domain's entry by hand, wipe. The engrams are gone at that point by the
/// person's own decision, and the bytes are still on disk twice: in their copy
/// and in the aside file the wipe leaves behind.
#[test]
fn the_exit_a_damaged_index_refusal_prints_runs_end_to_end() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = seed_two_engrams(work.path());

    bin()
        .args(["domain", "add", "notes", "--virtual", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    bin()
        .args(["write", "notes", "Only Copy"])
        .args(["--content", "virtualpayloadtoken that lives nowhere else"])
        .args(["--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    bin()
        .args(["reindex", "--full", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
        f.seek(SeekFrom::Start(16)).unwrap();
        f.write_all(b"\x0d\x0d").unwrap();
        f.flush().unwrap();
    }

    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!out.status.success(), "the wipe refuses");
    let err = String::from_utf8_lossy(&out.stderr);

    // Step 0: the refusal names the two files by their real paths, so a person
    // acting on it is not left resolving a default path themselves.
    assert!(
        err.contains(&db.display().to_string()),
        "the refusal names the index file: {err}"
    );
    assert!(
        err.contains(&config.display().to_string()),
        "and the config file the next step edits: {err}"
    );

    // Step 1: take a copy, which is what keeps the engrams recoverable.
    let safe = work.path().join("index.db.rescued");
    std::fs::copy(&db, &safe).unwrap();
    let needle = b"virtualpayloadtoken";
    assert!(
        std::fs::read(&safe)
            .unwrap()
            .windows(needle.len())
            .any(|w| w == needle),
        "the copy holds the engrams the damaged file still carries"
    );

    // Step 2: delete the virtual domain's entry by hand, which is the step the
    // refusal calls the one that loses the content.
    let text = std::fs::read_to_string(&config).unwrap();
    let mut kept = String::new();
    let mut dropping = false;
    for line in text.lines() {
        if line.trim_start().starts_with("notes:") && line.starts_with("  ") {
            dropping = true;
            continue;
        }
        if dropping {
            // The entry's own keys are indented under it; anything at or above
            // the entry's level ends it.
            if line.starts_with("    ") {
                continue;
            }
            dropping = false;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    assert!(
        !kept.contains("notes:"),
        "the entry really is gone from the config: {kept}"
    );
    std::fs::write(&config, &kept).unwrap();

    // Step 3: the wipe now runs, sets the unreadable file aside and rebuilds
    // the file domain from disk.
    let out = bin()
        .args(["reindex", "--wipe", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the exit the refusal printed ends in a wipe that runs: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(
        report.contains("set aside at"),
        "and it says where the unreadable file went: {report}"
    );

    let aside: Vec<_> = db
        .parent()
        .unwrap()
        .read_dir()
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.contains("unreadable-") && !name.ends_with("-wal") && !name.ends_with("-shm")
        })
        .collect();
    assert_eq!(aside.len(), 1, "exactly one database was set aside");
    assert!(
        std::fs::read(aside[0].path())
            .unwrap()
            .windows(needle.len())
            .any(|w| w == needle),
        "the aside copy still holds the engrams too: nothing was deleted"
    );

    // The file domain is back, which is what the wipe was for.
    let out = bin()
        .args(["--json", "search", "zephyrtoken", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(out.status.success());
    let search: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        search["total"],
        serde_json::json!(2),
        "the file domain rebuilt from its files: {search}"
    );
}

// --- domain add: adopt, conflict and name rules (issues 97 and 98) -----------

/// A config holding one team domain `eng` at `root`, with the three keys a
/// plain re-add used to drop. Written through the core's own serializer so the
/// path is valid YAML on every platform.
fn team_config(config: &Path, root: &Path) {
    use crystalline_core::config::{DomainEntry, GlobalConfig, OriginConfig, ReviewMode};
    let mut cfg = GlobalConfig::default();
    let mut entry = DomainEntry::file(std::fs::canonicalize(root).unwrap());
    entry.origin = Some(OriginConfig {
        repo: "acme/kb".to_string(),
        path: None,
        branch: Some("trunk".to_string()),
        poll_secs: None,
    });
    entry.provision = Some(true);
    entry.review = Some(ReviewMode::Overlay);
    cfg.domains.insert("eng".to_string(), entry);
    crystalline_core::config::save_yaml(config, &cfg).unwrap();
}

#[test]
fn a_team_domain_re_added_without_origin_keeps_origin_provision_and_review() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("kb");
    bin()
        .args(["domain", "init"])
        .arg(&root)
        .args(["--name", "eng"])
        .assert()
        .success();
    let config = work.path().join("config.yaml");
    team_config(&config, &root);
    let before = std::fs::read_to_string(&config).unwrap();

    let out = bin()
        .args(["--json", "domain", "add", "eng"])
        .arg(&root)
        .args(["--no-sync", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["adopted"], true, "{report}");
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        before,
        "the registration is left exactly as it was"
    );
}

#[test]
fn domain_add_refuses_a_name_registered_at_another_folder() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("kb");
    let other = work.path().join("other");
    for (dir, name) in [(&root, "eng"), (&other, "eng")] {
        bin()
            .args(["domain", "init"])
            .arg(dir)
            .args(["--name", name])
            .assert()
            .success();
    }
    let config = work.path().join("config.yaml");
    team_config(&config, &root);
    let before = std::fs::read_to_string(&config).unwrap();

    let out = bin()
        .args(["domain", "add", "eng"])
        .arg(&other)
        .args(["--no-sync", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already registered at a different folder"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
}

#[test]
fn domain_add_refuses_a_bad_name_with_the_shared_message() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("kb");
    bin()
        .args(["domain", "init"])
        .arg(&root)
        .args(["--name", "kb"])
        .assert()
        .success();
    let config = work.path().join("config.yaml");

    let out = bin()
        .args(["domain", "add", "a b"])
        .arg(&root)
        .args(["--no-sync", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("'a b' cannot name a domain: use letters"),
        "the message the JSON API and add_domain give: {stderr}"
    );

    let out = bin()
        .args(["domain", "add", "CON", "--virtual", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot name a domain"));
}

#[test]
fn a_grandfathered_name_is_re_added_without_error() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("old notes");
    bin()
        .args(["domain", "init"])
        .arg(&root)
        .args(["--name", "old"])
        .assert()
        .success();
    let config = work.path().join("config.yaml");
    let mut cfg = crystalline_core::config::GlobalConfig::default();
    cfg.domains.insert(
        "my notes".to_string(),
        crystalline_core::config::DomainEntry::file(std::fs::canonicalize(&root).unwrap()),
    );
    crystalline_core::config::save_yaml(&config, &cfg).unwrap();

    bin()
        .args(["domain", "add", "my notes"])
        .arg(&root)
        .args(["--no-sync", "--config"])
        .arg(&config)
        .assert()
        .success()
        .stdout(predicates::str::contains("already registered"));
}

#[test]
fn domain_init_refuses_a_bad_name() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path().join("kb");
    let out = bin()
        .args(["domain", "init"])
        .arg(&root)
        .args(["--name", "a/b"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot name a domain"));
    assert!(!root.join("MANIFEST.md").exists(), "nothing scaffolded");
}
