//! End-to-end tests for `crystalline users`, the account management command
//! for the web API. Every child is isolated with its own `HOME` and XDG base
//! directories, so the accounts land in a temp state directory rather than in
//! the developer's own `web-auth.db`.

use assert_cmd::Command;

use crate::common::isolate;
// Used only by the cross-process test below, which is unix-only (see there).
#[cfg(unix)]
use crate::common::isolation_env;

fn bin() -> Command {
    crate::common::crystalline()
}

// `isolate` and `isolation_env` (both from `common`) redirect every base
// directory this child can resolve into `home`, so the auth database at
// `<state_dir>/web-auth.db` is this test's alone. `isolation_env` is used
// directly (not through `isolate`) by
// `users_add_works_while_another_process_holds_the_auth_db`, which spawns its
// holder with a plain `std::process::Command`, not an `assert_cmd` one - both
// read the same list, so a variable added there reaches both and the two
// cannot drift into resolving different `web-auth.db` files.

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
fn the_last_admin_cannot_be_demoted_without_force() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let err = users_err(home.path(), &["demote", "ada"], None);
    assert!(
        err.contains("last admin"),
        "the lockout refusal is surfaced: {err}"
    );

    let out = users_ok(home.path(), &["--json", "list"], None);
    let users: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        users["users"][0]["role"], "admin",
        "the refused demotion left the role untouched"
    );
}

#[test]
fn demote_defaults_to_viewer() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );
    users_ok(
        home.path(),
        &["add", "bob", "--role", "admin", "--password-stdin"],
        Some("hunter2\n"),
    );

    let out = users_ok(home.path(), &["demote", "bob"], None);
    assert!(out.contains("viewer"), "{out}");

    let out = users_ok(home.path(), &["--json", "list"], None);
    let users: serde_json::Value = serde_json::from_str(&out).unwrap();
    let bob = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["name"] == "bob")
        .unwrap();
    assert_eq!(bob["role"], "viewer");
}

#[test]
fn demote_force_bypasses_the_last_admin_guard() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let out = users_ok(home.path(), &["demote", "ada", "--force"], None);
    assert!(out.contains("forced"), "{out}");
    assert!(
        out.contains("no admin"),
        "the operator is warned about the lockout: {out}"
    );

    let out = users_ok(home.path(), &["--json", "list"], None);
    let users: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        users["users"][0]["role"], "viewer",
        "the forced demotion went through despite being the last admin"
    );
}

#[test]
fn demote_force_can_set_a_specific_role() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    users_ok(
        home.path(),
        &["demote", "ada", "--role", "editor", "--force"],
        None,
    );

    let out = users_ok(home.path(), &["--json", "list"], None);
    let users: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(users["users"][0]["role"], "editor");
}

#[test]
fn remove_force_bypasses_the_last_admin_guard() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let out = users_ok(home.path(), &["remove", "ada", "--force"], None);
    assert!(out.contains("forced"), "{out}");
    assert!(
        out.contains("no admin"),
        "the operator is warned about the lockout: {out}"
    );

    let out = users_ok(home.path(), &["list"], None);
    assert!(
        !out.contains("ada"),
        "the forced removal went through: {out}"
    );
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

/// An account that connected a GitHub identity for sharing leaves a credential
/// behind; disabling or removing the account sweeps it, best effort, and never
/// touches anybody else's.
///
/// The probe account's name is one no real install carries, because the sweep
/// does reach this machine's credential store: it deletes whatever is filed
/// under that name, so only a name nobody connected is safe to point it at.
#[test]
#[cfg(unix)]
fn disabling_and_removing_an_account_forget_its_github_identity() {
    let home = tempfile::tempdir().unwrap();
    let origins = home.path().join("state/crystalline/origins");
    std::fs::create_dir_all(&origins).unwrap();
    let credential = |account: &str| origins.join(format!("github-token-personal-{account}.json"));
    let write_credential = |account: &str| {
        std::fs::write(
            credential(account),
            r#"{"access_token":"gho_x","host":"github.com","user":"probe","created_at":"2026-08-29T00:00:00Z"}"#,
        )
        .unwrap();
    };

    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );
    users_ok(
        home.path(),
        &["add", "sweep-probe", "--role", "editor", "--password-stdin"],
        Some("hunter2\n"),
    );
    write_credential("sweep-probe");
    write_credential("ada");

    users_ok(home.path(), &["disable", "sweep-probe"], None);
    assert!(
        !credential("sweep-probe").exists(),
        "disabling an account forgets the identity it shared with"
    );
    assert!(
        credential("ada").exists(),
        "and forgets nobody else's identity"
    );

    users_ok(home.path(), &["enable", "sweep-probe"], None);
    write_credential("sweep-probe");
    users_ok(home.path(), &["remove", "sweep-probe"], None);
    assert!(
        !credential("sweep-probe").exists(),
        "removing an account forgets it too"
    );
    assert!(credential("ada").exists());
}

