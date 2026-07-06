//! Integration tests for `crystalline hook stop`, spawning the real
//! `crystalline` binary. Every scenario needs control over the state
//! directory (`<state_dir>/hooks/<session_id>.json`), reachable only through
//! `HOME`/`XDG_*` and never a CLI flag - the same isolation technique
//! `crates/cli/tests/configure.rs` uses for its environment-driven tests,
//! applied here because `hook stop` itself takes no `--config` flag: the
//! config path comes from `CRYSTALLINE_CONFIG` or the default, set per child
//! with `assert_cmd`'s `.env`, never a process-global `std::env::set_var`.
//! Unix-only: `etcetera`'s base-directory resolution on Windows does not
//! honor these variables the way the XDG strategy the isolation relies on
//! does.
#![cfg(unix)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// The exact reminder text `hook.rs` prints, duplicated here because the
/// `crystalline` binary has no library target for a test to import it from;
/// this is a black-box check on what the subprocess actually printed.
const NUDGE_REASON: &str = "Review this conversation for durable learnings before finishing: new facts, decisions, patterns and antipatterns, gotchas, corrections from the user or researched answers worth keeping. If any are not yet captured, propose capturing each one as an engram into the fitting crystalline domain: name the insight and the domain and wait for a yes. If nothing qualifies or everything is already captured, finish normally without mentioning this check.";

fn bin() -> Command {
    Command::cargo_bin("crystalline").unwrap()
}

/// Redirect `HOME` and the XDG base directories into `home`, so the state
/// directory `hook stop` reads and writes never touches a real machine.
fn isolate(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"));
}

/// A minimal config file registering one domain, enough to make the hook's
/// `has_domains` check true. The domain's path is never read by the hook, so
/// it does not need to exist on disk.
fn write_domain_config(path: &Path) {
    std::fs::write(
        path,
        "domains:\n  test:\n    path: /nonexistent/test-domain\n",
    )
    .unwrap();
}

/// A transcript with 25 short lines: well under the byte threshold but over
/// the line-count one, so it reads as substantial. The same shape the plan's
/// manual verification fixture uses.
fn substantial_transcript(dir: &Path) -> PathBuf {
    let path = dir.join("transcript.jsonl");
    let mut content = String::new();
    for i in 0..25 {
        content.push_str(&format!("{{\"turn\":{i}}}\n"));
    }
    std::fs::write(&path, content).unwrap();
    path
}

/// A Stop hook stdin payload, `transcript_path` rendered as an explicit
/// string or `null`.
fn stop_payload(session_id: &str, transcript_path: Option<&Path>) -> String {
    serde_json::json!({
        "session_id": session_id,
        "transcript_path": transcript_path.map(|p| p.display().to_string()),
        "hook_event_name": "Stop",
    })
    .to_string()
}

/// Where `hook stop` writes session state under an isolated `home`.
fn state_hooks_dir(home: &Path) -> PathBuf {
    home.join("state").join("crystalline").join("hooks")
}

#[test]
fn fires_once_then_stays_silent_for_the_same_session() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let transcript = substantial_transcript(work.path());
    let payload = stop_payload("session-fire-once", Some(&transcript));

    let mut first = bin();
    isolate(&mut first, &home);
    let out = first
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(payload.clone())
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let decision: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(decision["decision"], "block");
    assert_eq!(decision["reason"], NUDGE_REASON);

    let mut second = bin();
    isolate(&mut second, &home);
    let out = second
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "a session already nudged must stay silent: {:?}",
        out.stdout
    );
}

#[test]
fn malformed_stdin_exits_zero_with_empty_stdout() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .args(["hook", "stop"])
        .write_stdin("this is not json")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn no_config_is_silent_even_with_a_substantial_transcript() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let transcript = substantial_transcript(work.path());
    let payload = stop_payload("session-no-config", Some(&transcript));

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    // No CRYSTALLINE_CONFIG and no file at the default path: the effective
    // config is the zero-domain default, so `has_domains` is false.
    let out = cmd
        .args(["hook", "stop"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn read_only_is_silent_even_with_a_substantial_transcript() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let transcript = substantial_transcript(work.path());
    let payload = stop_payload("session-read-only", Some(&transcript));

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .env("CRYSTALLINE_SERVICE_READ_ONLY", "true")
        .args(["hook", "stop"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
}

#[test]
fn null_transcript_path_fires_on_the_third_call() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let payload = stop_payload("session-fallback", None);

    for (call, expect_silent) in [(1, true), (2, true), (3, false)] {
        let mut cmd = bin();
        isolate(&mut cmd, &home);
        let out = cmd
            .env("CRYSTALLINE_CONFIG", &config)
            .args(["hook", "stop"])
            .write_stdin(payload.clone())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            out.stdout.is_empty(),
            expect_silent,
            "call {call}: {:?}",
            out.stdout
        );
    }
}

#[test]
fn traversal_session_id_writes_nothing_outside_the_state_hooks_dir() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let transcript = substantial_transcript(work.path());
    let payload = stop_payload("../evil", Some(&transcript));

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);

    let hooks_dir = state_hooks_dir(&home);
    assert!(
        !hooks_dir.exists() || std::fs::read_dir(&hooks_dir).unwrap().next().is_none(),
        "an invalid session id must never create a state file"
    );
    assert!(
        !home
            .join("state")
            .join("crystalline")
            .join("evil.json")
            .exists(),
        "a traversal id must never escape the hooks directory"
    );
}

#[test]
fn a_corrupt_config_file_is_silent() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    std::fs::write(&config, "domains: [not, a, map").unwrap();
    let transcript = substantial_transcript(work.path());
    let payload = stop_payload("session-corrupt-config", Some(&transcript));

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "a config that fails to load must bail silently: {:?}",
        out.stdout
    );
}

#[test]
fn an_env_defined_domain_alone_earns_the_nudge() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    std::fs::write(&config, "domains: {}\n").unwrap();
    let transcript = substantial_transcript(work.path());
    let payload = stop_payload("session-env-domain", Some(&transcript));

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .env("CRYSTALLINE_DOMAIN_TEAM", work.path().display().to_string())
        .args(["hook", "stop"])
        .write_stdin(payload)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let decision: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        decision["decision"], "block",
        "a writable node whose only domain comes from the environment is nudgeable"
    );
}
