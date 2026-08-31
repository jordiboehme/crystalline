//! `crystalline connect github` and its personal-identity addressing.
//!
//! Most of what is reachable here without a credential store is the addressing
//! itself - which credential the flags name, and what a name that cannot
//! address one is told - plus the disconnect path, exercised against a token
//! file in an isolated state directory under an account name no real install
//! carries. The identity mapping itself (normalization included) is pinned by
//! `cmd::connect_identity_tests` in the binary's own unit tests.
//!
//! One test does run a connect all the way through, and two seams are what
//! make that safe. `CRYSTALLINE_TEST_TOKEN_STORE_DIR` points the credential at
//! a file under a tempdir, so nothing here writes to - or prompts - the
//! developer's own login keychain; and `github.api_url` in the config points
//! the token validation at a listener this file starts, so nothing dials
//! github.com. Both are the only form such a seam can take for these tests:
//! the binary runs as a child process, where an in-process override cannot
//! reach it.

use std::io::{Read, Write};
use std::net::TcpListener;

use assert_cmd::Command;

mod common;
use common::isolate;

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// A stand-in for the forge, answering `GET /user` as `login` for as long as
/// the test holds it. Only that one endpoint, because only that one is called:
/// a token connect validates the token and does nothing else over the wire.
struct FakeForge {
    url: String,
    _thread: std::thread::JoinHandle<()>,
}

impl FakeForge {
    fn start(login: &str) -> FakeForge {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body = format!("{{\"login\":\"{login}\"}}");
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                // Enough of the request to know it arrived; the answer is the
                // same whatever it asked for.
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeForge {
            url,
            _thread: thread,
        }
    }
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

/// A personal connect, end to end through the real binary: the token is
/// validated against the stand-in forge, the credential lands where the seam
/// points and the printed line names the identity, the login and the store.
/// Then the disconnect undoes it.
///
/// This is the one test that runs the whole verb. What it is here to catch is
/// the wiring between the parts - the flags resolving to the owner identity,
/// the validated login reaching the stored token, the store the write chose
/// being the one the line names - none of which any unit test sees end to end.
#[test]
fn a_personal_connect_stores_the_owner_credential_and_disconnects_it() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let tokens = work.path().join("tokens");
    std::fs::create_dir_all(&tokens).unwrap();

    let forge = FakeForge::start("probe");
    let config = work.path().join("config.yaml");
    std::fs::write(
        &config,
        format!(
            "github:\n  enabled: true\n  share_identity: personal\n  api_url: \"{}\"\n",
            forge.url
        ),
    )
    .unwrap();

    let mut cmd = bin();
    isolate(&mut cmd, home.path());
    cmd.env("CRYSTALLINE_TEST_TOKEN_STORE_DIR", &tokens)
        .args(["connect", "github", "--personal"])
        .args(["--token", "gho_probe", "--config"])
        .arg(&config)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Connected your personal GitHub identity as probe",
        ))
        .stdout(predicates::str::contains("file token store"));

    // The owner's slot, under its own fixed name, beside nothing else: a
    // personal connect never touches the machine's own credential.
    let owner = tokens.join("github-token-personal-owner.json");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&owner).unwrap()).unwrap();
    assert_eq!(saved["user"], "probe", "the login the forge answered");
    assert_eq!(saved["access_token"], "gho_probe");
    assert!(
        !tokens.join("github-token.json").exists(),
        "the machine's own credential is untouched"
    );

    let mut cmd = bin();
    isolate(&mut cmd, home.path());
    cmd.env("CRYSTALLINE_TEST_TOKEN_STORE_DIR", &tokens)
        .args([
            "connect",
            "github",
            "--personal",
            "--disconnect",
            "--config",
        ])
        .arg(&config)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Disconnected your personal GitHub identity",
        ));
    assert!(!owner.exists(), "the credential is gone again");
}
