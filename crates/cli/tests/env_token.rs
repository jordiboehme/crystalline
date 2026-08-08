//! Integration tests for `CRYSTALLINE_GITHUB_TOKEN`: a headless node's GitHub
//! identity comes from the environment rather than a saved credential, so
//! `connect github` refuses to run at all while it is set. Every test injects
//! the variable per-child with `assert_cmd`'s `.env`, never a process-global
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
