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
const NUDGE_REASON: &str = "Review this conversation for durable learnings before finishing: new facts, decisions, patterns and antipatterns, gotchas, corrections from the user or researched answers worth keeping. Corrections include ones that make an existing engram wrong - for those propose the reconciling edit or supersession, not a new capture beside the old. If any are not yet captured, propose capturing each one as an engram into the fitting crystalline domain: name the insight, the domain and the folder when one fits and wait for a yes. If a recalled engram proved to be the key to the task, raise its salience. If nothing qualifies or everything is already captured, finish normally without mentioning this check.";

/// The ride-along maintenance paragraph, duplicated here for the same reason
/// [`NUDGE_REASON`] is: this is a black-box check on what the subprocess
/// printed.
const EVOLVE_NUDGE_REASON: &str = "Also due now: knowledge maintenance. Call the crystalline evolve_engrams tool and work the queue it returns: apply mechanical findings directly and summarize once at the end; propose judgment findings one at a time and wait for a yes. Engrams captured by a person are judgment class - never rewrite a human's words without asking.";

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
    write_domains_config(path, &["test"]);
}

/// The same, for a test that needs the registered names to be particular ones:
/// the maintenance ask names only domains this install still registers, so a
/// test about a pending domain has to register the domain it puts on the
/// backlog.
fn write_domains_config(path: &Path, names: &[&str]) {
    let mut yaml = String::from("domains:\n");
    for name in names {
        yaml.push_str(&format!(
            "  {name}:\n    path: /nonexistent/{name}-domain\n"
        ));
    }
    std::fs::write(path, yaml).unwrap();
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

/// The per-machine maintenance throttle record under an isolated `home`, the
/// file `crystalline-service` writes and this hook reads from another
/// process.
fn maintenance_path(home: &Path) -> PathBuf {
    state_hooks_dir(home).join("maintenance.json")
}

/// Install a maintenance state under `home`, creating the hooks folder.
fn write_maintenance(home: &Path, state: serde_json::Value) {
    let path = maintenance_path(home);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
}

fn read_maintenance(home: &Path) -> serde_json::Value {
    let bytes = std::fs::read(maintenance_path(home)).expect("the maintenance state exists");
    serde_json::from_slice(&bytes).unwrap()
}

/// An RFC 3339 stamp `days` ago, for a maintenance state whose arms must mean
/// the same thing whenever the suite runs: a literal date would drift past the
/// weekly interval and silently change which arm a test exercises.
fn stamp(days: i64) -> String {
    (chrono::Utc::now() - chrono::TimeDelta::days(days)).to_rfc3339()
}

/// Backdate a file's modification time past the sweep's one-week cutoff.
fn backdate(path: &Path) {
    let stale = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(8 * 24 * 60 * 60))
        .unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(stale).unwrap();
}

/// The clock behind the fresh-install quiet week starts on the first Stop
/// hook this machine ever runs, including one that says nothing.
#[test]
fn the_first_silent_call_seeds_the_maintenance_clock() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    // No transcript: the fallback counter keeps this first call silent.
    let payload = stop_payload("session-seed", None);

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

    let state = read_maintenance(&home);
    assert!(
        state["first_seen"].is_string(),
        "a silent call still starts the clock: {state}"
    );
    // Whether a silent call can stamp an ask is pinned by the test below, on
    // a state that is actually due; a fresh state is not due at all, so
    // asserting it here would hold with or without the ride-along gate.
}

/// The ride-along contract: an overdue backlog is never a reason of its own to
/// speak. With the ask clearly due but the capture nudge not firing, the hook
/// must stay silent and must not burn the 24 hour cooldown on a session that
/// said nothing - otherwise the next session that does earn a nudge carries no
/// maintenance paragraph.
#[test]
fn a_due_ask_stays_silent_when_the_capture_nudge_does_not_fire() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    // Registered, so the pending arm below is genuinely armed and the silence
    // this asserts comes from the ride-along gate rather than from the domain
    // being unreachable.
    write_domains_config(&config, &["playground"]);
    write_maintenance(
        &home,
        serde_json::json!({
            "v": 1,
            // Due twice over: a pending domain and a sweep far past the week.
            "pending_domains": ["playground"],
            "pending_since": "2026-06-01T09:00:00Z",
            "last_run_at": "2026-06-01T09:00:00Z",
            "last_nudge_at": null,
            "first_seen": "2026-05-01T09:00:00Z",
        }),
    );
    // No transcript: the fallback counter keeps this first call silent, so the
    // capture nudge never fires and the ask must not ride along.
    let payload = stop_payload("session-gate", None);

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
        "a due backlog must never speak on its own: {:?}",
        out.stdout
    );

    let state = read_maintenance(&home);
    assert!(
        state["last_nudge_at"].is_null(),
        "a silent call never stamps an ask it did not make: {state}"
    );
}

