//! Integration tests for `CRYSTALLINE_GITHUB_TOKEN`: a headless node's GitHub
//! identity comes from the environment rather than a saved credential, so
//! signing `connect github` in refuses to run at all while it is set - while
//! forgetting a STORED credential stays available, since that is about what is
//! saved rather than about who this machine acts as. Every test injects the
//! variable per-child with `assert_cmd`'s `.env`, never a process-global
//! `set_var`, and points every path at a tempdir.

use assert_cmd::Command;

mod common;
use common::isolate;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

// `isolate` (from `common`) redirects every base directory a child can
// resolve into `home`, so a child never reaches a real daemon socket, the
// developer's own config or a real credential store.

#[test]
fn connect_github_refuses_when_the_environment_owns_the_token() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_GITHUB_TOKEN", "y")
        .args(["connect", "github", "--token", "x", "--config"])
        .arg(&config)
        .output()
        .unwrap();

    assert!(
        !out.status.success(),
        "connect must refuse while the environment owns the token"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains(
            "this machine's GitHub identity comes from CRYSTALLINE_GITHUB_TOKEN; unset it to sign in interactively"
        ),
        "{stderr}"
    );
}

/// Forgetting a stored credential is not a sign-in and is not refused here: the
/// variable fixes which identity this machine ACTS as, while a disconnect
/// deletes what is saved, and a machine that no longer wants to hold a token it
/// has stopped using must be able to say so without unsetting its environment
/// first.
///
/// `--host` points the credential this deletes at a GitHub Enterprise Server
/// slot no real install carries, because a disconnect does reach this machine's
/// credential store: it deletes whatever is filed under the address it is
/// given, and only an address nobody connected is safe to point it at.
#[test]
fn disconnecting_a_stored_credential_works_beside_an_environment_token() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_GITHUB_TOKEN", "y")
        .args(["connect", "github", "--disconnect"])
        .args(["--host", "ghes-disconnect-probe.example", "--config"])
        .arg(&config)
        .output()
        .unwrap();

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "{stderr}");
    assert!(
        !stderr.contains("CRYSTALLINE_GITHUB_TOKEN"),
        "a disconnect is not a sign-in and must not be answered with one: {stderr}"
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("Disconnected this machine's GitHub identity."),
        "{stdout}"
    );
}
