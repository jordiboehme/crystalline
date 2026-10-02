//! Integration tests for `crystalline doctor`.
//!
//! Domain-level checks (orphans, unindexed files, config sanity) only need an
//! explicit `--config`/`--db`, matching the other data-command tests. The
//! stale lock/socket check additionally needs control over the state
//! directory, so that scenario isolates `HOME`/`XDG_*` the same way the
//! service integration tests do.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
// The provisioning doctor tests are unix-gated like the other harness-touching
// suites, so their imports gate with them or Windows sees them as unused.
#[cfg(unix)]
use crystalline_core::provision::sha256_hex;
use serde_json::Value;
#[cfg(unix)]
use serde_json::json;

fn bin() -> Command {
    crate::common::crystalline()
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody for {title} with enough content.\n\nSecond line.\n"
    )
}

/// Point a doctor invocation's `HOME`/`XDG_*`/Windows base-directory
/// variables at a fresh scratch directory so its exit-code assertions never
/// couple to the developer's real machine-wide state (harness configs under
/// `~/.claude` and `~/.codex`, service lock and socket). Returns the
/// directory as a guard: keep it bound in the caller for the duration of the
/// `Command` call, since dropping it removes the directory.
fn shield_ambient_home(cmd: &mut Command) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for (name, value) in crate::common::isolation_env(home.path()) {
        cmd.env(name, value);
    }
    home
}

/// Scaffold and register a fresh domain, returning its root path. `domain add`
/// now indexes on registration, so it needs the same `--db` the test's own
/// later sync/doctor calls use (`work/index.db`), rather than the machine's
/// default state directory.
fn setup_domain(work: &Path, name: &str, config: &Path) -> PathBuf {
    let domain_dir = work.join(format!("kb-{name}"));
    bin()
        .args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", name])
        .assert()
        .success();
    bin()
        .args(["domain", "add", name])
        .arg(&domain_dir)
        .arg("--config")
        .arg(config)
        .arg("--db")
        .arg(work.join("index.db"))
        .assert()
        .success();
    domain_dir
}

#[test]
fn reports_clean_when_nothing_is_wrong() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["domains"][0]["orphans"], serde_json::json!([]));
    assert_eq!(report["domains"][0]["unindexed"], serde_json::json!([]));
    assert_eq!(report["domains"][0]["path_exists"], serde_json::json!(true));
    assert_eq!(
        report["domains"][0]["manifest_present"],
        serde_json::json!(true)
    );
}

#[test]
fn reports_near_duplicate_tag_clusters_without_failing() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    // Two engrams whose tags are a plural apart: `database` and `databases`.
    write(
        &domain_dir,
        "a.md",
        "---\ntype: engram\ntitle: A\npermalink: a\ntags:\n  - database\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody with enough content.\n\nSecond line.\n",
    );
    write(
        &domain_dir,
        "b.md",
        "---\ntype: engram\ntitle: B\npermalink: b\ntags:\n  - databases\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody with enough content.\n\nSecond line.\n",
    );
    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        // Advisory only: near-duplicate tags never fail doctor's exit code.
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let clusters = report["tags"]["clusters"]
        .as_array()
        .expect("tags.clusters is present once an index exists");
    assert_eq!(clusters.len(), 1, "one near-duplicate cluster: {report}");
    let tags: Vec<&str> = clusters[0]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(
        tags.contains(&"database") && tags.contains(&"databases"),
        "the cluster groups the two spellings: {report}"
    );
}

#[test]
fn detects_orphan_and_fix_removes_it() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));
    write(&domain_dir, "b.md", &engram("B", "b"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // Plant the orphan: the file vanishes without a resync.
    std::fs::remove_file(domain_dir.join("b.md")).unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["domains"][0]["orphans"], serde_json::json!(["b.md"]));

    // --fix removes the orphan row and the report shows zero problems.
    let mut fixed_cmd = bin();
    let _home = shield_ambient_home(&mut fixed_cmd);
    let fixed_out = fixed_cmd
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fixed: Value = serde_json::from_slice(&fixed_out).unwrap();
    assert_eq!(fixed["domains"][0]["orphans_removed"], serde_json::json!(1));

    // A clean re-run confirms the row is really gone.
    let mut clean_cmd = bin();
    let _home = shield_ambient_home(&mut clean_cmd);
    let clean = clean_cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let clean: Value = serde_json::from_slice(&clean).unwrap();
    assert_eq!(clean["domains"][0]["orphans"], serde_json::json!([]));
}

#[test]
fn detects_unindexed_files_without_fixing_them() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();

    // A new file lands on disk after the last sync.
    write(&domain_dir, "c.md", &engram("C", "c"));

    let out = bin()
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1) // unindexed files are report-only, never auto-fixed.
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["unindexed"],
        serde_json::json!(["c.md"])
    );
}

/// Reproduces a colleague's real-world report: a file whose frontmatter
/// does not parse never becomes indexed no matter how many times `sync`
/// runs, so `doctor` must tell it apart from a file that is merely unsynced.
/// Covers a nested path, since `verify` reports an absolute path and the
/// unindexed set holds forward-slashed paths relative to the domain root -
/// the two must be normalised to the same shape before they can be compared.
#[test]
fn tells_an_unsyncable_file_from_an_unsynced_one() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);

    // A well-formed file that simply has not been synced yet.
    write(&domain_dir, "good.md", &engram("Good", "good"));

    // A nested file whose frontmatter has an unbalanced quote: `verify` calls
    // this E001, and no amount of syncing will ever index it.
    write(
        &domain_dir,
        "a/b/bad.md",
        "---\ntype: engram\ntitle: \"Bad\npermalink: bad\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody.\n",
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["unindexed"],
        serde_json::json!(["good.md"]),
        "the broken file must not show up as merely unindexed: {report}"
    );
    let unsyncable = &report["domains"][0]["unsyncable"];
    assert_eq!(unsyncable[0]["path"], serde_json::json!("a/b/bad.md"));
    assert!(
        unsyncable[0]["message"]
            .as_str()
            .unwrap()
            .contains("frontmatter YAML is invalid"),
        "unsyncable message should explain why: {unsyncable}"
    );

    // The human report renders a runnable sync command with no path or colon
    // glued onto it, and a separate block for the unsyncable file.
    let mut human_cmd = bin();
    let _home = shield_ambient_home(&mut human_cmd);
    let human_out = human_cmd
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(human_out).unwrap();
    assert!(
        stdout.contains("run: crystalline sync --domain eng\n"),
        "the suggested command must be pasteable, with nothing glued after the domain name: {stdout}"
    );
    assert!(
        stdout.contains("cannot be indexed until the frontmatter is fixed (verify rule E001)"),
        "unsyncable files get their own explanation: {stdout}"
    );
    assert!(
        stdout.contains("a/b/bad.md: ") && stdout.contains("frontmatter YAML is invalid"),
        "the unsyncable line names the file and the reason: {stdout}"
    );
}

const REPEATED_STATUS: &str = "---\ntype: engram\ntitle: Dup\npermalink: dup\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\nstatus: current\n---\n\nBody for Dup with enough content.\n\nSecond line.\n";