/// A token left under a name (an earlier removal whose delete failed) is gone
/// before a new account under that name can act, and a delete that fails now
/// leaves no account.
#[test]
#[cfg(unix)]
fn adding_an_account_sweeps_a_token_left_under_its_name() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().unwrap();
    let origins = home.path().join("state/crystalline/origins");
    std::fs::create_dir_all(&origins).unwrap();
    let credential = origins.join("github-token-personal-sweep-new.json");
    let left = r#"{"access_token":"gho_x","host":"github.com","user":"probe","created_at":"2026-08-29T00:00:00Z"}"#;
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    std::fs::write(&credential, left).unwrap();
    std::fs::set_permissions(&origins, std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = users_err(
        home.path(),
        &["add", "sweep-new", "--role", "editor", "--password-stdin"],
        Some("hunter2\n"),
    );
    std::fs::set_permissions(&origins, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.contains("GitHub token"), "{err}");
    assert!(credential.exists());
    let list = users_ok(home.path(), &["list"], None);
    assert!(!list.contains("sweep-new"), "{list}");

    users_ok(
        home.path(),
        &["add", "sweep-new", "--role", "editor", "--password-stdin"],
        Some("hunter2\n"),
    );
    assert!(!credential.exists(), "swept");
    let list = users_ok(home.path(), &["list"], None);
    assert!(list.contains("sweep-new"), "{list}");
}

/// A store that cannot answer used to read as "no token left", where REST
/// refuses. A token file that cannot be read or deleted refuses here too.
#[cfg(unix)]
#[test]
fn adding_an_account_refuses_when_the_token_store_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().unwrap();
    let origins = home.path().join("state/crystalline/origins");
    std::fs::create_dir_all(&origins).unwrap();
    let credential = origins.join("github-token-personal-unread.json");
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );
    std::fs::write(&credential, "this is not a token").unwrap();
    std::fs::set_permissions(&origins, std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = users_err(
        home.path(),
        &["add", "unread", "--role", "editor", "--password-stdin"],
        Some("hunter2\n"),
    );
    std::fs::set_permissions(&origins, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.contains("could not be read"), "{err}");
    let list = users_ok(home.path(), &["list"], None);
    assert!(!list.contains("unread"), "{list}");
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

/// The two identity verbs: an administrator ties a provider identity to an
/// account and takes it away again. This is the surface the spec calls the
/// repair for a provider that re-issued its subjects, so the messages have to
/// say what happened to which account.
#[test]
fn users_link_and_unlink_move_an_identity() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );

    let out = users_ok(
        home.path(),
        &[
            "link",
            "ada",
            "--issuer",
            "https://idp.example",
            "--subject",
            "sub-1",
        ],
        None,
    );
    assert!(out.contains("ada"), "{out}");
    assert!(out.contains("https://idp.example"), "{out}");

    // The same pair again is refused, naming who holds it: the operator at
    // the command line is exactly who may know that.
    let err = users_err(
        home.path(),
        &[
            "link",
            "ada",
            "--issuer",
            "https://idp.example",
            "--subject",
            "sub-1",
        ],
        None,
    );
    assert!(err.contains("already linked"), "{err}");
    assert!(err.contains("ada"), "{err}");

    // One identity per provider per account: the stale one goes first, which
    // is the re-registration repair.
    let err = users_err(
        home.path(),
        &[
            "link",
            "ada",
            "--issuer",
            "https://idp.example",
            "--subject",
            "sub-2",
        ],
        None,
    );
    assert!(err.contains("already holds"), "{err}");

    let out = users_ok(
        home.path(),
        &["unlink", "ada", "--issuer", "https://idp.example"],
        None,
    );
    assert!(out.contains("ada"), "{out}");
    let err = users_err(
        home.path(),
        &["unlink", "ada", "--issuer", "https://idp.example"],
        None,
    );
    assert!(err.contains("no identity"), "{err}");

    // And the repair completes: the new subject links once the old one is gone.
    users_ok(
        home.path(),
        &[
            "link",
            "ada",
            "--issuer",
            "https://idp.example",
            "--subject",
            "sub-2",
        ],
        None,
    );
}

/// Linking to a name that is nobody is refused rather than creating anything:
/// the account is what the identity is tied to, so it has to exist first.
#[test]
fn users_link_refuses_a_name_that_is_nobody() {
    let home = tempfile::tempdir().unwrap();
    let err = users_err(
        home.path(),
        &[
            "link",
            "ghost",
            "--issuer",
            "https://idp.example",
            "--subject",
            "sub-1",
        ],
        None,
    );
    assert!(err.contains("no such user"), "{err}");
}

