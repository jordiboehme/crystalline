//! End-to-end tests for `crystalline users`, the account management command
//! for the web API. Every child is isolated with its own `HOME` and XDG base
//! directories, so the accounts land in a temp state directory rather than in
//! the developer's own `web-auth.db`.

use assert_cmd::Command;

mod common;
use common::isolate;
// Used only by the cross-process test below, which is unix-only (see there).
#[cfg(unix)]
use common::isolation_env;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
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
// so that two processes can hold it at once. turso 0.7.2 grants that only on
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
/// 0.7.2's default Windows IO backend reports no shared WAL coordination, so
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
