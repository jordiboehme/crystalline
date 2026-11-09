//! `crystalline kept`, and the kept texts `doctor` and `status` report: the
//! texts the server kept when a co-editing room could not save. Nothing here
//! deletes one unless a person says so.

use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use crystalline_core::kept::{self, KeptKind};
use serde_json::Value;

fn run(home: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = crate::common::crystalline();
    crate::common::isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_SERVICE_HTTP", "false")
        .env("CRYSTALLINE_TEST_DAEMON_TASK", "ready")
        .args(args)
        .output()
        .unwrap()
}

fn kept_root(home: &Path) -> PathBuf {
    crate::common::isolated_state_dir(home).join(kept::KEPT_DIR)
}

/// A page's text kept an hour ago and an engram's text kept 45 days ago.
fn plant(home: &Path) -> (PathBuf, PathBuf) {
    let root = kept_root(home);
    let page = kept::write(
        &root,
        "eng",
        None,
        KeptKind::Reflection,
        "guides/a.md",
        Utc::now() - Duration::hours(1),
        "Typed on the page.\n",
    )
    .unwrap();
    let engram = kept::write(
        &root,
        "ops",
        None,
        KeptKind::Engram,
        "runbooks/deploy.md",
        Utc::now() - Duration::days(45),
        "Typed on the engram.\n",
    )
    .unwrap();
    (page, engram)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn listed(home: &Path, extra: &[&str]) -> Vec<Value> {
    let mut args = vec!["kept", "list", "--json"];
    args.extend_from_slice(extra);
    let out = run(home, &args);
    assert!(out.status.success(), "{out:?}");
    let body: Value = serde_json::from_slice(&out.stdout).unwrap();
    body["kept"].as_array().unwrap().clone()
}

#[test]
fn kept_list_shows_every_text_and_filters_by_domain() {
    let home = tempfile::tempdir().unwrap();
    let (page, engram) = plant(home.path());
    let all = listed(home.path(), &[]);
    assert_eq!(all.len(), 2, "{all:?}");
    // Oldest first.
    assert_eq!(all[0]["domain"], "ops");
    assert_eq!(all[0]["kind"], "engram");
    assert_eq!(all[0]["path"], "runbooks/deploy.md");
    assert_eq!(all[0]["bytes"], 21);
    assert_eq!(all[0]["file"], engram.display().to_string());
    assert_eq!(all[1]["file"], page.display().to_string());
    let eng = listed(home.path(), &["--domain", "eng"]);
    assert_eq!(eng.len(), 1);
    assert_eq!(eng[0]["path"], "guides/a.md");

    let human = stdout(&run(home.path(), &["kept", "list"]));
    let id = eng[0]["id"].as_str().unwrap();
    assert!(human.contains(id), "{human}");
    assert!(human.contains("eng reflection guides/a.md"), "{human}");
    assert!(human.contains(&page.display().to_string()), "{human}");
    assert!(human.contains("crystalline kept show <id>"), "{human}");

    let none = tempfile::tempdir().unwrap();
    assert_eq!(
        stdout(&run(none.path(), &["kept", "list"])).trim(),
        "No kept texts."
    );
}

#[test]
fn kept_show_prints_the_text_as_it_was_kept() {
    let home = tempfile::tempdir().unwrap();
    plant(home.path());
    let id = listed(home.path(), &["--domain", "eng"])[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = run(home.path(), &["kept", "show", &id]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(stdout(&out), "Typed on the page.\n");
    let json: Value =
        serde_json::from_slice(&run(home.path(), &["kept", "show", &id, "--json"]).stdout).unwrap();
    assert_eq!(json["text"], "Typed on the page.\n");
    assert_eq!(json["path"], "guides/a.md");

    let missing = run(home.path(), &["kept", "show", "000000000000"]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no kept text '000000000000'"),
        "{missing:?}"
    );
}

/// `drop` deletes only on a yes: without a terminal and without `--yes` it
/// refuses, `--json` needs `--yes`, and `--yes` drops that one file.
#[test]
fn kept_drop_asks_first_and_drops_one_text() {
    let home = tempfile::tempdir().unwrap();
    let (page, engram) = plant(home.path());
    let id = listed(home.path(), &["--domain", "eng"])[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let refused = run(home.path(), &["kept", "drop", &id]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("pass --yes"),
        "{refused:?}"
    );
    let refused = run(home.path(), &["kept", "drop", &id, "--json"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--json requires --yes"),
        "{refused:?}"
    );
    assert!(page.is_file(), "nothing is dropped without a yes");

    let dropped = run(home.path(), &["kept", "drop", &id, "--yes"]);
    assert!(dropped.status.success(), "{dropped:?}");
    assert!(
        stdout(&dropped).contains("Dropped the text kept for eng reflection guides/a.md"),
        "{dropped:?}"
    );
    assert!(!page.exists());
    assert!(engram.is_file(), "the other text stays");
    assert_eq!(listed(home.path(), &[]).len(), 1);
}

/// `doctor` lists every kept text, warns about the one older than 30 days,
/// counts neither as a problem, and `--fix` deletes neither.
#[test]
fn doctor_lists_kept_texts_warns_on_old_ones_and_fix_keeps_them() {
    let home = tempfile::tempdir().unwrap();
    let before = run(home.path(), &["doctor", "--json"]);
    let clean: Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(clean["kept"], serde_json::json!([]));

    let (page, engram) = plant(home.path());
    let out = run(home.path(), &["doctor", "--json"]);
    assert_eq!(
        out.status.code(),
        before.status.code(),
        "kept texts are no problem"
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = report["kept"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{report}");
    assert_eq!(rows[0]["domain"], "ops");
    assert_eq!(rows[0]["stale"], true);
    assert_eq!(rows[1]["stale"], false);
    assert_eq!(rows[1]["file"], page.display().to_string());

    let fixed = run(home.path(), &["doctor", "--fix"]);
    let text = stdout(&fixed);
    assert!(text.contains("kept texts:"), "{text}");
    assert!(
        text.contains("[warning]") && text.contains("ops engram runbooks/deploy.md"),
        "{text}"
    );
    assert!(text.contains("more than 30 days ago"), "{text}");
    assert!(text.contains("doctor never deletes them"), "{text}");
    assert!(page.is_file() && engram.is_file(), "--fix deletes nothing");
}

/// `status` says how many texts are kept, in one line, only when there are
/// any; `--json` carries the count the same way.
#[test]
fn status_counts_the_kept_texts_only_when_there_are_any() {
    let home = tempfile::tempdir().unwrap();
    let none = stdout(&run(home.path(), &["status"]));
    assert!(!none.contains("Kept texts"), "{none}");
    let json: Value =
        serde_json::from_slice(&run(home.path(), &["status", "--json"]).stdout).unwrap();
    assert!(json.get("kept_texts").is_none(), "{json}");

    plant(home.path());
    let text = stdout(&run(home.path(), &["status"]));
    assert!(
        text.contains("Kept texts: 2 (crystalline kept list)"),
        "{text}"
    );
    let json: Value =
        serde_json::from_slice(&run(home.path(), &["status", "--json"]).stdout).unwrap();
    assert_eq!(json["kept_texts"], 2);
}

/// The same line and key when a daemon answers `status`: the count is read
/// from this machine's state folder either way.
#[cfg(unix)]
#[test]
fn status_through_the_daemon_counts_the_kept_texts() {
    let mut env = crate::daemon_env::DaemonEnv::new("kept");
    std::fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    std::fs::write(
        env.config_path(),
        "embeddings:\n  provider: openai-compatible\n  model: stub\n",
    )
    .unwrap();
    let root = env.state_dir().join(kept::KEPT_DIR);
    kept::write(
        &root,
        "eng",
        None,
        KeptKind::Reflection,
        "a.md",
        Utc::now(),
        "x",
    )
    .unwrap();
    env.serve();
    let (ok, text) = env.run(&["status"]);
    assert!(ok, "{text}");
    assert!(text.contains("running (pid"), "the daemon answered: {text}");
    assert!(
        text.contains("Kept texts: 1 (crystalline kept list)"),
        "{text}"
    );
    let (ok, json) = env.run(&["status", "--json"]);
    assert!(ok, "{json}");
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["kept_texts"], 1, "{json}");
}

/// A temporary file a keep left when its process stopped is listed as an
/// unfinished keep with its path, `kept show` prints it, and doctor warns
/// about it; nothing deletes it.
#[test]
fn an_unfinished_keep_is_listed_shown_and_warned_about() {
    let home = tempfile::tempdir().unwrap();
    let folder = kept_root(home.path())
        .join("eng")
        .join("engram")
        .join("runbooks");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join(".deploy.md.20261015T080000Z.42-0.keeping");
    std::fs::write(&file, "Half kept.\n").unwrap();
    // Left more than a minute: a keep still being written is not listed.
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120))
        .unwrap();

    let all = listed(home.path(), &[]);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["unfinished"], true);
    assert_eq!(all[0]["path"], "runbooks/deploy.md");
    assert_eq!(all[0]["file"], file.display().to_string());
    let human = stdout(&run(home.path(), &["kept", "list"]));
    assert!(
        human.contains("eng engram runbooks/deploy.md (unfinished keep)"),
        "{human}"
    );
    let id = all[0]["id"].as_str().unwrap();
    assert_eq!(
        stdout(&run(home.path(), &["kept", "show", id])),
        "Half kept.\n"
    );
    let doctor = stdout(&run(home.path(), &["doctor", "--fix"]));
    assert!(
        doctor.contains("[warning]") && doctor.contains("a keep that never finished"),
        "{doctor}"
    );
    assert!(file.is_file(), "nothing deletes it");
}

/// Kept texts left under a name no domain owns are set aside when a domain
/// takes the name, so the new domain never reaches them.
#[test]
fn a_new_domain_never_reaches_texts_left_under_its_name() {
    let home = tempfile::tempdir().unwrap();
    kept::write(
        &kept_root(home.path()),
        "fresh",
        None,
        KeptKind::Engram,
        "old.md",
        Utc::now(),
        "left over",
    )
    .unwrap();
    let out = run(home.path(), &["domain", "add", "fresh", "--virtual"]);
    assert!(out.status.success(), "{out:?}");
    let all = listed(home.path(), &[]);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["removed"], true, "{all:?}");
    assert!(!kept_root(home.path()).join("fresh").exists());
}

/// A draft file kept as its bytes is shown as a plain sentence that says
/// where it is, never as a text error, and the list gives no text hint for it.
#[test]
fn a_kept_file_is_shown_as_a_file_and_not_as_text() {
    let home = tempfile::tempdir().unwrap();
    let root = kept_root(home.path());
    let file = kept::write_bytes(
        &root,
        "team",
        Some("alice"),
        KeptKind::Engram,
        "assets/deck.png",
        Utc::now(),
        b"\x89PNG\r\n\x1a\n\x00\xff her deck",
    )
    .unwrap();
    let all = listed(home.path(), &[]);
    let id = all[0]["id"].as_str().unwrap();

    let out = run(home.path(), &["kept", "show", id]);
    assert!(!out.status.success(), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!(
            "This kept copy is a file, not text. It is at {}.",
            file.display()
        )),
        "{err}"
    );
    assert!(!err.contains("UTF-8"), "{err}");

    let human = stdout(&run(home.path(), &["kept", "list"]));
    assert!(human.contains("A file, not text."), "{human}");
    assert!(!human.contains("Copy the text back"), "{human}");
}