/// The last-way-in guard, met from the command line: an account a first
/// sign-on provisioned has no password, so unlinking its only identity is
/// refused until `--force` says the operator means it - which is what makes
/// the re-registration repair possible without leaving a stranded account by
/// accident.
///
/// Unix only, and for a different reason than the cross-process test below:
/// there is no CLI verb that creates a passwordless account (`users add`
/// always sets one), so this opens the auth database in the test process to
/// create what a first sign-on would have. The store is dropped before any
/// child runs, but a second open of the same file is refused outright on
/// Windows, so the whole test stays here with its siblings.
#[cfg(unix)]
#[test]
fn users_unlink_refuses_the_last_way_in_unless_forced() {
    let home = tempfile::tempdir().unwrap();
    // Creates the database in the isolated home, which is what the direct
    // open below then finds.
    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-stdin"],
        Some("s3cret\n"),
    );
    let db = find_auth_db(home.path()).expect("the add created the auth database");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = crystalline_service::rest::AuthStore::open(&db)
            .await
            .unwrap();
        store
            .provision_linked_user(
                "https://idp.example",
                "sub-1",
                "grace",
                None,
                None,
                crystalline_service::rest::Role::Viewer,
                100,
            )
            .await
            .expect("a first sign-on provisions an account with no password");
    });

    let err = users_err(
        home.path(),
        &["unlink", "grace", "--issuer", "https://idp.example"],
        None,
    );
    assert!(err.contains("crystalline users passwd"), "{err}");

    let out = users_ok(
        home.path(),
        &[
            "unlink",
            "grace",
            "--issuer",
            "https://idp.example",
            "--force",
        ],
        None,
    );
    assert!(
        out.contains("crystalline users passwd"),
        "the forced unlink says what it left behind: {out}"
    );
}

/// The `web-auth.db` somewhere under an isolated home, whatever base-directory
/// layout this platform resolves to.
#[cfg(unix)]
fn find_auth_db(home: &std::path::Path) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(home).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = find_auth_db(&path) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|name| name == "web-auth.db") {
            return Some(path);
        }
    }
    None
}

// Everything from here to the end of the file is the cross-process test and
// its machinery, and all of it is `#[cfg(unix)]`, because the promise it pins
// is unix-only today.
//
// `AuthStore` opens `web-auth.db` with turso's experimental multiprocess WAL
// so that two processes can hold it at once. turso 0.7.2 (and 0.8.1) grants that only on
// an IO backend that reports `supports_shared_wal_coordination`, and on
// Windows the default backend (`WindowsIO`) does not: the trait default is
// `false` and only `WindowsIOCP`, behind the off-by-default
// `experimental_win_iocp` cargo feature, overrides it. So the multiprocess
// open is refused there, `AuthStore` falls back to a legacy open, and a legacy
// open on Windows takes an exclusive one-byte lock on the file with
// `LOCKFILE_FAIL_IMMEDIATELY`, which refuses the second process outright. The
// holder below wins the race and the CLI's open dies with a locking error, so
// this test fails deterministically on Windows rather than flakily. What a
// Windows user gets instead is the message `legacy_open_error` in
// `crystalline-service`'s `rest::auth_store` writes, which names the daemon
// and the way out; that mapping is tested there, on every platform.
//
// Delete the `cfg(unix)` attributes and this comment once either unlock
// lands: turso shipping shared WAL coordination on the default Windows
// backend, or `experimental_win_iocp` shedding its experimental label so we
// can select it. Nothing else about the test needs to change.

/// Names the auth database this process must hold open, turning this test
/// binary into the stand-in for a running daemon (see [`holds_the_auth_db`]).
/// The value is the isolated home, from which the child derives every path.
#[cfg(unix)]
const HOLD_ENV: &str = "CRYSTALLINE_TEST_AUTH_HOLD";

/// The child touches this file once its `AuthStore` is open, and exits once
/// the parent creates [`STOP_FILE`] beside it.
#[cfg(unix)]
const READY_FILE: &str = "auth-hold-ready";
#[cfg(unix)]
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
#[cfg(unix)]
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
///
/// Unix-only, for the upstream reason spelled out above the constants: turso
/// 0.7.2's (and 0.8.1's) default Windows IO backend reports no shared WAL coordination, so
/// the multiprocess open is refused there and the legacy fallback's exclusive
/// file lock refuses the second process.
#[cfg(unix)]
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
        .args(["users::holds_the_auth_db", "--exact", "--nocapture"])
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