/// A domain a human wrote to arms the ask, and the emitted reason carries the
/// capture nudge, the maintenance paragraph and the focus domains in one
/// string. Firing stamps `last_nudge_at`, which is what the 24 hour cooldown
/// reads next time.
#[test]
fn the_ride_along_ask_names_the_pending_domains_and_stamps_the_nudge() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    // The pending domain below has to be one this install registers: a name it
    // does not is a ghost the ask never speaks about, which the test after this
    // one pins.
    write_domains_config(&config, &["playground"]);
    let transcript = substantial_transcript(work.path());
    write_maintenance(
        &home,
        serde_json::json!({
            "v": 1,
            "pending_domains": ["playground"],
            "pending_since": "2026-08-15T09:00:00Z",
            // Recent enough that the weekly arm is not what fires here.
            "last_run_at": "2026-08-16T09:00:00Z",
            "last_nudge_at": null,
            "first_seen": "2026-07-01T09:00:00Z",
        }),
    );

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(stop_payload("session-ride-along", Some(&transcript)))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let decision: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(decision["decision"], "block");
    assert_eq!(
        decision["reason"],
        serde_json::Value::String(format!(
            "{NUDGE_REASON} {EVOLVE_NUDGE_REASON} Focus domains: playground."
        ))
    );

    let state = read_maintenance(&home);
    assert!(
        state["last_nudge_at"].is_string(),
        "the ask stamps the cooldown before printing: {state}"
    );
    assert_eq!(
        state["pending_domains"],
        serde_json::json!(["playground"]),
        "the hook never clears the backlog - only a sweep does"
    );
}

/// A domain that went pending and was later unregistered is a ghost: no sweep
/// can reach it, so the ask must neither arm on it nor name it. The session
/// still earns its capture nudge, the maintenance paragraph stays away, and the
/// 24 hour cooldown is never burnt on an ask nobody could act on.
#[test]
fn a_pending_ghost_domain_neither_arms_the_ask_nor_is_named() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    // Registers exactly one domain, `test`; the backlog below names another.
    write_domain_config(&config);
    let transcript = substantial_transcript(work.path());
    write_maintenance(
        &home,
        serde_json::json!({
            "v": 1,
            "pending_domains": ["ghost"],
            "pending_since": stamp(2),
            // Recent, so the weekly arm is not what could fire here: the
            // pending arm is the one under test. Relative to the clock rather
            // than a literal date, so the week never quietly expires on this
            // test.
            "last_run_at": stamp(1),
            "last_nudge_at": null,
            "first_seen": stamp(60),
        }),
    );

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(stop_payload("session-ghost", Some(&transcript)))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let decision: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        decision["reason"],
        serde_json::Value::String(NUDGE_REASON.to_string()),
        "an unregistered domain must never pull the maintenance paragraph in"
    );

    let state = read_maintenance(&home);
    assert!(
        state["last_nudge_at"].is_null(),
        "no ask was made, so no cooldown was burnt: {state}"
    );
    assert_eq!(
        state["pending_domains"],
        serde_json::json!(["ghost"]),
        "the hook never edits the backlog - a full sweep is what clears a ghost"
    );
}

/// The stale sweep removes week-old session files, but the maintenance
/// record is per-machine and long-lived: a quiet week must not erase the
/// throttle and re-arm the fresh-install grace period.
#[test]
fn the_stale_sweep_spares_the_maintenance_file() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    // `first_seen` already set, and a silent call: nothing rewrites the file
    // during this run, so its backdated mtime is what the sweep sees.
    write_maintenance(
        &home,
        serde_json::json!({
            "v": 1,
            "pending_domains": [],
            "pending_since": null,
            "last_run_at": "2026-08-16T09:00:00Z",
            "last_nudge_at": null,
            "first_seen": "2026-07-01T09:00:00Z",
        }),
    );
    backdate(&maintenance_path(&home));
    let stale_session = state_hooks_dir(&home).join("old-session.json");
    std::fs::write(&stale_session, b"{}").unwrap();
    backdate(&stale_session);

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(stop_payload("session-sweep", None))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);

    assert!(
        !stale_session.exists(),
        "a week-old session file is still swept"
    );
    assert!(
        maintenance_path(&home).exists(),
        "the maintenance throttle record must survive a quiet week"
    );
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
