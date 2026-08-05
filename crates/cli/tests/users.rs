//! End-to-end tests for `crystalline users`, the account management command
//! for the web API. Every child is isolated with its own `HOME` and XDG base
//! directories, so the accounts land in a temp state directory rather than in
//! the developer's own `web-auth.db`.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect `HOME` and the XDG base directories into `home`. The auth database
/// lives at `<state_dir>/web-auth.db`, and `XDG_STATE_HOME` is what
/// `crystalline_core::config::state_dir` resolves it from.
fn isolate(cmd: &mut Command, home: &std::path::Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"));
}

/// Run `crystalline users ...` in the isolated home, feeding `stdin` when
/// given, and return stdout on success.
fn users_ok(home: &std::path::Path, args: &[&str], stdin: Option<&str>) -> String {
    let mut cmd = bin();
    isolate(&mut cmd, home);
    cmd.arg("users").args(args);
    if let Some(input) = stdin {
        cmd.write_stdin(input.to_string());
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "users {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// The same, for a command that must fail. Returns stderr.
fn users_err(home: &std::path::Path, args: &[&str], stdin: Option<&str>) -> String {
    let mut cmd = bin();
    isolate(&mut cmd, home);
    cmd.arg("users").args(args);
    if let Some(input) = stdin {
        cmd.write_stdin(input.to_string());
    }
    let out = cmd.output().unwrap();
    assert!(
        !out.status.success(),
        "users {args:?} unexpectedly succeeded: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn users_add_and_list() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let out = users_ok(home.path(), &["list"], None);
    assert!(out.contains("ada"), "the account is listed: {out}");
    assert!(out.contains("admin"), "its role is listed: {out}");
}

#[test]
fn add_defaults_to_viewer_and_records_display_and_email() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &[
            "add",
            "  Ada  ",
            "--display",
            "Ada Lovelace",
            "--email",
            "ada@example.com",
            "--password-stdin",
        ],
        Some("s3cret\n"),
    );

    let out = users_ok(home.path(), &["--json", "list"], None);
    let users: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ada = &users["users"][0];
    assert_eq!(ada["name"], "ada", "the store folds the name: {out}");
    assert_eq!(ada["display"], "Ada Lovelace");
    assert_eq!(ada["email"], "ada@example.com");
    assert_eq!(ada["role"], "viewer", "the default role is viewer");
    assert_eq!(ada["disabled"], false);
}

#[test]
fn the_whole_lifecycle_round_trips() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );
    users_ok(
        home.path(),
        &["add", "bob", "--password-stdin"],
        Some("hunter2\n"),
    );

    users_ok(home.path(), &["role", "bob", "editor"], None);
    users_ok(
        home.path(),
        &["passwd", "bob", "--password-stdin"],
        Some("hunter3\n"),
    );
    users_ok(home.path(), &["disable", "bob"], None);

    let out = users_ok(home.path(), &["--json", "list"], None);
    let users: serde_json::Value = serde_json::from_str(&out).unwrap();
    let bob = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == "bob")
        .unwrap();
    assert_eq!(bob["role"], "editor");
    assert_eq!(bob["disabled"], true);

    users_ok(home.path(), &["enable", "bob"], None);
    users_ok(home.path(), &["remove", "bob"], None);

    let out = users_ok(home.path(), &["list"], None);
    assert!(!out.contains("bob"), "bob is gone: {out}");
    assert!(out.contains("ada"), "ada is untouched: {out}");
}

#[test]
fn the_last_admin_cannot_be_removed() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let err = users_err(home.path(), &["remove", "ada"], None);
    assert!(
        err.contains("last admin"),
        "the lockout refusal is surfaced: {err}"
    );

    // With a second admin the removal goes through.
    users_ok(
        home.path(),
        &["add", "bob", "--role", "admin", "--password-stdin"],
        Some("hunter2\n"),
    );
    users_ok(home.path(), &["remove", "ada"], None);
}

#[test]
fn adding_the_same_name_twice_says_so_in_words() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--password-stdin"],
        Some("s3cret\n"),
    );
    // Any casing of the same name is the same account.
    let err = users_err(
        home.path(),
        &["add", "ADA", "--password-stdin"],
        Some("other\n"),
    );
    assert!(err.contains("already exists"), "{err}");
    assert!(
        !err.contains("UNIQUE constraint"),
        "the raw constraint violation must not reach the operator: {err}"
    );
}

#[test]
fn a_mistyped_name_is_reported() {
    let home = tempfile::tempdir().unwrap();
    let err = users_err(home.path(), &["role", "ghost", "admin"], None);
    assert!(err.contains("no such user"), "{err}");
}

#[test]
fn an_empty_list_says_so() {
    let home = tempfile::tempdir().unwrap();
    let out = users_ok(home.path(), &["list"], None);
    assert!(out.contains("No users"), "{out}");
}

#[test]
fn a_password_is_required_and_a_non_terminal_run_must_pass_password_stdin() {
    let home = tempfile::tempdir().unwrap();
    // No `--password-stdin` and no terminal: refuse rather than hang.
    let err = users_err(home.path(), &["add", "ada"], None);
    assert!(err.contains("--password-stdin"), "{err}");

    // An empty password is refused too.
    let err = users_err(home.path(), &["add", "ada", "--password-stdin"], Some("\n"));
    assert!(err.contains("password"), "{err}");
}