/// The MCP token verb end to end: issue, list, rotate, revoke.
///
/// The token itself is asserted on twice over - the prefix and the shape - and
/// the teaching line is asserted verbatim, because it is what tells the
/// operator where the token goes; the MCP gate's own refusal names the same
/// header, so the two must keep saying the same thing.
#[test]
fn mcp_token_is_issued_once_then_listed_rotated_and_revoked() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "editor", "--password-stdin"],
        Some("s3cret\n"),
    );

    let out = users_ok(home.path(), &["mcp-token", "ada", "--label", "ci"], None);
    let token = out
        .split_whitespace()
        .find(|word| word.starts_with("cmt_"))
        .unwrap_or_else(|| panic!("the token is printed: {out}"))
        .to_string();
    assert_eq!(
        token.len(),
        68,
        "the prefix plus 64 hex characters: {token}"
    );
    assert!(token[4..].chars().all(|c| c.is_ascii_hexdigit()), "{token}");
    assert!(
        out.contains(
            "Add it to the agent's MCP registration as header \
             'Authorization: Bearer <token>'."
        ),
        "the token is useless without knowing where it goes: {out}"
    );
    assert!(out.contains("ci"), "the label is confirmed: {out}");

    let listed = users_ok(home.path(), &["mcp-token", "ada", "--list"], None);
    assert!(listed.contains("ci"), "the one token is listed: {listed}");
    assert!(listed.contains("never"), "never presented yet: {listed}");
    assert!(
        !listed.contains(&token) && !listed.contains("cmt_"),
        "and never the token itself, which is stored only as a hash: {listed}"
    );
    let id = listed
        .lines()
        .nth(1)
        .and_then(|row| row.split_whitespace().next())
        .expect("the listing has a row")
        .to_string();

    let rotated = users_ok(home.path(), &["mcp-token", "ada", "--rotate", &id], None);
    let fresh = rotated
        .split_whitespace()
        .find(|word| word.starts_with("cmt_"))
        .unwrap_or_else(|| panic!("the replacement is printed: {rotated}"));
    assert_ne!(fresh, token, "rotation mints a new secret");
    let listed = users_ok(home.path(), &["mcp-token", "ada", "--list"], None);
    assert!(listed.contains("ci"), "keeping the label: {listed}");
    let new_id = listed
        .lines()
        .nth(1)
        .and_then(|row| row.split_whitespace().next())
        .expect("the listing has a row")
        .to_string();
    assert_ne!(new_id, id, "under a new id: {listed}");

    // The retired id is gone, and saying so is the point: a revoke that
    // silently did nothing would leave a live token behind.
    let refused = users_err(home.path(), &["mcp-token", "ada", "--revoke", &id], None);
    assert!(refused.contains("holds no MCP token"), "{refused}");

    users_ok(
        home.path(),
        &["mcp-token", "ada", "--revoke", &new_id],
        None,
    );
    let listed = users_ok(home.path(), &["mcp-token", "ada", "--list"], None);
    assert!(
        listed.contains("holds no MCP tokens"),
        "the account is back to none: {listed}"
    );
}

/// A token is minted against an account, so a mistyped name is refused rather
/// than stranding a row nothing can resolve - on every branch of the verb,
/// because "'ghsot' holds no MCP tokens" would read as a fact about an account
/// that does not exist.
#[test]
fn an_mcp_token_for_an_unknown_account_is_refused() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["mcp-token", "ghost"],
        vec!["mcp-token", "ghost", "--list"],
        vec!["mcp-token", "ghost", "--revoke", "1"],
        vec!["mcp-token", "ghost", "--rotate", "1"],
    ] {
        let err = users_err(home.path(), &args, None);
        assert!(err.contains("no such user"), "{args:?}: {err}");
    }
}