/// A file whose frontmatter holds `status` twice with the same value is its
/// own finding, not a merely unindexed file, and `--fix` keeps one copy.
#[test]
fn reports_and_fixes_a_frontmatter_key_held_twice_with_one_value() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "dup.md", REPEATED_STATUS);

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let domain = &report["domains"][0];
    assert_eq!(
        domain["duplicate_keys"],
        serde_json::json!([{
            "path": "dup.md",
            "keys": [{ "key": "status", "lines": [7, 9], "same_value": true }],
            "fixable": true,
            "fixed": false
        }]),
        "{report}"
    );
    assert!(
        !domain["unindexed"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("dup.md")),
        "a file no sync can index is not merely unindexed: {report}"
    );

    let mut human = bin();
    let _home = shield_ambient_home(&mut human);
    let stdout = String::from_utf8(
        human
            .args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout.contains("[problem] dup.md repeats the frontmatter key `status` on lines 7 and 9 with the same value (verify rule E010), rerun with --fix to keep one copy"),
        "{stdout}"
    );

    let mut fix = bin();
    let _home = shield_ambient_home(&mut fix);
    let fixed_out = String::from_utf8(
        fix.args(["doctor", "--fix", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        fixed_out.contains(
            "fixed dup.md: kept one `status` line, run `crystalline sync --domain eng` to index it"
        ),
        "{fixed_out}"
    );
    assert_eq!(
        std::fs::read_to_string(domain_dir.join("dup.md")).unwrap(),
        REPEATED_STATUS.replacen(
            "recorded_at: 2026-01-01\nstatus: current\n",
            "recorded_at: 2026-01-01\n",
            1
        )
    );
}

/// Copies with different values are a person's call: reported, never written.
#[test]
fn leaves_a_key_held_twice_with_different_values_to_a_person() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    let source = "---\ntype: engram\ntitle: Bad\npermalink: bad\nstatus: a\nstatus: a\ntags: [a]\ntags: [b]\nrecorded_at: 2026-01-01\n---\n\nBody.\n";
    write(&domain_dir, "bad.md", source);

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let stdout = String::from_utf8(
        cmd.args(["doctor", "--fix", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout.contains("[problem] bad.md repeats the frontmatter key `tags` on lines 7 and 8 with different values (verify rule E010): keep the right one and delete the other"),
        "{stdout}"
    );
    assert!(
        stdout.contains("[problem] bad.md repeats the frontmatter key `status` on lines 5 and 6 with the same value (verify rule E010): delete one of the lines"),
        "the agreeing key is named for a person too: {stdout}"
    );
    assert!(
        !stdout.contains("rerun with --fix"),
        "--fix cannot fix this file, so doctor must not send anyone there: {stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(domain_dir.join("bad.md")).unwrap(),
        source,
        "nothing is half fixed"
    );
}

/// Three copies of a key take a plural: "the others", "all but one".
#[test]
fn names_every_extra_copy_when_a_key_appears_three_times() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    let source = "---\ntype: engram\ntitle: Three\npermalink: three\nstatus: a\nstatus: a\nstatus: a\ntags: [a]\ntags: [b]\ntags: [c]\nrecorded_at: 2026-01-01\n---\n\nBody.\n";
    write(&domain_dir, "three.md", source);

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let stdout = String::from_utf8(
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout.contains("[problem] three.md repeats the frontmatter key `tags` on lines 8, 9 and 10 with different values (verify rule E010): keep the right one and delete the others"),
        "{stdout}"
    );
    assert!(
        stdout.contains("[problem] three.md repeats the frontmatter key `status` on lines 5, 6 and 7 with the same value (verify rule E010): delete all but one of the lines"),
        "{stdout}"
    );
}

/// A read-only instance never writes a domain file, so --fix leaves the
/// repeat in place and says why instead of sending anyone to --fix.
#[test]
fn the_fix_on_a_read_only_instance_writes_nothing() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "dup.md", REPEATED_STATUS);
    let mut text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("service:"), "{text}");
    text.push_str("service:\n  read_only: true\n");
    std::fs::write(&config, text).unwrap();

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let stdout = String::from_utf8(
        cmd.args(["doctor", "--fix", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout.contains("[problem] dup.md repeats the frontmatter key `status` on lines 7 and 9 with the same value (verify rule E010). This instance is read-only, so --fix leaves it"),
        "{stdout}"
    );
    assert!(!stdout.contains("rerun with --fix"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(domain_dir.join("dup.md")).unwrap(),
        REPEATED_STATUS
    );
}

/// The plain invocation on a machine with no daemon: nothing to ask, so the
/// index is opened here and every index-backed check runs exactly as it
/// always did. The daemon route must not change what a person sees when there
/// is no daemon, so this pins the direct branch by name.
#[test]
#[cfg(unix)]
fn without_a_daemon_the_index_is_read_directly() {
    let home = tempfile::tempdir().unwrap();
    let domain_dir = home.path().join("kb-eng");
    let mut init = bin();
    crate::common::isolate(&mut init, home.path());
    init.args(["domain", "init"])
        .arg(&domain_dir)
        .args(["--name", "eng"])
        .assert()
        .success();

    // No --config and no --db: the default paths inside the isolated home,
    // which is what makes this the socket-first branch with no socket to find.
    let mut add = bin();
    crate::common::isolate(&mut add, home.path());
    add.args(["domain", "add", "eng"])
        .arg(&domain_dir)
        .assert()
        .success();

    let mut cmd = bin();
    crate::common::isolate(&mut cmd, home.path());
    let out = cmd
        .args(["--json", "doctor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["index"]["source"],
        serde_json::json!("direct"),
        "with no daemon the index is opened here: {report}"
    );
    assert_eq!(
        report["domains"][0]["index_checked"],
        serde_json::json!(true),
        "and the index-backed checks ran: {report}"
    );
    assert!(
        report["embeddings"].is_object(),
        "including the embedding summary, which needs the open store: {report}"
    );
    assert_eq!(
        report["contradictions"]["profile"],
        serde_json::json!("off"),
        "{report}"
    );
}

/// A diagnostic tool that dies when one of its sources is unavailable is no
/// diagnostic tool. With an index nobody can open, every check that does not
/// need it still runs, the report says what stopped the ones that do and what
/// to do about it, and the exit code still reports a problem.
#[test]
fn an_unreadable_index_still_produces_a_report() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "a.md", &engram("A", "a"));

    // Not a database at all. The route does not matter to the report: an
    // unopenable file, a file another process holds and a file this user
    // cannot read all land in the same branch.
    std::fs::write(&db, b"this is not a database\n").unwrap();

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["index"]["source"],
        serde_json::json!("unavailable"),
        "{report}"
    );
    let reason = report["index"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("did not run") && reason.contains("Check that the file is readable"),
        "the reason reads as guidance, not as a bare error: {reason}"
    );
    assert_eq!(
        report["domains"][0]["index_checked"],
        serde_json::json!(false),
        "the index-backed checks are marked as not run: {report}"
    );
    assert_eq!(
        report["domains"][0]["path_exists"],
        serde_json::json!(true),
        "while the checks that need no index still ran: {report}"
    );
    assert!(
        report["service"].is_object(),
        "the service section is one of them: {report}"
    );

    // The human report says the same thing, and never claims the domain is ok.
    let mut human = bin();
    let _home = shield_ambient_home(&mut human);
    let stdout = String::from_utf8(
        human
            .args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout.contains("index:\n  [problem] the index at "),
        "the index section carries the problem: {stdout}"
    );
    assert!(
        stdout.contains("index checks skipped (orphan rows, unindexed files)"),
        "and each domain says which of its checks did not run: {stdout}"
    );
    let domains_block = stdout.split("service:").next().unwrap_or_default();
    assert!(
        !domains_block.contains("\n  ok\n"),
        "a domain whose index checks never ran is never reported as ok: {stdout}"
    );
    assert!(
        stdout.contains("embeddings: not read, the index checks did not run"),
        "the embedding line says why it is empty rather than 'no index yet': {stdout}"
    );
}

#[test]
fn detects_a_registered_domain_whose_path_vanished() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    std::fs::remove_dir_all(&domain_dir).unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["path_exists"],
        serde_json::json!(false)
    );
}

#[test]
fn detects_a_domain_path_that_lost_its_manifest() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    std::fs::remove_file(domain_dir.join("MANIFEST.md")).unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["domains"][0]["path_exists"], serde_json::json!(true));
    assert_eq!(
        report["domains"][0]["manifest_present"],
        serde_json::json!(false)
    );
}

/// A MANIFEST policy value nobody recognizes is named with what it was read
/// as; `--fix` leaves it alone, since a policy is a decision.
#[test]
fn reports_a_manifest_policy_value_nobody_recognizes_and_what_it_reads_as() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(
        &domain_dir,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\nsharing: dirct\n---\n\n# eng\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n",
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["policy_problems"],
        serde_json::json!([{ "key": "sharing", "declared": "dirct", "read_as": "proposal" }]),
        "{report}"
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    cmd.args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .stdout(predicates::str::contains(
            "MANIFEST sharing: dirct is not proposal or direct; read as proposal",
        ));

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    cmd.args(["doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    assert!(
        std::fs::read_to_string(domain_dir.join("MANIFEST.md"))
            .unwrap()
            .contains("sharing: dirct"),
        "--fix does nothing here"
    );
}

/// A valid `domain_name` declaration is never a policy problem: the free-text
/// registry entry accepts it, unlike the pre-3b bug where every declared
/// value failed an empty `values` list.
#[test]
fn a_valid_domain_name_is_not_a_policy_problem() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(
        &domain_dir,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\ndomain_name: platform\n---\n\n# eng\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n",
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["policy_problems"],
        serde_json::json!([]),
        "{report}"
    );
}

/// An invalid `domain_name` is a policy problem naming what a valid one looks
/// like, since the key has no enumerable value list to join.
#[test]
fn an_invalid_domain_name_is_a_policy_problem_naming_a_valid_domain_name() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(
        &domain_dir,
        "MANIFEST.md",
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\ndomain_name: ../up\n---\n\n# eng\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n",
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["domains"][0]["policy_problems"],
        serde_json::json!([{ "key": "domain_name", "declared": "../up", "read_as": "" }]),
        "{report}"
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    cmd.args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .stdout(predicates::str::contains(
            "MANIFEST domain_name: ../up is not a valid domain name; read as ignored; the local name holds",
        ));
}

#[test]
fn domain_filter_restricts_checks_to_one_domain() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    setup_domain(work.path(), "eng", &config);
    setup_domain(work.path(), "product", &config);

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--domain", "eng", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let domains = report["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["name"], serde_json::json!("eng"));
}

