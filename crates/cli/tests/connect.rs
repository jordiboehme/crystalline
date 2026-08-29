//! `crystalline connect github` and its personal-identity addressing.
//!
//! Nothing here signs in: a real connect ends in a keychain write, and a test
//! that reached it would write to the developer's own login keychain. What is
//! reachable without a network call or a credential store is the addressing
//! itself - which credential the flags name, and what a name that cannot
//! address one is told - plus the disconnect path, which is exercised against
//! a token file in an isolated state directory under an account name no real
//! install carries. The identity mapping itself (normalization included) is
//! pinned by `cmd::connect_identity_tests` in the binary's own unit tests.

use assert_cmd::Command;

mod common;
// Used only by the disconnect test below, which is unix-only (see there).
#[cfg(unix)]
use common::isolate;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

#[test]
fn connect_help_teaches_the_personal_flags() {
    bin()
        .args(["connect", "github", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--personal"))
        .stdout(predicates::str::contains("--as <ACCOUNT>"))
        .stdout(predicates::str::contains("bot account"))
        .stdout(predicates::str::contains("--disconnect"));
}

#[test]
fn connecting_on_behalf_of_an_account_needs_the_personal_flag() {
    let out = bin()
        .args(["connect", "github", "--as", "bot", "--token", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--personal"), "{stderr}");
}

/// The teaching refusal lands before anything is validated, which is what makes
/// it reachable at all here: a name that cannot address a credential must not
/// cost a browser sign-in first.
#[test]
fn a_name_that_cannot_address_a_credential_is_refused_before_any_sign_in() {
    let work = tempfile::tempdir().unwrap();
    let out = bin()
        .args(["connect", "github", "--personal", "--as", "Ann+Lee"])
        .args(["--token", "not-used", "--config"])
        .arg(work.path().join("config.yaml"))
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("Ann+Lee"), "{stderr}");
    assert!(
        stderr.contains("lowercase letters, digits"),
        "the CLI teaches the class a name may be drawn from: {stderr}"
    );
}

#[test]
fn a_disconnect_cannot_also_carry_a_token() {
    let out = bin()
        .args(["connect", "github", "--disconnect", "--token", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--token"), "{stderr}");
}

/// Disconnecting a personal identity forgets its credential file. The account
/// name is one no real install carries, because this does reach the machine's
/// credential store: it deletes whatever is filed under that name, and only a
/// name nobody connected is safe to point it at.
#[test]
#[cfg(unix)]
fn disconnecting_a_personal_identity_forgets_its_credential() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let tokens = home.path().join("state/crystalline/origins");
    std::fs::create_dir_all(&tokens).unwrap();
    let token_file = tokens.join("github-token-personal-cli-forget-probe.json");
    std::fs::write(
        &token_file,
        r#"{"access_token":"gho_x","host":"github.com","user":"probe","created_at":"2026-08-29T00:00:00Z"}"#,
    )
    .unwrap();

    let mut cmd = bin();
    isolate(&mut cmd, home.path());
    cmd.args([
        "connect",
        "github",
        "--personal",
        "--as",
        "cli-forget-probe",
    ])
    .args(["--disconnect", "--config"])
    .arg(work.path().join("config.yaml"))
    .assert()
    .success()
    .stdout(predicates::str::contains("cli-forget-probe"));

    assert!(
        !token_file.exists(),
        "the personal credential file must be gone: {}",
        token_file.display()
    );
}