/// `--json` puts the object on stdout and the teaching line on stderr, so a
/// provisioning script can pipe stdout straight into a parser. Asserted by
/// parsing it: a stray `println!` on that path would break scripts silently.
#[test]
fn an_mcp_token_issued_with_json_leaves_stdout_parseable() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "ada", "--role", "editor", "--password-stdin"],
        Some("s3cret\n"),
    );
    let out = users_ok(
        home.path(),
        &["--json", "mcp-token", "ada", "--label", "ci"],
        None,
    );
    let issued: serde_json::Value =
        serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("stdout is JSON: {e}: {out}"));
    assert!(
        issued["token"].as_str().unwrap().starts_with("cmt_"),
        "{issued}"
    );
    assert_eq!(issued["label"], "ci");
    assert!(issued["id"].as_i64().is_some(), "{issued}");
    assert!(
        !out.contains("Authorization"),
        "the teaching line goes to stderr: {out}"
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
#[cfg(unix)]
struct Holder(std::process::Child);

#[cfg(unix)]
impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// RUST_LOG reaches the plain CLI: the credential sweep's debug line, silent
/// by default, shows on stderr when asked for, and never on stdout.
#[test]
fn rust_log_reaches_the_cli_on_stderr_only() {
    let home = tempfile::tempdir().unwrap();
    // A name the account store takes and a GitHub identity cannot address,
    // so the sweep takes its debug branch.
    users_ok(
        home.path(),
        &["add", "ann+lee", "--role", "editor", "--password-stdin"],
        Some("ann-password-1"),
    );
    users_ok(
        home.path(),
        &["add", "bob+lee", "--role", "editor", "--password-stdin"],
        Some("bob-password-1"),
    );

    let mut quiet = bin();
    isolate(&mut quiet, home.path());
    let quiet = quiet
        .env_remove("RUST_LOG")
        .args(["users", "disable", "ann+lee"])
        .output()
        .unwrap();
    assert!(quiet.status.success(), "{quiet:?}");
    assert!(
        !String::from_utf8_lossy(&quiet.stderr).contains("cannot address a credential"),
        "the default filter is warn"
    );

    let mut loud = bin();
    isolate(&mut loud, home.path());
    let loud = loud
        .env("RUST_LOG", "debug")
        .args(["users", "disable", "bob+lee"])
        .output()
        .unwrap();
    assert!(loud.status.success(), "{loud:?}");
    assert!(
        String::from_utf8_lossy(&loud.stderr).contains("cannot address a credential"),
        "{}",
        String::from_utf8_lossy(&loud.stderr)
    );
    assert!(!String::from_utf8_lossy(&loud.stdout).contains("cannot address a credential"));
}

/// The token-only account an agent runs under: no password, one personal
/// token, created in one step. Every field of the JSON object is asserted,
/// because a script reads them by name.
#[test]
fn add_with_mcp_token_creates_a_passwordless_account_and_prints_its_token() {
    let home = tempfile::tempdir().unwrap();
    let mut cmd = bin();
    isolate(&mut cmd, home.path());
    let out = cmd
        .args([
            "users",
            "add",
            "Agent-Build",
            "--role",
            "editor",
            "--mcp-token",
            "build",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let issued: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(issued["name"], "agent-build");
    assert_eq!(issued["role"], "editor");
    assert_eq!(
        issued["reflection"], "default",
        "the same account fields as `users add --json`: {issued}"
    );
    assert_eq!(issued["label"], "build");
    assert!(issued["id"].is_i64(), "{issued}");
    let token = issued["token"].as_str().unwrap();
    assert!(token.starts_with("cmt_") && token.len() == 68, "{token}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Authorization: Bearer"),
        "the teaching line stays on stderr"
    );

    let listed: serde_json::Value =
        serde_json::from_str(&users_ok(home.path(), &["--json", "list"], None)).unwrap();
    let row = &listed["users"][0];
    assert_eq!(row["name"], "agent-build");
    assert_eq!(row["has_password"], false, "{listed}");

    let text = users_ok(home.path(), &["list"], None);
    assert!(text.contains("active, no password"), "{text}");

    // A second account with a password lists as having one.
    users_ok(
        home.path(),
        &["add", "ada", "--password-stdin"],
        Some("s3cret\n"),
    );
    let listed: serde_json::Value =
        serde_json::from_str(&users_ok(home.path(), &["--json", "list"], None)).unwrap();
    assert_eq!(listed["users"][0]["name"], "ada");
    assert_eq!(listed["users"][0]["has_password"], true, "{listed}");
    assert_eq!(listed["users"][1]["has_password"], false, "{listed}");

    // The same name again is refused with the existing sentence.
    let refused = users_err(
        home.path(),
        &["add", "agent-build", "--mcp-token", "again"],
        None,
    );
    assert!(
        refused.contains("a user named 'agent-build' already exists"),
        "{refused}"
    );
}

#[test]
fn add_with_mcp_token_prints_the_human_text_and_the_token_block() {
    let home = tempfile::tempdir().unwrap();
    let out = users_ok(
        home.path(),
        &[
            "add",
            "agent-build",
            "--role",
            "editor",
            "--mcp-token",
            "build",
        ],
        None,
    );
    assert!(
        out.contains("Added user 'agent-build' with role editor and no password."),
        "{out}"
    );
    assert!(
        out.contains("for 'agent-build' (label 'build'), shown once:"),
        "{out}"
    );
    assert!(out.contains("cmt_"), "{out}");
}

#[test]
fn mcp_token_and_password_stdin_cannot_be_combined() {
    let home = tempfile::tempdir().unwrap();
    let err = users_err(
        home.path(),
        &[
            "add",
            "agent-build",
            "--mcp-token",
            "build",
            "--password-stdin",
        ],
        Some("s3cret\n"),
    );
    assert!(err.contains("cannot be used with"), "{err}");
    let listed = users_ok(home.path(), &["list"], None);
    assert!(
        listed.contains("No users yet"),
        "nothing was created: {listed}"
    );
}

/// `--password-file` reads a Docker secret or a provisioning script's file:
/// one trailing line break goes, CRLF or LF. It cannot be combined with
/// `--password-stdin`, and a file that cannot be read is named.
#[test]
fn users_add_reads_the_password_from_a_file() {
    let home = tempfile::tempdir().unwrap();
    let secret = home.path().join("admin-password.txt");
    std::fs::write(&secret, "s3cret\r\n").unwrap();
    let path = secret.to_str().unwrap();

    users_ok(
        home.path(),
        &["add", "ada", "--role", "admin", "--password-file", path],
        None,
    );
    let out = users_ok(home.path(), &["list"], None);
    assert!(out.contains("ada") && out.contains("admin"), "{out}");

    let err = users_err(
        home.path(),
        &["add", "bob", "--password-file", path, "--password-stdin"],
        Some("hunter2\n"),
    );
    assert!(err.contains("cannot be used with"), "{err}");

    let missing = home.path().join("no-such-file");
    let err = users_err(
        home.path(),
        &["add", "bob", "--password-file", missing.to_str().unwrap()],
        None,
    );
    assert!(err.contains("no-such-file"), "the path is named: {err}");

    let empty = home.path().join("empty.txt");
    std::fs::write(&empty, "\n").unwrap();
    let err = users_err(
        home.path(),
        &["add", "bob", "--password-file", empty.to_str().unwrap()],
        None,
    );
    assert!(err.contains("the password is empty"), "{err}");

    // The password is the file's content without its line ending. Read back
    // on unix only, where `find_auth_db` is compiled.
    #[cfg(unix)]
    {
        let db = find_auth_db(home.path()).expect("the accounts database exists");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = crystalline_service::rest::AuthStore::open(&db)
                .await
                .unwrap();
            assert!(
                store
                    .verify_password("ada", "s3cret")
                    .await
                    .unwrap()
                    .is_some()
            );
            assert!(
                store
                    .verify_password("ada", "s3cret\r\n")
                    .await
                    .unwrap()
                    .is_none()
            );
        });
    }
}