/// Stale lock/socket detection needs control over the state directory, which
/// is only reachable through `HOME`/`XDG_*`, not a CLI flag. Isolated with a
/// short-path temp `HOME`, the same technique the service integration tests
/// use for their stale-lock scenario.
#[test]
#[cfg(unix)]
fn detects_and_fixes_a_stale_lock_and_orphaned_socket() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = PathBuf::from("/tmp").join(format!("cq-doctor-{nanos}"));
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(home.join("state")).unwrap();
    std::fs::create_dir_all(home.join("cache")).unwrap();
    let state_dir = home.join("state/crystalline");
    std::fs::create_dir_all(&state_dir).unwrap();

    let apply = |cmd: &mut Command| {
        cmd.env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CACHE_HOME", home.join("cache"));
    };

    // A lock file naming a pid that cannot possibly be alive, plus an
    // orphaned socket file, simulating a `kill -9`'d daemon.
    std::fs::write(
        state_dir.join("service.lock"),
        r#"{"pid":2147483647,"socket_path":"x","version":"0.0.0","started_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    std::fs::write(state_dir.join("service.sock"), b"").unwrap();

    let mut cmd = bin();
    apply(&mut cmd);
    let out = cmd
        .args(["--json", "doctor"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["service"]["lock_stale"], serde_json::json!(true));
    assert_eq!(
        report["service"]["socket_orphaned"],
        serde_json::json!(true)
    );

    let mut fix_cmd = bin();
    apply(&mut fix_cmd);
    let fixed_out = fix_cmd
        .args(["--json", "doctor", "--fix"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fixed: Value = serde_json::from_slice(&fixed_out).unwrap();
    assert_eq!(fixed["service"]["lock_removed"], serde_json::json!(true));
    assert_eq!(fixed["service"]["socket_removed"], serde_json::json!(true));
    assert!(!state_dir.join("service.lock").exists());
    assert!(!state_dir.join("service.sock").exists());

    let _ = std::fs::remove_dir_all(&home);
}

/// A lock genuinely held (by a real separate process, not a fabricated file)
/// with no readable record beside it must never be treated as stale: `--fix`
/// deleting `service.lock` here would let the next opener take a fresh inode
/// while the real holder still has the old one locked, exactly the "two
/// owners" failure mode the 2026-09-23 incident's `doctor --fix` produced.
///
/// `crystalline hold-lock` (the same hidden test entry the wedge tests in
/// `service.rs` use) takes the lock and publishes `service.json` before it
/// announces readiness; this test deletes that record while the child still
/// holds the lock, so what doctor meets is exactly "something holds
/// `service.lock`, nothing answers its socket, and no record says who" - the
/// `holder_unknown` case, not `lock_stale`.
#[test]
#[cfg(unix)]
fn a_held_lock_with_no_record_is_never_treated_as_stale() {
    let (home, state_dir) = isolated_home("held-no-record");
    let apply = |cmd: &mut Command| apply_home(cmd, &home);

    let mut holder = crate::common::crystalline_std();
    holder
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1")
        .args(["hold-lock", "--secs", "60"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = holder.spawn().unwrap();
    // The readiness line means the lock is taken and service.json published;
    // no sleep-and-hope.
    {
        use std::io::BufRead;
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "holding", "hold-lock did not take the lock");
    }

    let info_path = state_dir.join("service.json");
    assert!(
        info_path.is_file(),
        "hold-lock publishes a record before announcing readiness"
    );
    std::fs::remove_file(&info_path).unwrap();
    // A socket file beside the held lock belongs to its holder too.
    let sock_path = state_dir.join("service.sock");
    std::fs::write(&sock_path, b"").unwrap();

    let mut cmd = bin();
    apply(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--fix"])
        .output()
        .unwrap()
        .stdout;
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["service"]["lock_stale"],
        serde_json::json!(false),
        "a lock the OS probe finds held is not stale, record or no record: {report}"
    );
    assert!(
        report["service"]["lock_removed"]
            .as_bool()
            .is_some_and(|b| !b),
        "and --fix never marks it removed: {report}"
    );
    assert!(
        report["service"]["holder_unknown"].is_string(),
        "the held-but-unidentified lock is named instead: {report}"
    );
    let holder_detail = report["service"]["holder_unknown"].as_str().unwrap();
    assert!(
        holder_detail.contains("no service record names the holder"),
        "{holder_detail}"
    );

    assert!(
        state_dir.join("service.lock").exists(),
        "--fix must never delete a service.lock that is actually held"
    );
    assert!(
        sock_path.exists(),
        "nor the socket beside it, which a running holder still serves on: {report}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&home);
}

/// The other half of the record-less lock file: nobody holds it at all (a
/// killed process that never got as far as publishing `service.json`, or a
/// legacy record this reader could not parse), so the OS probe reads free and
/// `lock_stale` must still fire - the fix in
/// [`a_held_lock_with_no_record_is_never_treated_as_stale`] narrows the
/// `lock_stale` verdict, it must not disable it. `lock_pid` is `None` here
/// (no record at all, not even a dead one), so the human report has to say
/// "no record" rather than the old "dead pid None".
#[test]
#[cfg(unix)]
fn a_free_lock_with_no_record_is_reported_by_name_and_fixed() {
    let (home, state_dir) = isolated_home("free-no-record");
    let apply = |cmd: &mut Command| apply_home(cmd, &home);

    // An empty lock file: present, unheld (nothing ever took `flock` on it in
    // this test), and with no `service.json` beside it - the "no record"
    // shape rather than "dead pid".
    std::fs::write(state_dir.join("service.lock"), b"").unwrap();

    let mut cmd = bin();
    apply(&mut cmd);
    let human = String::from_utf8(cmd.arg("doctor").output().unwrap().stdout).unwrap();
    assert!(
        human.contains("stale lock file (no record)"),
        "the free, record-less lock is named \"no record\", not \"dead pid None\": {human}"
    );

    let mut fix_cmd = bin();
    apply(&mut fix_cmd);
    let out = fix_cmd
        .args(["--json", "doctor", "--fix"])
        .output()
        .unwrap()
        .stdout;
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["service"]["lock_stale"], serde_json::json!(true));
    assert_eq!(report["service"]["lock_pid"], serde_json::Value::Null);
    assert_eq!(report["service"]["lock_removed"], serde_json::json!(true));
    assert!(!state_dir.join("service.lock").exists());

    let _ = std::fs::remove_dir_all(&home);
}

/// Origin state (like the service lock/socket above) lives under the state
/// directory, reachable only through `HOME`/`XDG_*`, never a CLI flag, so the
/// tests below isolate a short-path temp `HOME` the same way. Unix-only like
/// every test that calls it: the isolation runs through `/tmp` and `HOME`.
#[cfg(unix)]
fn isolated_home(tag: &str) -> (PathBuf, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = PathBuf::from("/tmp").join(format!("cq-doctor-{tag}-{nanos}"));
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(home.join("state")).unwrap();
    std::fs::create_dir_all(home.join("cache")).unwrap();
    let state_dir = home.join("state/crystalline");
    std::fs::create_dir_all(&state_dir).unwrap();
    (home, state_dir)
}

#[cfg(unix)]
fn apply_home(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        // `crystalline_remote::token`'s test seam: none of the three
        // variables above reach it, since the OS keychain service name is a
        // hardcoded constant rather than derived from any base directory, so
        // without this a `doctor` run whose config turns `github.enabled` on
        // (`write_team_domain_config`, below) asks the real login keychain
        // for a `github` credential the moment `check_github` runs. This is
        // the boolean kill switch, not `CRYSTALLINE_TEST_TOKEN_STORE_DIR`:
        // it falls back to the file store at whatever `origins_state_dir()`
        // already resolves to under the isolated `XDG_STATE_HOME` above,
        // rather than redirecting to a directory of its own.
        .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1")
        // A developer machine's own Copilot home must never leak into the
        // harnesses section's path resolution.
        .env_remove("COPILOT_HOME");
}

/// A team domain's config entry, with a real MANIFEST.md at `domain_dir` so
/// the ordinary per-domain checks report clean and the assertions below stay
/// focused on the github section.
#[cfg(unix)]
fn write_team_domain_config(config: &Path, domain_dir: &Path) {
    std::fs::create_dir_all(domain_dir).unwrap();
    std::fs::write(domain_dir.join("MANIFEST.md"), "# Manifest\n").unwrap();
    std::fs::write(
        config,
        format!(
            "domains:\n  brand:\n    path: {}\n    origin:\n      repo: acme/brand-knowledge\n      branch: main\ngithub:\n  enabled: true\n",
            domain_dir.display()
        ),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn github_section_reports_disconnected_and_an_intact_origin_as_clean() {
    let (home, state_dir) = isolated_home("intact");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    write_team_domain_config(&config, &work.path().join("kb-brand"));

    // A base snapshot matching its recorded stamp exactly (sha256 and size of
    // the literal bytes written below).
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(origin_dir.join("base")).unwrap();
    std::fs::write(origin_dir.join("base/a.md"), "# Team\n\nHello.\n").unwrap();
    std::fs::write(
        origin_dir.join("state.json"),
        r#"{"version":1,"repo":"acme/brand-knowledge","branch":"main","base_commit":"abc123","ref_etag":null,"last_checked":null,"files":{"a.md":{"sha256":"c3c11220a2499569be3fefd408a950e49125ad33d587a26dadbcb210127098fc","size":15}},"proposals":[],"history":[],"conflicts":[]}"#,
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["github"]["connected"], serde_json::json!(false));
    assert_eq!(
        report["github"]["origins"][0]["name"],
        serde_json::json!("brand")
    );
    assert_eq!(
        report["github"]["origins"][0]["repo"],
        serde_json::json!("acme/brand-knowledge")
    );
    assert_eq!(
        report["github"]["origins"][0]["state_present"],
        serde_json::json!(true)
    );
    assert_eq!(
        report["github"]["origins"][0]["base_mismatches"],
        serde_json::json!([])
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn github_section_reports_missing_origin_state_as_a_problem() {
    let (home, _state_dir) = isolated_home("missing-state");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    write_team_domain_config(&config, &work.path().join("kb-brand"));
    // No `origins/brand/state.json` is ever written: the state directory was
    // lost or the domain never fully connected.

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["github"]["origins"][0]["state_present"],
        serde_json::json!(false)
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("no origin state on disk"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

/// In a team domain, the copy the base snapshot has is the one that stays,
/// so the file is byte-equal to the base again and no change is pending.
#[test]
#[cfg(unix)]
fn the_fix_in_a_team_domain_keeps_the_copy_the_base_has() {
    let (home, state_dir) = isolated_home("dup-base");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = work.path().join("kb-brand");
    write_team_domain_config(&config, &domain_dir);

    let base = "---\ntype: manifest\ntitle: Brand\npermalink: manifest\nstatus: stable\ndomain_name: brand\n---\n\n# Brand\n\n## Scope\n\n- s\n\n## When to Use\n\n- w\n";
    let local = base.replacen("title: Brand\n", "title: Brand\ndomain_name: brand\n", 1);
    std::fs::write(domain_dir.join("MANIFEST.md"), &local).unwrap();
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(origin_dir.join("base")).unwrap();
    std::fs::write(origin_dir.join("base/MANIFEST.md"), base).unwrap();
    std::fs::write(
        origin_dir.join("state.json"),
        format!(
            r#"{{"version":1,"repo":"acme/brand-knowledge","branch":"main","base_commit":"abc123","ref_etag":null,"last_checked":null,"files":{{"MANIFEST.md":{{"sha256":"{}","size":{}}}}},"proposals":[],"history":[],"conflicts":[]}}"#,
            sha256_hex(base.as_bytes()),
            base.len()
        ),
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let _ = cmd
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert();
    assert_eq!(
        std::fs::read_to_string(domain_dir.join("MANIFEST.md")).unwrap(),
        base,
        "keeping the first copy would leave a one-line change pending"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// A domain that reviews changes only moves through a merge: doctor restores
/// the base when it can and otherwise leaves the file and says why.
#[test]
#[cfg(unix)]
fn the_fix_in_a_reviewing_domain_only_restores_the_base() {
    let (home, state_dir) = isolated_home("dup-review");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = work.path().join("kb-brand");
    write_team_domain_config(&config, &domain_dir);
    let reviewing = std::fs::read_to_string(&config).unwrap().replacen(
        "    origin:\n",
        "    review: overlay\n    origin:\n",
        1,
    );
    std::fs::write(&config, reviewing).unwrap();

    let base =
        "---\ntype: manifest\ntitle: Brand\npermalink: manifest\nstatus: stable\n---\n\n# Brand\n";
    let local = "---\ntype: manifest\ntitle: Brand\ndomain_name: brand\npermalink: manifest\nstatus: stable\ndomain_name: brand\n---\n\n# Brand\n";
    std::fs::write(domain_dir.join("MANIFEST.md"), local).unwrap();
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(origin_dir.join("base")).unwrap();
    std::fs::write(origin_dir.join("base/MANIFEST.md"), base).unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let stdout = String::from_utf8(
        cmd.args(["doctor", "--fix", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout
            .contains("This domain reviews changes, so --fix leaves it: fix it in the repository"),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(domain_dir.join("MANIFEST.md")).unwrap(),
        local
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// In a domain that reviews changes the fix does write when the result is
/// the base snapshot byte for byte: the folder goes back to what the team
/// reviewed, so nothing new lands outside a merge.
#[test]
#[cfg(unix)]
fn the_fix_in_a_reviewing_domain_writes_when_it_restores_the_base() {
    let (home, state_dir) = isolated_home("dup-review-base");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let domain_dir = work.path().join("kb-brand");
    write_team_domain_config(&config, &domain_dir);
    let reviewing = std::fs::read_to_string(&config).unwrap().replacen(
        "    origin:\n",
        "    review: overlay\n    origin:\n",
        1,
    );
    std::fs::write(&config, reviewing).unwrap();

    let base = "---\ntype: manifest\ntitle: Brand\npermalink: manifest\nstatus: stable\ndomain_name: brand\n---\n\n# Brand\n";
    let local = base.replacen("title: Brand\n", "title: Brand\ndomain_name: brand\n", 1);
    std::fs::write(domain_dir.join("MANIFEST.md"), &local).unwrap();
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(origin_dir.join("base")).unwrap();
    std::fs::write(origin_dir.join("base/MANIFEST.md"), base).unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let stdout = String::from_utf8(
        cmd.args(["doctor", "--fix", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .assert()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(
        stdout.contains("fixed MANIFEST.md: kept one `domain_name` line"),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(domain_dir.join("MANIFEST.md")).unwrap(),
        base
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn github_section_reports_a_base_snapshot_mismatch_as_a_problem() {
    let (home, state_dir) = isolated_home("base-mismatch");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    write_team_domain_config(&config, &work.path().join("kb-brand"));

    // State records `a.md`, but its base snapshot copy was never written (or
    // was lost), so `verify_base` finds it missing.
    let origin_dir = state_dir.join("origins/brand");
    std::fs::create_dir_all(&origin_dir).unwrap();
    std::fs::write(
        origin_dir.join("state.json"),
        r#"{"version":1,"repo":"acme/brand-knowledge","branch":"main","base_commit":"abc123","ref_etag":null,"last_checked":null,"files":{"a.md":{"sha256":"c3c11220a2499569be3fefd408a950e49125ad33d587a26dadbcb210127098fc","size":15}},"proposals":[],"history":[],"conflicts":[]}"#,
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        report["github"]["origins"][0]["state_present"],
        serde_json::json!(true)
    );
    assert_eq!(
        report["github"]["origins"][0]["base_mismatches"],
        serde_json::json!(["a.md"])
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn github_section_reports_the_environment_token_store_when_the_variable_is_set() {
    let (home, _state_dir) = isolated_home("env-token");
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    // No domains and no on-disk `github.enabled`: turning collaboration on
    // and supplying the token both come from the environment alone, proving
    // the overlay drives `check_github` end to end.

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_GITHUB_ENABLED", "true")
        .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["github"]["connected"], serde_json::json!(true));
    assert_eq!(report["github"]["user"], serde_json::Value::Null);
    assert_eq!(
        report["github"]["token_store"],
        serde_json::json!("environment")
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.env("CRYSTALLINE_GITHUB_ENABLED", "true")
            .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
            .args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("connected via CRYSTALLINE_GITHUB_TOKEN (environment token store)"),
        "{human}"
    );
    assert!(!human.contains("SECRET"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

/// A minimal config plus an env-defined domain (with a real `MANIFEST.md` so
/// the per-domain checks report clean and the assertions below stay focused
/// on the environment section), used by the three tests below.
fn setup_env_domain(work: &Path) -> (PathBuf, PathBuf) {
    let config = work.join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();
    let domain_dir = work.join("kb-team");
    std::fs::create_dir_all(&domain_dir).unwrap();
    std::fs::write(domain_dir.join("MANIFEST.md"), "# Manifest\n").unwrap();
    (config, domain_dir)
}

#[test]
fn environment_section_reports_masked_and_filtered_overrides_domains_and_token() {
    let work = tempfile::tempdir().unwrap();
    let (config, domain_dir) = setup_env_domain(work.path());
    let env_config_path = work.path().join("elsewhere-config.yaml");

    let out = bin()
        .env("CRYSTALLINE_SERVICE_READ_ONLY", "true")
        .env("CRYSTALLINE_DOMAIN_TEAM", &domain_dir)
        .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
        .env("CRYSTALLINE_CONFIG", &env_config_path)
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .output()
        .unwrap()
        .stdout;
    let report: Value = serde_json::from_slice(&out).unwrap();
    let env = &report["environment"];

    // The `--config` flag wins for what actually loads, but the overlay still
    // records the variable itself, independent of which path won.
    assert_eq!(
        env["config_path_var"],
        serde_json::json!(env_config_path.display().to_string())
    );

    let overrides = env["overrides"].as_array().unwrap();
    assert!(
        overrides
            .iter()
            .any(|o| o["var"] == "CRYSTALLINE_SERVICE_READ_ONLY"
                && o["key"] == "service.read_only"
                && o["value"] == "true"),
        "{overrides:?}"
    );
    assert!(
        !overrides
            .iter()
            .any(|o| o["key"].as_str().unwrap_or_default().starts_with("domain.")),
        "domain rows belong in the dedicated `domains` list, not the flat overrides: {overrides:?}"
    );
    assert!(
        !overrides.iter().any(|o| o["key"] == "github.token"),
        "the token belongs in the dedicated `github_token` flag, not the flat overrides: {overrides:?}"
    );

    let domains = env["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(
        domains[0]["var"],
        serde_json::json!("CRYSTALLINE_DOMAIN_TEAM")
    );
    assert_eq!(domains[0]["name"], serde_json::json!("team"));
    assert_eq!(
        domains[0]["path"],
        serde_json::json!(domain_dir.display().to_string())
    );
    assert_eq!(domains[0]["origin"], serde_json::Value::Null);

    assert_eq!(env["github_token"], serde_json::json!(true));
}

#[test]
fn environment_section_renders_for_a_human_without_leaking_the_token() {
    let work = tempfile::tempdir().unwrap();
    let (config, domain_dir) = setup_env_domain(work.path());

    let human = bin()
        .env("CRYSTALLINE_SERVICE_READ_ONLY", "true")
        .env("CRYSTALLINE_DOMAIN_TEAM", &domain_dir)
        .env("CRYSTALLINE_GITHUB_TOKEN", "gho_SECRETSECRET")
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .output()
        .unwrap()
        .stdout;
    let human = String::from_utf8(human).unwrap();

    assert!(human.contains("environment:"), "{human}");
    assert!(
        human.contains("CRYSTALLINE_SERVICE_READ_ONLY overrides service.read_only = true"),
        "{human}"
    );
    assert!(
        human.contains(&format!(
            "CRYSTALLINE_DOMAIN_TEAM defines domain 'team' at {}",
            domain_dir.display()
        )),
        "{human}"
    );
    assert!(
        human.contains("CRYSTALLINE_GITHUB_TOKEN provides the GitHub token (read-only)"),
        "{human}"
    );
    assert!(!human.contains("SECRET"), "{human}");
}

#[test]
fn environment_section_is_absent_with_no_env_vars_active() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();

    let out = bin()
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .output()
        .unwrap()
        .stdout;
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["environment"], serde_json::Value::Null);
}

/// The `harnesses` section reads `~/.claude` and `~/.codex`/`~/.agents`,
/// reachable only through `HOME`, never a CLI flag, so every scenario below
/// isolates a fresh `HOME` the same way the lock/socket and github tests
/// above do. A domain-less config plus a fresh `--db` path keeps every
/// assertion focused on the harnesses section alone.
#[cfg(unix)]
fn empty_config(work: &Path) -> (PathBuf, PathBuf) {
    let config = work.join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();
    (config, work.join("index.db"))
}

/// Find one harness's entry in a `--json doctor` report's `harnesses` array
/// by its `name` (`"claude-code"` or `"codex"`).
#[cfg(unix)]
fn harness_entry<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["harnesses"]
        .as_array()
        .unwrap_or_else(|| panic!("harnesses section is absent: {report}"))
        .iter()
        .find(|h| h["name"] == name)
        .unwrap_or_else(|| panic!("no {name} entry in harnesses: {report}"))
}

/// A bin folder holding only a `crystalline` symlink to the binary under
/// test, so the absolute spelling Cursor and Kiro get is deterministic.
#[cfg(unix)]
fn shim_bin(home: &Path) -> PathBuf {
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let link = bin.join("crystalline");
    if !link.exists() {
        std::os::unix::fs::symlink(assert_cmd::cargo::cargo_bin("crystalline"), &link).unwrap();
    }
    bin
}

#[cfg(unix)]
fn run_in(home: &Path, args: &[&str]) -> Vec<u8> {
    let mut cmd = bin();
    apply_home(&mut cmd, home);
    cmd.env_remove("COPILOT_HOME").env("PATH", shim_bin(home));
    cmd.args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone()
}

#[cfg(unix)]
fn doctor_json(home: &Path, work: &Path) -> Value {
    let (config, db) = empty_config(work);
    let out = run_in(
        home,
        &[
            "--json",
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--db",
            db.to_str().unwrap(),
        ],
    );
    serde_json::from_slice(&out).unwrap()
}

#[cfg(unix)]
fn doctor_human(home: &Path, work: &Path) -> String {
    let (config, db) = empty_config(work);
    let out = run_in(
        home,
        &[
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--db",
            db.to_str().unwrap(),
        ],
    );
    String::from_utf8(out).unwrap()
}

/// Decision 16.
#[test]
#[cfg(unix)]
fn a_machine_without_the_new_harnesses_shows_the_same_three_entries() {
    let (home, _) = isolated_home("harness-legacy-only");
    let work = tempfile::tempdir().unwrap();
    run_in(&home, &["install", "claude-code", "--skip-mcp"]);
    let report = doctor_json(&home, work.path());
    let names: Vec<&str> = report["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["claude-code", "codex", "copilot"]);
    assert!(
        harness_entry(&report, "claude-code")
            .get("mcp_entry")
            .is_none(),
        "legacy JSON unchanged: {report}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn each_new_harness_reports_its_mcp_entry_hook_pointer_and_skills() {
    // Only Kiro owns a pointer file; Gemini and Qwen never get one
    // (install does not edit instruction files).
    for (id, pointer) in [
        ("cursor", false),
        ("kiro", true),
        ("gemini", false),
        ("qwen", false),
    ] {
        let (home, _) = isolated_home(&format!("harness-{id}"));
        let work = tempfile::tempdir().unwrap();
        run_in(&home, &["install", id]);
        let report = doctor_json(&home, work.path());
        let h = harness_entry(&report, id);
        assert_eq!(h["mcp_entry"], "up-to-date", "{id}: {h}");
        assert_eq!(h["session_start_hook"], true, "{id}: {h}");
        assert_eq!(h["stop_hook"], false, "{id}: {h}");
        assert!(h["prompt_hook"].is_null(), "{id}: {h}");
        if pointer {
            assert_eq!(h["pointer_present"], true, "{id}: {h}");
            assert!(
                h["pointer_path"]
                    .as_str()
                    .unwrap()
                    .ends_with("crystalline.md")
            );
        } else {
            assert!(
                h.get("pointer_present").is_none(),
                "{id} has no pointer: {h}"
            );
            assert!(h.get("pointer_path").is_none(), "{id} has no pointer: {h}");
        }
        assert!(
            h["skills_folder"].as_str().unwrap().ends_with("skills"),
            "{id}: {h}"
        );
        let folder = PathBuf::from(h["skills_folder"].as_str().unwrap());
        let shipped = skill_dirs(&folder);
        assert!(shipped >= 4, "{id}: {folder:?}");
        assert_eq!(h["skills_installed"], shipped, "{id}: {h}");
        let human = doctor_human(&home, work.path());
        assert!(
            !human.contains(&format!("partial setup - run: crystalline install {id}")),
            "{id}: {human}"
        );
        assert!(
            !human.contains("UserPromptSubmit hook: not available"),
            "{id}: {human}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[test]
#[cfg(unix)]
fn a_malformed_mcp_json_is_a_problem_doctor_counts() {
    let (home, _) = isolated_home("harness-cursor-broken");
    let work = tempfile::tempdir().unwrap();
    run_in(&home, &["install", "cursor"]);
    std::fs::write(home.join(".cursor/mcp.json"), "{").unwrap();
    let (config, db) = empty_config(work.path());
    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .env("PATH", shim_bin(&home))
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let err = harness_entry(&report, "cursor")["mcp_parse_error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(err.contains("mcp.json") && err.contains("line"), "{err}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn cursor_beside_claude_code_carries_the_dedupe_note_and_the_cover() {
    let (home, _) = isolated_home("harness-cursor-claude");
    let work = tempfile::tempdir().unwrap();
    run_in(&home, &["install", "claude-code", "--skip-mcp"]);
    run_in(&home, &["install", "cursor"]);
    let report = doctor_json(&home, work.path());
    let c = harness_entry(&report, "cursor");
    assert!(
        c["note"].as_str().unwrap().contains("stays silent there"),
        "{c}"
    );
    assert!(
        c["skills_covered_by"]
            .as_str()
            .unwrap()
            .ends_with(".claude/skills"),
        "{c}"
    );
    assert_eq!(
        c["skills_installed"], 0,
        "a covered row writes nothing: {c}"
    );
    let human = doctor_human(&home, work.path());
    assert!(human.contains("covered by"), "{human}");
    let _ = std::fs::remove_dir_all(&home);
}

/// A folder several harnesses write is attributed to the rows that write it,
/// never to a harness that did not install anything there.
#[test]
#[cfg(unix)]
fn skills_cursor_wrote_in_the_shared_folder_are_not_codex_skills() {
    let (home, _) = isolated_home("harness-shared-folder");
    let work = tempfile::tempdir().unwrap();
    run_in(&home, &["install", "cursor"]);
    let report = doctor_json(&home, work.path());
    let shipped = skill_dirs(&home.join(".agents/skills"));
    assert!(shipped >= 4);
    assert_eq!(
        harness_entry(&report, "cursor")["skills_installed"],
        shipped
    );
    assert_eq!(
        harness_entry(&report, "codex")["skills_installed"],
        0,
        "{report}"
    );
    run_in(&home, &["install", "codex", "--skip-mcp"]);
    let report = doctor_json(&home, work.path());
    assert_eq!(harness_entry(&report, "codex")["skills_installed"], shipped);
    assert_eq!(
        harness_entry(&report, "cursor")["skills_installed"],
        shipped
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// Without a hook install writes the MCP entry and the skills only, and
/// doctor names the missing part. Gemini has no pointer either way.
#[test]
#[cfg(unix)]
fn gemini_without_its_hook_is_a_partial_setup_and_no_instruction_file_appears() {
    let (home, _) = isolated_home("harness-gemini-nohook");
    let work = tempfile::tempdir().unwrap();
    run_in(&home, &["install", "gemini", "--skip-hooks"]);
    assert!(!home.join(".gemini/GEMINI.md").exists());
    let human = doctor_human(&home, work.path());
    assert!(
        human.contains("partial setup - run: crystalline install gemini"),
        "{human}"
    );
    let report = doctor_json(&home, work.path());
    let g = harness_entry(&report, "gemini");
    assert_eq!(g["session_start_hook"], false);
    assert_eq!(g["mcp_entry"], "up-to-date");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_reports_both_hooks_present_after_install() {
    let (home, _state_dir) = isolated_home("harness-installed");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    // Seed a real Claude Code settings file the same way a user would: run
    // the installer itself, skipping MCP (no shim on PATH) and skills
    // (irrelevant to this scenario) so only the two hooks land.
    let mut install_cmd = bin();
    apply_home(&mut install_cmd, &home);
    install_cmd
        .args(["install", "claude-code", "--skip-mcp", "--skip-skills"])
        .assert()
        .success();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let claude = harness_entry(&report, "claude-code");
    assert_eq!(claude["settings_present"], serde_json::json!(true));
    assert_eq!(claude["settings_parse_error"], serde_json::Value::Null);
    assert_eq!(claude["session_start_hook"], serde_json::json!(true));
    assert_eq!(claude["stop_hook"], serde_json::json!(true));
    assert_eq!(claude["prompt_hook"], serde_json::json!(true));

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_counts_a_corrupt_settings_file_as_a_problem() {
    let (home, _state_dir) = isolated_home("harness-corrupt");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "{ not valid json").unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let claude = harness_entry(&report, "claude-code");
    assert!(
        claude["settings_parse_error"].is_string(),
        "a corrupt settings file must report a parse error: {report}"
    );
    // A parse error reads as `Some(false)`, exactly like `session_start_hook`
    // and `stop_hook` answer plain `false` - "checked, could not read it",
    // never the `null` reserved for a harness with no prompt-hook channel.
    assert_eq!(claude["session_start_hook"], serde_json::json!(false));
    assert_eq!(claude["stop_hook"], serde_json::json!(false));
    assert_eq!(claude["prompt_hook"], serde_json::json!(false));

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("not valid JSON"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

/// The regression trap for reading the Copilot-owned hooks file with the
/// Claude-shaped presence predicate: Copilot entries are flat and carry the
/// copilot session start command, so only the shape-aware dispatch reports
/// them present.
#[test]
#[cfg(unix)]
fn harnesses_section_reports_copilot_hooks_present_after_install() {
    let (home, _state_dir) = isolated_home("harness-copilot");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let mut install_cmd = bin();
    apply_home(&mut install_cmd, &home);
    install_cmd
        .args(["install", "copilot", "--skip-mcp", "--skip-skills"])
        .assert()
        .success();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let copilot = harness_entry(&report, "copilot");
    assert_eq!(copilot["settings_present"], serde_json::json!(true));
    assert_eq!(copilot["settings_parse_error"], serde_json::Value::Null);
    assert_eq!(copilot["session_start_hook"], serde_json::json!(true));
    assert_eq!(copilot["stop_hook"], serde_json::json!(true));
    // Ruled 2026-09-21: Copilot's prompt hook is written too, in the same
    // shape as Claude Code and Codex - present, not the `null` a harness
    // with no output channel at all would report.
    assert_eq!(copilot["prompt_hook"], serde_json::json!(true));

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_counts_a_corrupt_copilot_file_as_a_problem() {
    let (home, _state_dir) = isolated_home("harness-copilot-corrupt");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let hooks_dir = home.join(".copilot").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    std::fs::write(hooks_dir.join("crystalline.json"), "{ not valid json").unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let copilot = harness_entry(&report, "copilot");
    assert!(
        copilot["settings_parse_error"].is_string(),
        "a corrupt owned hooks file must report a parse error: {report}"
    );
    // Copilot's prompt hook being permanently inert is a separate fact from
    // "could this file be read" - a parse error still reads as `Some(false)`,
    // never `null`.
    assert_eq!(copilot["prompt_hook"], serde_json::json!(false));

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_hints_partial_setup_when_only_one_hook_is_present() {
    let (home, _state_dir) = isolated_home("harness-partial");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    // Only the hand-written SessionStart recipe from docs/learning-loop.md, no Stop
    // hook: exactly the "half installed" shape the hint exists for.
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{ "hooks": { "SessionStart": [ { "matcher": "startup", "hooks": [ { "type": "command", "command": "crystalline prompt system" } ] } ] } }"#,
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let human = cmd
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .output()
        .unwrap()
        .stdout;
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("partial setup - run: crystalline install claude-code"),
        "{human}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
#[cfg(unix)]
fn harnesses_section_is_absent_with_no_trace_on_either_harness() {
    let (home, _state_dir) = isolated_home("harness-none");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["harnesses"], serde_json::Value::Null);

    let _ = std::fs::remove_dir_all(&home);
}

// --- receipt-aware harness diagnostics ---------------------------------------
//
// The scenario below needs a real install receipt to tamper with, which only
// `crystalline install` writes. `receipt_file`, `tamper_receipt` and
// `write_shim` mirror the same-named helpers in `tests/setup/install.rs` - test
// binaries do not share code across files, so they are duplicated here rather
// than factored out.

/// Create an executable shim named `name` in `bin_dir` that appends its
/// arguments to `log` and exits 1 for `mcp get`, 0 otherwise, so the install
/// this scenario seeds proceeds exactly like a real (not-yet-registered)
/// harness CLI would.
#[cfg(unix)]
fn write_shim(bin_dir: &Path, name: &str, log: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = mcp ] && [ \"$2\" = get ]; then\n  exit 1\nfi\nexit 0\n",
        log.display()
    );
    let path = bin_dir.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The receipt path under an isolated home: state_dir honors
/// `XDG_STATE_HOME`, which `apply_home` points at `<home>/state`.
#[cfg(unix)]
fn receipt_file(home: &Path) -> PathBuf {
    home.join("state").join("crystalline").join("installs.json")
}

/// Rewrite the receipt with a mutation applied, for simulating an install
/// last reconciled by a different binary version, or one that still
/// remembers a skill this version no longer ships.
#[cfg(unix)]
fn tamper_receipt(home: &Path, mutate: impl FnOnce(&mut Value)) {
    let path = receipt_file(home);
    let bytes = std::fs::read(&path).unwrap();
    let mut receipt: Value = serde_json::from_slice(&bytes).unwrap();
    mutate(&mut receipt);
    std::fs::write(&path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();
}

/// A version-skewed install whose receipt still remembers a retired skill:
/// `doctor` must surface both the skew and the leftover, in `--json` and in
/// the human rendering, without failing the exit code - both self-heal, the
/// skew at the next session start and the leftover at the next `crystalline
/// install`.
#[test]
#[cfg(unix)]
fn doctor_reports_a_version_skewed_install_and_retired_leftovers() {
    let (home, _state_dir) = isolated_home("skew-leftover");
    let work = tempfile::tempdir().unwrap();
    let bin_dir = work.path().join("bin");
    let log = work.path().join("claude.log");
    write_shim(&bin_dir, "claude", &log);
    let (config, db) = empty_config(work.path());

    let mut install_cmd = bin();
    apply_home(&mut install_cmd, &home);
    install_cmd
        .env("PATH", &bin_dir)
        .args(["install", "claude-code"])
        .assert()
        .success();

    // Tamper the receipt: an older version reconciled this install, and it
    // still remembers a skill folder this binary no longer ships.
    tamper_receipt(&home, |receipt| {
        let entry = &mut receipt["installs"][0];
        entry["version"] = serde_json::json!("0.0.1");
        let skills = entry["skills"].as_array_mut().unwrap();
        skills.push(serde_json::json!({
            "name": "crystalline-legacy",
            "sha256": "0".repeat(64),
        }));
    });
    let legacy_dir = home
        .join(".claude")
        .join("skills")
        .join("crystalline-legacy");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join("SKILL.md"), "legacy body").unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let claude = harness_entry(&report, "claude-code");
    assert_eq!(claude["receipt_version"], serde_json::json!("0.0.1"));
    assert_eq!(
        claude["retired_leftovers"],
        serde_json::json!(["crystalline-legacy"])
    );

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("installed by 0.0.1"), "{human}");
    assert!(human.contains("crystalline-legacy"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

// --- provisioning section ---------------------------------------------------
//
// `doctor`'s provisioning section reads straight off
// `crystalline_core::provision::status` and the on-disk provisioning receipt
// at `<state_dir>/provisions.json`, reachable only through `HOME`/`XDG_*`
// like the harnesses section above, so these scenarios isolate a fresh
// `HOME` the same way.

/// Write a minimal valid MANIFEST at `dir` declaring a `## Provisioning`
/// section from `bullets` (already `- `-prefixed lines, one per artifact
/// type) - the same shape `crystalline_core::provision`'s own tests use.
#[cfg(unix)]
fn write_provisioning_manifest(dir: &Path, title: &str, bullets: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let source = format!(
        "---\ntype: manifest\ntitle: {title}\npermalink: manifest\n---\n\n\
         # {title}\n\n\
         ## Scope\n\n- {title} knowledge\n\n\
         ## When to Use\n\n- When working on {title}\n\n\
         ## Provisioning\n\n{bullets}"
    );
    std::fs::write(dir.join("MANIFEST.md"), source).unwrap();
}

/// The provisioning receipt path under an isolated home: `state_dir` honors
/// `XDG_STATE_HOME`, which `apply_home` points at `<home>/state`.
#[cfg(unix)]
fn provision_receipt_file(home: &Path) -> PathBuf {
    home.join("state")
        .join("crystalline")
        .join("provisions.json")
}

/// Write a claude-code install receipt at the isolated home's state
/// directory, marking it onboarded at user scope - the same shape
/// `tests/setup/provision.rs`'s `write_install_receipt` writes. Doctor's
/// provisioning section is gated to installed harnesses only, the same gate
/// `apply` and `provision status` use.
#[cfg(unix)]
fn write_claude_code_install_receipt(home: &Path) {
    std::fs::write(
        receipt_file(home),
        serde_json::to_string_pretty(&json!({
            "format": 1,
            "installs": [
                {
                    "harness": "claude-code",
                    "scope": "user",
                    "version": "0.0.0",
                    "parts": { "mcp": true, "hooks": true, "skills": true },
                    "skills": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

/// One fixture exercising every new count at once: `harbor` is opted in and
/// was reconciled once already, `cove` still awaits a decision. The
/// hand-crafted provisioning receipt gives claude-code three recorded rows:
/// `SKILL.md` records exactly what harbor ships now, but the installed copy
/// on disk was hand-edited since (edited, not drift); `chart.sh`'s recorded
/// hash is stale against what harbor ships now, with the installed copy left
/// matching that stale record (drift, not edited); `commands/retired.md`
/// names an artifact harbor no longer declares at all (orphaned).
#[test]
#[cfg(unix)]
fn provisioning_section_reports_domain_counts_pending_line_and_harness_drift_edited_orphaned() {
    let (home, _state_dir) = isolated_home("provisioning");
    let work = tempfile::tempdir().unwrap();

    let harbor_dir = work.path().join("kb-harbor");
    write_provisioning_manifest(&harbor_dir, "harbor", "- skills: skills\n");
    let current_skill = "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n";
    write(&harbor_dir, "skills/tide-tables/SKILL.md", current_skill);
    let current_chart = "#!/bin/sh\necho new-chart\n";
    write(
        &harbor_dir,
        "skills/tide-tables/scripts/chart.sh",
        current_chart,
    );

    let cove_dir = work.path().join("kb-cove");
    write_provisioning_manifest(&cove_dir, "cove", "- skills: skills\n");
    write(
        &cove_dir,
        "skills/lookout/SKILL.md",
        "---\nname: lookout\n---\n\nWatches for approaching ships.\n",
    );

    let config = work.path().join("config.yaml");
    std::fs::write(
        &config,
        format!(
            "domains:\n  harbor:\n    path: {}\n    provision: true\n  cove:\n    path: {}\n",
            harbor_dir.display(),
            cove_dir.display()
        ),
    )
    .unwrap();

    write_claude_code_install_receipt(&home);

    let stale_chart = "#!/bin/sh\necho old-chart\n";
    std::fs::write(
        provision_receipt_file(&home),
        serde_json::to_string_pretty(&json!({
            "format": 1,
            "sources": {},
            "harnesses": {
                "claude-code": {
                    "files": {
                        "skills/tide-tables/SKILL.md": {
                            "domain": "harbor",
                            "sha256": sha256_hex(current_skill.as_bytes()),
                        },
                        "skills/tide-tables/scripts/chart.sh": {
                            "domain": "harbor",
                            "sha256": sha256_hex(stale_chart.as_bytes()),
                        },
                        "commands/retired.md": {
                            "domain": "harbor",
                            "sha256": sha256_hex(b"anything"),
                        }
                    },
                    "mcps": {}
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    // The installed copy on disk: `SKILL.md` was hand-edited since the last
    // reconcile, `chart.sh` still matches the stale record exactly, and
    // `retired.md` was never installed at all (missing, incidental to this
    // scenario).
    write(
        &home,
        ".claude/skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nHand edited locally.\n",
    );
    write(
        &home,
        ".claude/skills/tide-tables/scripts/chart.sh",
        stale_chart,
    );

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let provisioning = &report["provisioning"];

    let domains = provisioning["domains"].as_array().unwrap();
    let harbor = domains
        .iter()
        .find(|d| d["name"] == "harbor")
        .unwrap_or_else(|| panic!("no harbor entry: {provisioning}"));
    assert_eq!(harbor["decision"], json!("allowed"));
    assert_eq!(harbor["counts"]["skills"], json!(1));
    assert_eq!(harbor["mirror_present"], Value::Null);

    let cove = domains
        .iter()
        .find(|d| d["name"] == "cove")
        .unwrap_or_else(|| panic!("no cove entry: {provisioning}"));
    assert_eq!(cove["decision"], json!("undecided"));

    let claude = provisioning["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["harness"] == "claude-code")
        .unwrap_or_else(|| panic!("no claude-code entry: {provisioning}"));
    assert_eq!(claude["installed_files"], json!(3));
    assert_eq!(claude["installed_mcps"], json!(0));
    assert_eq!(claude["drift"], json!(1), "{provisioning}");
    assert_eq!(claude["edited"], json!(1), "{provisioning}");
    assert_eq!(claude["orphaned"], json!(1), "{provisioning}");
    // The orphaned `retired.md` row was never installed on disk either, so
    // it doubles as the missing count's fixture.
    assert_eq!(claude["missing"], json!(1), "{provisioning}");

    let pending = provisioning["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{provisioning}");
    assert_eq!(pending[0]["domain"], json!("cove"));

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("provisioning:"), "{human}");
    assert!(human.contains("harbor: allowed"), "{human}");
    assert!(human.contains("cove: undecided"), "{human}");
    assert!(
        human.contains(
            "claude-code: 3 file(s) installed, 0 mcp(s) installed, 1 drifted, 1 edited, 1 orphaned, 1 missing"
        ),
        "{human}"
    );
    assert!(
        human.contains("crystalline provision allow cove"),
        "{human}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// The provisioning section lists domains in the same order `provision
/// status` does: allowed, undecided, denied, each by name, whatever order the
/// config registers them in, and the awaiting list goes by name too.
#[test]
#[cfg(unix)]
fn provisioning_section_lists_domains_and_pending_in_the_status_order() {
    let (home, _state_dir) = isolated_home("provisioning-order");
    let work = tempfile::tempdir().unwrap();

    let mut yaml = String::from("domains:\n");
    for (name, decision) in [
        ("zulu", "provision: false\n"),
        ("mike", ""),
        ("Yankee", "provision: true\n"),
        ("echo", ""),
        ("alpha", "provision: true\n"),
    ] {
        let dir = work.path().join(format!("kb-{name}"));
        write_provisioning_manifest(&dir, name, "- agents: agents\n");
        write(&dir, "agents/scout.md", "# Scout\n");
        yaml.push_str(&format!("  {name}:\n    path: {}\n", dir.display()));
        if !decision.is_empty() {
            yaml.push_str(&format!("    {decision}"));
        }
    }
    let config = work.path().join("config.yaml");
    std::fs::write(&config, yaml).unwrap();

    let run = |json: bool| {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        if json {
            cmd.arg("--json");
        }
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(work.path().join("index.db"))
            .output()
            .unwrap()
            .stdout
    };

    let report: Value = serde_json::from_slice(&run(true)).unwrap();
    let names: Vec<&str> = report["provisioning"]["domains"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "Yankee", "echo", "mike", "zulu"]);
    let pending: Vec<&str> = report["provisioning"]["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["domain"].as_str().unwrap())
        .collect();
    assert_eq!(pending, ["echo", "mike"]);

    let human = String::from_utf8(run(false)).unwrap();
    let at = |needle: &str| {
        human
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} missing: {human}"))
    };
    assert!(at("  alpha: allowed") < at("  Yankee: allowed"), "{human}");
    assert!(at("  Yankee: allowed") < at("  echo: undecided"), "{human}");
    assert!(at("  echo: undecided") < at("  mike: undecided"), "{human}");
    assert!(at("  mike: undecided") < at("  zulu: denied"), "{human}");
    assert!(
        at("    echo: ") < at("    mike: "),
        "the awaiting list is by name: {human}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// No registered domain declares a `Provisioning` section at all: the
/// section stays out of the report entirely, the same "omit rather than
/// show empty" rule the environment and harnesses sections follow.
#[test]
#[cfg(unix)]
fn provisioning_section_is_absent_when_no_domain_declares() {
    let (home, _state_dir) = isolated_home("provisioning-none");
    let work = tempfile::tempdir().unwrap();
    let (config, db) = empty_config(work.path());

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["provisioning"], Value::Null);

    let human = {
        let mut cmd = bin();
        apply_home(&mut cmd, &home);
        cmd.args(["doctor", "--config"])
            .arg(&config)
            .args(["--db"])
            .arg(&db)
            .output()
            .unwrap()
            .stdout
    };
    let human = String::from_utf8(human).unwrap();
    assert!(!human.contains("provisioning:"), "{human}");

    let _ = std::fs::remove_dir_all(&home);
}

/// `--domain` restricts the provisioning section's domain-keyed lists
/// (`domains` and `pending`) to the selected domain, the same way it
/// restricts the per-domain and github sections. Two declaring, undecided
/// domains; filtering to one must leave the other out of both lists.
#[test]
#[cfg(unix)]
fn provisioning_section_honors_the_domain_filter() {
    let (home, _state_dir) = isolated_home("provisioning-filter");
    let work = tempfile::tempdir().unwrap();

    let harbor_dir = work.path().join("kb-harbor");
    write_provisioning_manifest(&harbor_dir, "harbor", "- skills: skills\n");
    write(
        &harbor_dir,
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n",
    );

    let cove_dir = work.path().join("kb-cove");
    write_provisioning_manifest(&cove_dir, "cove", "- skills: skills\n");
    write(
        &cove_dir,
        "skills/lookout/SKILL.md",
        "---\nname: lookout\n---\n\nWatches for approaching ships.\n",
    );

    let config = work.path().join("config.yaml");
    std::fs::write(
        &config,
        format!(
            "domains:\n  harbor:\n    path: {}\n  cove:\n    path: {}\n",
            harbor_dir.display(),
            cove_dir.display()
        ),
    )
    .unwrap();

    let mut cmd = bin();
    apply_home(&mut cmd, &home);
    let out = cmd
        .args(["--json", "doctor", "--domain", "harbor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(work.path().join("index.db"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let provisioning = &report["provisioning"];

    let domains = provisioning["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{provisioning}");
    assert_eq!(domains[0]["name"], json!("harbor"));

    let pending = provisioning["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{provisioning}");
    assert_eq!(pending[0]["domain"], json!("harbor"));

    let _ = std::fs::remove_dir_all(&home);
}

/// The other half of the colleague's report: running `sync` by hand on a
/// domain holding an unparseable file used to print a normal-looking summary
/// plus one `failed:` line and still exit 0, so nothing in an automated
/// pipeline ever saw the partial failure. `doctor` already exits 1 on a
/// problem and `verify` exits 2; a `sync` that silently succeeded was the
/// outlier.
#[test]
fn sync_fails_the_process_when_a_file_could_not_be_indexed() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(
        &domain_dir,
        "bad.md",
        "---\ntype: engram\ntitle: Bad\npermalink: bad\ntags: [a]\ntags: [b]\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nBody.\n",
    );

    let out = bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .get_output()
        .clone();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("added"),
        "the summary line still prints before the failure: {stdout}"
    );
    assert!(
        stdout.contains("failed: "),
        "the per-file failure line still prints: {stdout}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains('1') && stderr.to_lowercase().contains("fail"),
        "the process failure names the count: {stderr}"
    );
}

/// A clean domain (nothing failed) still exits 0, so the new failure path
/// only fires on an actual `failed` entry, never on an ordinary sync.
#[test]
fn sync_still_succeeds_when_nothing_failed() {
    let work = tempfile::tempdir().unwrap();
    let config = work.path().join("config.yaml");
    let db = work.path().join("index.db");
    let domain_dir = setup_domain(work.path(), "eng", &config);
    write(&domain_dir, "good.md", &engram("Good", "good"));

    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
}

// --- rows whose domain nobody registers any more -----------------------------
//
// The 0.17.0 shape, reproduced by hand: two domains indexed, then one of them
// unregistered with its rows left behind. The removal is a config edit rather
// than `domain remove`, because 0.18.0's removal already clears the rows - a
// test built on it would assert on an index with no orphan in it at all.

/// Drop `name` from the config file without touching the index, which is
/// exactly what a 0.17.0 removal left behind.
fn unregister(config: &Path, name: &str) {
    let mut cfg: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(config).unwrap();
    cfg.domains.shift_remove(name);
    crystalline_core::config::save_yaml(config, &cfg).unwrap();
}

/// Two domains synced, one unregistered afterwards.
fn orphan_fixture(work: &Path) -> (PathBuf, PathBuf) {
    let config = work.join("config.yaml");
    let db = work.join("index.db");
    let kept = setup_domain(work, "eng", &config);
    let gone = setup_domain(work, "retired", &config);
    write(&kept, "a.md", &engram("A", "a"));
    write(&gone, "b.md", &engram("B", "b"));
    bin()
        .args(["sync", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success();
    unregister(&config, "retired");
    (config, db)
}

/// The report: the rows are named, counted and counted as a problem, and not
/// one of them is removed by a run that was only asked to look.
#[test]
fn reports_the_rows_of_a_domain_nobody_registers() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = orphan_fixture(work.path());

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let rows = &report["orphaned_rows"]["domains"];
    assert_eq!(rows[0]["name"], "retired", "the orphan is named: {report}");
    assert!(
        rows[0]["engrams"].as_i64().unwrap() >= 2,
        "with the rows at stake: {report}"
    );
    assert_eq!(
        rows[0]["age_days"],
        Value::Null,
        "an index inherited from a version that never stamped has no age: {report}"
    );
    assert_eq!(rows[0]["collected"], false, "nothing was removed: {report}");
    assert_eq!(rows[0]["collectable"], true, "and --fix would: {report}");

    // The human render says what is true of them and names the one command
    // that ends them now. A full reindex is never the advice.
    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let human = cmd
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("retired") && human.contains("never seen registered"),
        "the render names the domain and how long it has been gone: {human}"
    );
    assert!(
        human.contains("crystalline doctor --fix"),
        "and names the immediate path: {human}"
    );
    assert!(
        !human.to_lowercase().contains("reindex"),
        "and never the heaviest command in the tool: {human}"
    );

    // A second look finds the same rows: looking removes nothing.
    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let again = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let again: Value = serde_json::from_slice(&again).unwrap();
    assert_eq!(
        again["orphaned_rows"]["domains"][0]["engrams"], rows[0]["engrams"],
        "every row is where it was: {again}"
    );
}

/// The fix: a person asking is the signal the grace period waits for, so the
/// rows go on the run that was asked, not a week later.
#[test]
fn fix_collects_the_rows_of_a_domain_nobody_registers() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = orphan_fixture(work.path());

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let rows = &report["orphaned_rows"]["domains"];
    assert_eq!(rows[0]["name"], "retired", "{report}");
    assert_eq!(
        rows[0]["collected"], true,
        "the never-stamped orphan is collected on the run a person asked for: {report}"
    );

    // And it is gone: the next look has nothing left to report.
    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let after = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let after: Value = serde_json::from_slice(&after).unwrap();
    assert_eq!(
        after["orphaned_rows"]["domains"],
        serde_json::json!([]),
        "a domain with no rows left is nothing to report: {after}"
    );
}

/// A read-only instance collects nothing and says so, with the rows it would
/// have collected still named: that operator is exactly the one who wants to
/// know what is sitting in their index, and a `--fix` that silently did
/// nothing would tell them the opposite.
#[test]
fn a_read_only_instance_reports_the_rows_and_collects_none_of_them() {
    let work = tempfile::tempdir().unwrap();
    let (config, db) = orphan_fixture(work.path());
    let mut cfg: crystalline_core::config::GlobalConfig =
        crystalline_core::config::load_yaml(&config).unwrap();
    cfg.service = Some(crystalline_core::config::ServiceConfig {
        read_only: Some(true),
        ..crystalline_core::config::ServiceConfig::default()
    });
    crystalline_core::config::save_yaml(&config, &cfg).unwrap();

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let out = cmd
        .args(["--json", "doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    let rows = &report["orphaned_rows"]["domains"];
    assert_eq!(rows[0]["name"], "retired", "the rows are named: {report}");
    assert_eq!(rows[0]["collected"], false, "and not collected: {report}");
    assert_eq!(rows[0]["kept"], "read_only", "{report}");
    assert!(
        report["orphaned_rows"]["skipped"]
            .as_str()
            .unwrap()
            .contains("read-only"),
        "and the report says why: {report}"
    );

    let mut cmd = bin();
    let _home = shield_ambient_home(&mut cmd);
    let human = cmd
        .args(["doctor", "--fix", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let human = String::from_utf8(human).unwrap();
    assert!(
        human.contains("nothing was collected: this instance is read-only"),
        "the render says it too: {human}"
    );
}

/// Rewind the receipt so it reads as written by an older release that did
/// not know the newest managed skill: the row version goes to 0.0.1 and the
/// skill's record is dropped. Returns the name of a skill whose `since` is
/// newer than that, taken from the receipt's own list so the test follows
/// the shipped set.
#[cfg(unix)]
fn age_receipt(state_dir: &Path, skill: &str) {
    let path = state_dir.join("installs.json");
    let mut receipt: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for row in receipt["installs"].as_array_mut().unwrap() {
        row["version"] = json!("0.0.1");
        if let Some(skills) = row["skills"].as_array_mut() {
            skills.retain(|s| s["name"] != skill);
        }
    }
    std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
}

/// A skill new in a release newer than the row's version is "not installed
/// yet" for a row that installs skills, and nothing is promised to a row
/// installed with `--skip-skills`.
#[test]
#[cfg(unix)]
fn a_new_managed_skill_is_pending_only_for_a_row_that_installs_skills() {
    let skill = "crystalline-provisioning";
    for (flags, pending) in [
        (&["--skip-mcp"][..], true),
        (&["--skip-mcp", "--skip-skills"][..], false),
    ] {
        let (home, state) = isolated_home("harness-pending-skill");
        let work = tempfile::tempdir().unwrap();
        let mut args = vec!["install", "claude-code"];
        args.extend_from_slice(flags);
        run_in(&home, &args);
        let _ = std::fs::remove_dir_all(home.join(".claude/skills").join(skill));
        age_receipt(&state, skill);
        let report = doctor_json(&home, work.path());
        let claude = harness_entry(&report, "claude-code");
        let listed = claude
            .get("skills_pending")
            .and_then(|p| p.as_array())
            .is_some_and(|p| p.iter().any(|n| n == skill));
        assert_eq!(listed, pending, "{flags:?}: {claude}");
        let human = doctor_human(&home, work.path());
        assert_eq!(
            human.contains("next session start installs it"),
            pending,
            "{human}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}

/// Doctor classifies the MCP entry with the planner install runs: a bare
/// command where the profile wants the absolute path is a problem install
/// repairs, not "up-to-date".
#[test]
#[cfg(unix)]
fn a_bare_command_where_an_absolute_path_is_wanted_is_a_problem() {
    let (home, _) = isolated_home("harness-cursor-bare");
    let work = tempfile::tempdir().unwrap();
    run_in(&home, &["install", "cursor"]);
    let file = home.join(".cursor/mcp.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let bin = shim_bin(&home).join("crystalline");
    assert!(text.contains(bin.to_str().unwrap()), "{text}");
    std::fs::write(&file, text.replace(bin.to_str().unwrap(), "crystalline")).unwrap();

    let (config, db) = empty_config(work.path());
    let mut cmd = bin_cmd(&home);
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(harness_entry(&report, "cursor")["mcp_entry"], "ours-older");
    let human = doctor_human_code(&home, work.path(), 1);
    assert!(human.contains("Run: crystalline install cursor"), "{human}");

    // The repair the line names makes it green again.
    run_in(&home, &["install", "cursor"]);
    let report = doctor_json(&home, work.path());
    assert_eq!(harness_entry(&report, "cursor")["mcp_entry"], "up-to-date");
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(unix)]
fn bin_cmd(home: &Path) -> Command {
    let mut cmd = bin();
    apply_home(&mut cmd, home);
    cmd.env_remove("COPILOT_HOME").env("PATH", shim_bin(home));
    cmd
}

#[cfg(unix)]
fn doctor_human_code(home: &Path, work: &Path, code: i32) -> String {
    let (config, db) = empty_config(work);
    let out = bin_cmd(home)
        .args(["doctor", "--config"])
        .arg(&config)
        .args(["--db"])
        .arg(&db)
        .assert()
        .code(code)
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// How many skill folders (each with a `SKILL.md`) a skills folder holds, so
/// the tests follow the shipped set instead of a hard-coded count.
#[cfg(unix)]
fn skill_dirs(folder: &Path) -> usize {
    std::fs::read_dir(folder)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().join("SKILL.md").is_file())
        .count()
}
