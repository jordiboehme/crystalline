//! End-to-end tests for `crystalline users`, the account management command
//! for the web API. Every child is isolated with its own `HOME` and XDG base
//! directories, so the accounts land in a temp state directory rather than in
//! the developer's own `web-auth.db`.

use assert_cmd::Command;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect every base directory this child can resolve into `home`, so the
/// auth database at `<state_dir>/web-auth.db` is this test's alone.
///
/// Both families are needed, because `state_dir` goes through etcetera's
/// `choose_base_strategy`, which is a different strategy per platform: the XDG
/// one on unix and macOS, which reads `HOME` and `XDG_*_HOME`, and the Windows
/// one, which reads `USERPROFILE`, `APPDATA` and `LOCALAPPDATA` and ignores
/// the XDG variables entirely. On Windows it also has no state directory of
/// its own, so `state_dir` falls back to the data directory, `APPDATA`.
/// Setting only the XDG variables would leave every test in this file
/// resolving one real `%APPDATA%\crystalline\web-auth.db` and locking each
/// other out of it (Windows byte-range locks are mandatory). The Windows names
/// and layout mirror `service_windows.rs`, which isolates the daemon the same
/// way; setting them on unix as well is harmless there and keeps this helper
/// free of a `cfg`.
fn isolate(cmd: &mut Command, home: &std::path::Path) {
    for (name, value) in isolation_env(home) {
        cmd.env(name, value);
    }
}

/// The variables [`isolate`] sets, as pairs, because one other place needs the
/// same set and cannot go through [`isolate`]:
/// `users_add_works_while_another_process_holds_the_auth_db` spawns its holder
/// with a plain [`std::process::Command`], not an `assert_cmd` one. Both read
/// this list, so a variable added here reaches both and the two cannot drift
/// into resolving different `web-auth.db` files.
fn isolation_env(home: &std::path::Path) -> [(&'static str, std::path::PathBuf); 7] {
    [
        ("HOME", home.to_path_buf()),
        ("XDG_CONFIG_HOME", home.join("config")),
        ("XDG_STATE_HOME", home.join("state")),
        ("XDG_CACHE_HOME", home.join("cache")),
        ("USERPROFILE", home.to_path_buf()),
        ("APPDATA", home.join("roaming")),
        ("LOCALAPPDATA", home.join("local")),
    ]
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

/// Names the auth database this process must hold open, turning this test
/// binary into the stand-in for a running daemon (see [`holds_the_auth_db`]).
/// The value is the isolated home, from which the child derives every path.
const HOLD_ENV: &str = "CRYSTALLINE_TEST_AUTH_HOLD";

/// The child touches this file once its `AuthStore` is open, and exits once
/// the parent creates [`STOP_FILE`] beside it.
const READY_FILE: &str = "auth-hold-ready";
const STOP_FILE: &str = "auth-hold-stop";

/// The daemon stand-in, run as a child process by
/// [`users_add_works_while_another_process_holds_the_auth_db`] and a no-op in
/// an ordinary test run.
///
/// It has to be a real second process. Two `AuthStore`s in one process prove
/// nothing about this: turso keeps a process-wide registry of open databases
/// keyed by file identity, so the second open is handed the first one's
/// `Database` and never touches the file lock that the CLI trips over. See
/// `two_stores_on_one_file_interleave_writes` in `crystalline-service`, which
/// is the same-process test and says so.
#[test]
fn holds_the_auth_db() {
    let Ok(home) = std::env::var(HOLD_ENV) else {
        return;
    };
    let home = std::path::PathBuf::from(home);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let path = crystalline_core::config::web_auth_db_path().unwrap();
        let store = crystalline_service::rest::AuthStore::open(&path)
            .await
            .expect("the holder must be able to open the auth database");
        std::fs::write(home.join(READY_FILE), "").unwrap();

        // Keep writing while the parent writes, the way a serving daemon
        // issues sessions while an operator edits accounts. This is what puts
        // the busy timeout to work across processes, which only has a chance
        // of mattering once the open no longer locks the whole file.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !home.join(STOP_FILE).exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the parent never came"
            );
            store
                .create_session("ada", 3600)
                .await
                .expect("the daemon's own writes must not fail while the CLI writes");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // Still holding the same open store, the writes the other process made
        // meanwhile are visible without reopening.
        let names: Vec<String> = store
            .list_users()
            .await
            .expect("listing must work after the other process wrote")
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert!(
            names.iter().any(|n| n == "bob"),
            "the holder sees the account the CLI added: {names:?}"
        );
    });
}

/// The bug this guards: while `serve` holds `web-auth.db` open, every
/// `crystalline users` command used to fail at open time, because turso's
/// default open takes a whole-file exclusive advisory lock for the life of the
/// handle. Neither the busy timeout nor `BEGIN IMMEDIATE` could help, since
/// both come after the open. `AuthStore` therefore opens with turso's
/// multiprocess WAL, and this test is the only one that can tell the
/// difference: it holds the database open in a second, real process.
#[test]
fn users_add_works_while_another_process_holds_the_auth_db() {
    let home = tempfile::tempdir().unwrap();
    // Create the database first, so the holder does not race the CLI over
    // which process gets to create the file.
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["holds_the_auth_db", "--exact", "--nocapture"])
        .env(HOLD_ENV, home.path());
    for (name, value) in isolation_env(home.path()) {
        command.env(name, value);
    }
    let mut holder = Holder(command.spawn().unwrap());

    let ready = home.path().join(READY_FILE);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !ready.exists() {
        if let Some(status) = holder.0.try_wait().unwrap() {
            panic!("the holder exited before it opened the database: {status}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the holder never opened the database"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // The real thing: a second process writes while the first holds its handle.
    users_ok(
        home.path(),
        &["add", "bob", "--password-stdin"],
        Some("hunter2\n"),
    );
    let out = users_ok(home.path(), &["list"], None);
    assert!(out.contains("bob"), "the write landed: {out}");

    std::fs::write(home.path().join(STOP_FILE), "").unwrap();
    let status = holder.0.wait().unwrap();
    assert!(
        status.success(),
        "the holder's own assertions must have passed: {status}"
    );
}

/// The spawned holder, killed when this goes out of scope.
///
/// Every path between the spawn and the orderly stop can panic - a failing
/// `users_ok`, the readiness loop timing out, the `bob` assertion - and each
/// one would otherwise leave the child running for its full 60 second timeout:
/// holding the inherited stdout pipe, which nextest reports as a leak, and on
/// Windows holding `web-auth.db` open, so the `TempDir` would fail to delete
/// its own directory on the way out. Killing on unwind makes every failure
/// path shut the holder down at once. On the success path the child has
/// already exited through `STOP_FILE` by the time this runs, and both calls
/// below are no-ops.
struct Holder(std::process::Child);

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