/// Run `crystalline users ...` with `CRYSTALLINE_CONFIG` pointing at a config
/// that registers `domains`, so the unknown-entry warning has a registry to
/// check against. Returns the whole output; the caller asserts the status.
fn users_with_registry(
    home: &std::path::Path,
    domains: &[&str],
    args: &[&str],
) -> std::process::Output {
    let config = home.join("registry.yaml");
    let mut yaml = String::from("domains:\n");
    for name in domains {
        let dir = home.join("domains").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        yaml.push_str(&format!("  {name}:\n    path: {}\n", dir.display()));
    }
    std::fs::write(&config, yaml).unwrap();
    let mut cmd = bin();
    isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_CONFIG", &config);
    cmd.arg("users").args(args);
    cmd.output().unwrap()
}

#[test]
fn users_add_with_writes_sets_the_list_for_an_agent_and_a_person() {
    let home = tempfile::tempdir().unwrap();
    let out = users_with_registry(
        home.path(),
        &["agents-bot", "shared-notes"],
        &[
            "add",
            "bot",
            "--role",
            "editor",
            "--mcp-token",
            "bot",
            "--writes",
            "agents-bot,shared-*",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Write list: agents-bot, shared-*."),
        "{stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("warning"),
        "every entry matches a registered domain"
    );
    let printed = users_ok(home.path(), &["writes", "bot"], None);
    assert!(
        printed.contains("Write list of 'bot': agents-bot, shared-*."),
        "{printed}"
    );

    users_ok(
        home.path(),
        &[
            "add",
            "ada",
            "--role",
            "editor",
            "--writes",
            "agents-ada",
            "--password-stdin",
        ],
        Some("s3cret\n"),
    );
    let printed = users_ok(home.path(), &["writes", "ada"], None);
    assert!(
        printed.contains("Write list of 'ada': agents-ada."),
        "{printed}"
    );
}

#[test]
fn users_writes_sets_clears_and_prints() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "bot", "--role", "editor", "--password-stdin"],
        Some("s3cret\n"),
    );
    let none = users_ok(home.path(), &["writes", "bot"], None);
    assert!(none.contains("'bot' has no write list."), "{none}");

    let set = users_ok(home.path(), &["writes", "Bot", "agents-bot, team-*"], None);
    assert!(
        set.contains("Set the write list of 'bot': agents-bot, team-*."),
        "{set}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&users_ok(home.path(), &["--json", "writes", "bot"], None)).unwrap();
    assert_eq!(json["name"], "bot");
    assert_eq!(json["writes"], serde_json::json!(["agents-bot", "team-*"]));

    let cleared = users_ok(home.path(), &["writes", "bot", "--clear"], None);
    assert!(
        cleared.contains("Removed the write list of 'bot'."),
        "{cleared}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&users_ok(home.path(), &["--json", "writes", "bot"], None)).unwrap();
    assert_eq!(json["writes"], serde_json::Value::Null);

    let ghost = users_err(home.path(), &["writes", "ghost", "open"], None);
    assert!(ghost.contains("no such user: 'ghost'"), "{ghost}");
    let ghost = users_err(home.path(), &["writes", "ghost"], None);
    assert!(ghost.contains("no such user: 'ghost'"), "{ghost}");
}

/// A path separator is not a domain name: the entry is refused with a plain
/// sentence and nothing is stored, the valid entry beside it included.
#[test]
fn users_writes_refuses_a_slash() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "bot", "--role", "editor", "--password-stdin"],
        Some("s3cret\n"),
    );
    let refused = users_err(home.path(), &["writes", "bot", "open,agents-bot/"], None);
    assert!(
        refused.contains(&format!(
            "'agents-bot/' is not a domain name or a name prefix with one * at the end: {}",
            crystalline_core::config::registration::DOMAIN_NAME_RULE
        )),
        "{refused}"
    );
    for bad in ["*", "a*b*", "*agents"] {
        let refused = users_err(home.path(), &["writes", "bot", bad], None);
        assert!(refused.contains("is not a domain name"), "{bad}: {refused}");
    }
    let empty = users_err(home.path(), &["writes", "bot", " , "], None);
    assert!(
        empty.contains("give at least one entry, or use --clear"),
        "{empty}"
    );
    let printed = users_ok(home.path(), &["writes", "bot"], None);
    assert!(
        printed.contains("'bot' has no write list."),
        "nothing stored: {printed}"
    );

    // `users add --writes` validates before it creates anything.
    let refused = users_err(
        home.path(),
        &[
            "add",
            "agent",
            "--mcp-token",
            "agent",
            "--writes",
            "agents/x",
        ],
        None,
    );
    assert!(refused.contains("is not a domain name"), "{refused}");
    let listed = users_ok(home.path(), &["list"], None);
    assert!(
        !listed.contains("agent "),
        "no account was created: {listed}"
    );
}

/// An entry that names no registered domain is kept with a warning (the
/// domain may be created later), and until then it matches nothing: the
/// account writes nowhere.
#[test]
fn unknown_entries_warn_and_match_nothing() {
    let home = tempfile::tempdir().unwrap();
    users_ok(
        home.path(),
        &["add", "bot", "--role", "editor", "--password-stdin"],
        Some("s3cret\n"),
    );
    let out = users_with_registry(home.path(), &["open"], &["writes", "bot", "ghost,nobody-*"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: no registered domain matches 'ghost'; the entry is kept"),
        "{stderr}"
    );
    assert!(stderr.contains("'nobody-*'"), "{stderr}");
    let printed = users_ok(home.path(), &["writes", "bot"], None);
    assert!(
        printed.contains("Write list of 'bot': ghost, nobody-*."),
        "{printed}"
    );

    // And the account writes nowhere: every registered domain is capped.
    let db = crate::common::isolated_state_dir(home.path()).join("web-auth.db");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let auth = std::sync::Arc::new(
            crystalline_service::rest::AuthStore::open(&db)
                .await
                .unwrap(),
        );
        let access = crystalline_service::DomainAccess::new(auth);
        let scope = crystalline_service::Scope::User {
            account: "bot".to_string(),
            admin: false,
        };
        assert_eq!(
            access.write_right(&scope, "open").await.unwrap(),
            crystalline_service::DomainRight::Read
        );
    });
}

#[test]
fn users_list_shows_the_write_list() {
    let home = tempfile::tempdir().unwrap();
    for name in ["ada", "bot"] {
        users_ok(
            home.path(),
            &["add", name, "--role", "editor", "--password-stdin"],
            Some("s3cret\n"),
        );
    }
    users_ok(home.path(), &["writes", "bot", "agents-bot,team-*"], None);
    let text = users_ok(home.path(), &["list"], None);
    assert!(text.contains("WRITES"), "{text}");
    let bot_line = text.lines().find(|l| l.starts_with("bot ")).unwrap();
    assert!(bot_line.contains("agents-bot,team-*"), "{bot_line}");
    let json: serde_json::Value =
        serde_json::from_str(&users_ok(home.path(), &["--json", "list"], None)).unwrap();
    assert_eq!(json["users"][0]["name"], "ada");
    assert!(
        json["users"][0].get("writes").is_none(),
        "no list, no key: {json}"
    );
    assert_eq!(
        json["users"][1]["writes"],
        serde_json::json!(["agents-bot", "team-*"])
    );
}

/// B-2: every editing form of `users writes` and `users add` answers JSON
/// with --json, in the shape `users writes <name> --json` shows.
#[test]
fn users_writes_and_add_print_json() {
    let home = tempfile::tempdir().unwrap();
    let added: serde_json::Value = serde_json::from_str(&users_ok(
        home.path(),
        &[
            "--json",
            "add",
            "bot",
            "--mcp-token",
            "bot",
            "--role",
            "editor",
            "--writes",
            "agents-bot",
        ],
        None,
    ))
    .unwrap();
    assert_eq!(added["name"], "bot");
    assert_eq!(added["writes"], serde_json::json!(["agents-bot"]));
    assert!(added["token"].as_str().is_some());

    let set: serde_json::Value = serde_json::from_str(&users_ok(
        home.path(),
        &["--json", "writes", "bot", "agents-bot,team-*"],
        None,
    ))
    .unwrap();
    assert_eq!(
        set,
        serde_json::json!({ "name": "bot", "writes": ["agents-bot", "team-*"] })
    );

    let cleared: serde_json::Value = serde_json::from_str(&users_ok(
        home.path(),
        &["--json", "writes", "bot", "--clear"],
        None,
    ))
    .unwrap();
    assert_eq!(
        cleared,
        serde_json::json!({ "name": "bot", "writes": null })
    );

    let ada: serde_json::Value = serde_json::from_str(&users_ok(
        home.path(),
        &[
            "--json",
            "add",
            "ada",
            "--role",
            "viewer",
            "--password-stdin",
        ],
        Some("pw12345678\n"),
    ))
    .unwrap();
    assert_eq!(
        ada,
        serde_json::json!({
            "name": "ada",
            "role": "viewer",
            "reflection": "default",
            "writes": null
        })
    );
}

/// B-2: a 64-character stem with `*` is a valid prefix (a 64-character
/// domain name is legal and matches it); a 65-character one is not.
#[test]
fn a_full_length_stem_with_a_star_is_accepted() {
    let home = tempfile::tempdir().unwrap();
    users_ok(home.path(), &["add", "bot", "--mcp-token", "bot"], None);
    let stem = "a".repeat(64);
    users_ok(home.path(), &["writes", "bot", &format!("{stem}*")], None);
    let long = "a".repeat(65);
    let refused = users_err(home.path(), &["writes", "bot", &format!("{long}*")], None);
    assert!(refused.contains("is not a domain name"), "{refused}");
}

/// Knowledge `none` and the Reflection level travel together: an account
/// created at `none` with a Reflection level lists both, and a member at
/// knowledge `none` with no Reflection level is refused, because it would
/// hold nothing.
#[test]
fn a_knowledge_none_account_carries_its_reflection_level() {
    let home = tempfile::tempdir().unwrap();
    let added = users_ok(
        home.path(),
        &[
            "add",
            "nora",
            "--role",
            "none",
            "--reflection",
            "contributor",
            "--password-stdin",
        ],
        Some("s3cret\n"),
    );
    assert!(
        added.contains("Added user 'nora' with role none and Reflection contributor."),
        "{added}"
    );
    let listed: serde_json::Value =
        serde_json::from_str(&users_ok(home.path(), &["list", "--json"], None)).unwrap();
    let row = &listed["users"][0];
    assert_eq!(row["name"], "nora", "{listed}");
    assert_eq!(row["role"], "none", "{listed}");
    assert_eq!(row["reflection"], "contributor", "{listed}");

    let changed = users_ok(home.path(), &["reflection", "nora", "reader"], None);
    assert!(
        changed.contains("'nora' now has Reflection reader."),
        "{changed}"
    );
    let table = users_ok(home.path(), &["list"], None);
    assert!(table.contains("REFLECTION"), "{table}");

    // A private domain to invite nora into.
    users_ok(
        home.path(),
        &["add", "ada", "--role", "editor", "--password-stdin"],
        Some("s3cret\n"),
    );
    let config = home.path().join("config.yaml");
    let config = config.to_str().unwrap();
    let domain_dir = home.path().join("kb");
    let domain = |args: &[&str]| {
        let mut cmd = bin();
        isolate(&mut cmd, home.path());
        cmd.arg("domain").args(args).arg("--config").arg(config);
        cmd.output().unwrap()
    };
    let mut init = bin();
    isolate(&mut init, home.path());
    let out = init
        .args([
            "domain",
            "init",
            domain_dir.to_str().unwrap(),
            "--name",
            "eng",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for args in [
        &["add", "eng", domain_dir.to_str().unwrap(), "--no-sync"][..],
        &["visibility", "eng", "private", "--owner", "ada"][..],
    ] {
        let out = domain(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let refused = domain(&["members", "eng", "add", "nora", "--level", "none"]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("needs a Reflection level"), "{stderr}");

    let invited = domain(&[
        "members",
        "eng",
        "add",
        "nora",
        "--level",
        "none",
        "--reflection",
        "contributor",
    ]);
    assert!(
        invited.status.success(),
        "{}",
        String::from_utf8_lossy(&invited.stderr)
    );
    let stdout = String::from_utf8_lossy(&invited.stdout);
    assert!(
        stdout.contains("'nora' is a none on domain 'eng' with Reflection contributor."),
        "{stdout}"
    );
    // Without --reflection the stored level stays.
    let relevelled = domain(&["members", "eng", "add", "nora", "--level", "viewer"]);
    let stdout = String::from_utf8_lossy(&relevelled.stdout);
    assert!(stdout.contains("with Reflection contributor"), "{stdout}");
    let listed = domain(&["members", "eng", "list"]);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("REFLECTION"), "{stdout}");
}
