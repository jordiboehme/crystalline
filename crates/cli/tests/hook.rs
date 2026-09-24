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

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::{Value, json};

/// The exact reminder text `hook.rs` prints, duplicated here because the
/// `crystalline` binary has no library target for a test to import it from;
/// this is a black-box check on what the subprocess actually printed.
const NUDGE_REASON: &str = "Review this conversation for durable learnings before finishing - what a future session would reach for: new facts, decisions, patterns and antipatterns, gotchas, corrections from the user or researched answers. Not durable: what this session did, temporary paths and outputs, a one-off bug's error text, steps that will not recur, and anything any model already knows. Corrections that make an existing engram wrong get the reconciling edit or supersession, not a new capture beside the old. Propose capturing each as an engram in the fitting crystalline domain, naming the insight, the domain and the folder when one fits, and wait for a yes. Raise the salience of a recalled engram that proved key to the task. If nothing qualifies or everything is captured, finish normally.";

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

/// The sharing paragraph, duplicated here for the reason the two above it
/// are: this is a black-box check on what the subprocess printed.
const SHARE_NUDGE_REASON: &str = "If that work is done, propose sharing it with share_changes so the team has it and the team's archive stays current - and wait for a yes.";

/// A registered team domain under `work`, holding `files`, with an origin
/// state whose base snapshot is empty - so everything in the root reads as
/// work the team has not seen.
///
/// The state file is written by hand rather than through the library: this is
/// a black-box test of the binary, and what it pins is that the hook finds the
/// state exactly where `<state_dir>/origins/<domain>/state.json` puts it.
fn write_team_domain(work: &Path, home: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = work.join(name);
    std::fs::create_dir_all(&root).unwrap();
    for (file, body) in files {
        std::fs::write(root.join(file), body).unwrap();
    }
    let origin_dir = home
        .join("state")
        .join("crystalline")
        .join("origins")
        .join(name);
    std::fs::create_dir_all(&origin_dir).unwrap();
    std::fs::write(
        origin_dir.join("state.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "repo": "acme/kb",
            "branch": "main",
            "base_commit": "",
            "ref_etag": null,
            "last_checked": null,
            "files": {},
            "proposals": [],
            "history": [],
            "conflicts": [],
        }))
        .unwrap(),
    )
    .unwrap();
    root
}

/// A session that earns the capture nudge and sits on unshared team work is
/// asked to propose it, by name and by count. Offline throughout: no forge is
/// reachable from this test, and the paragraph still arrives.
#[test]
fn the_nudge_asks_for_work_a_team_domain_has_not_shared() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    let engram = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\nBody.\n";
    let root = write_team_domain(
        work.path(),
        &home,
        "eng",
        &[
            ("alpha.md", engram),
            ("beta.md", engram),
            // A regenerated listing is never a reason to share, so it is
            // never counted.
            ("index.md", "# eng\n\n- alpha\n"),
        ],
    );
    std::fs::write(
        &config,
        format!(
            "domains:\n  eng:\n    path: {}\n    origin:\n      repo: acme/kb\n",
            root.display()
        ),
    )
    .unwrap();
    let transcript = substantial_transcript(work.path());

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(stop_payload("session-share", Some(&transcript)))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let decision: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        decision["reason"],
        serde_json::Value::String(format!(
            "{NUDGE_REASON} Also: 2 changes in the team domain eng are not yet shared. {SHARE_NUDGE_REASON}"
        )),
        "the listing is left out and the one domain is named"
    );
}

/// A local domain holds no unshared work, whatever is written into it: there
/// is nowhere to share it to, so the nudge says nothing about sharing.
#[test]
fn the_nudge_says_nothing_about_sharing_without_a_team_domain() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let transcript = substantial_transcript(work.path());

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop"])
        .write_stdin(stop_payload("session-no-share", Some(&transcript)))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let decision: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(
        decision["reason"],
        serde_json::Value::String(NUDGE_REASON.to_string()),
        "the capture nudge alone, byte for byte"
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

/// The exact sentence Claude Code renders as `Stop says: <text>`, duplicated
/// here for the reason [`NUDGE_REASON`] is: this is a black-box check on what
/// the subprocess printed.
const STOP_SYSTEM_MESSAGE: &str =
    "Crystalline is checking this session for anything worth keeping.";

/// A Stop hook wired for Claude Code answers with the top-level pair that
/// delivers the nudge and the plain sentence a person reads beside the
/// harness's own unconditional error line - and nothing else. No
/// `hookSpecificOutput` block: an older Claude Code's schema rejects the
/// whole answer over it, and a current one only logs its keys as
/// unrecognized.
#[test]
fn the_claude_code_nudge_carries_the_message_and_no_hook_specific_output() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let transcript = substantial_transcript(work.path());

    let mut cmd = bin();
    isolate(&mut cmd, &home);
    let out = cmd
        .env("CRYSTALLINE_CONFIG", &config)
        .args(["hook", "stop", "--harness", "claude-code"])
        .write_stdin(stop_payload("session-claude-code", Some(&transcript)))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let printed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(printed["decision"], "block");
    assert_eq!(printed["reason"], NUDGE_REASON);
    assert_eq!(printed["systemMessage"], STOP_SYSTEM_MESSAGE);
    assert!(
        printed.get("hookSpecificOutput").is_none(),
        "no hookSpecificOutput block: {printed}"
    );
}

/// Every harness whose Stop parser nobody here has measured gets byte-for-byte
/// the line it has always got: no flag at all (an install written before the
/// flag existed), a harness that is not Claude Code, and an id this binary does
/// not know (a hook a newer release wired up, run by an older binary). All
/// three answer identically, and a hand invocation keeps working unchanged.
#[test]
fn an_unmeasured_or_unknown_harness_gets_the_line_it_always_got() {
    let expected = serde_json::to_string(&serde_json::json!({
        "decision": "block",
        "reason": NUDGE_REASON,
    }))
    .unwrap();

    for (session, flag) in [
        ("session-no-flag", None),
        ("session-codex", Some("codex")),
        ("session-unknown", Some("a-harness-from-a-future-release")),
    ] {
        let work = tempfile::tempdir().unwrap();
        let home = work.path().join("home");
        let config = work.path().join("config.yaml");
        write_domain_config(&config);
        let transcript = substantial_transcript(work.path());

        let mut args = vec!["hook", "stop"];
        if let Some(flag) = flag {
            args.extend_from_slice(&["--harness", flag]);
        }
        let mut cmd = bin();
        isolate(&mut cmd, &home);
        let out = cmd
            .env("CRYSTALLINE_CONFIG", &config)
            .args(&args)
            .write_stdin(stop_payload(session, Some(&transcript)))
            .output()
            .unwrap();
        assert!(out.status.success(), "{flag:?} must still exit 0");
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            expected,
            "{flag:?} must get exactly the fields it always got"
        );
    }
}

/// **The MCP write receipts ask for a share in exactly these words.**
///
/// An agent over MCP never meets a Stop hook, so `crystalline_service::nudge`
/// carries the same sharing ask on its write receipts. `crates/service` sits
/// below `crates/cli` and cannot import this constant, so the sentence is
/// copied there - and a copy nobody pins is two dialects of one ask waiting to
/// happen.
///
/// The pin reads this crate's own source rather than a third copy of the
/// sentence: a literal repeated here would drift together with the one it is
/// supposed to catch. Unlike the black-box constants above, that is exactly the
/// right instrument for this claim - the question is not what the subprocess
/// printed but whether two constants in two crates are the same bytes.
#[test]
fn the_mcp_share_nudge_mirrors_the_hook_byte_for_byte() {
    const DECL: &str = "pub const SHARE_NUDGE_REASON: &str = \"";
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("hook.rs"),
    )
    .expect("the hook's own source");
    assert_eq!(
        source.matches(DECL).count(),
        1,
        "exactly one declaration of SHARE_NUDGE_REASON is scannable; \
         teach this test the new shape before changing it"
    );
    let literal = source
        .split_once(DECL)
        .expect("the declaration")
        .1
        .split_once("\";")
        .expect("the literal ends on the same line it starts")
        .0;
    assert!(
        !literal.is_empty() && !literal.contains('\n') && !literal.contains('\\'),
        "the scanner only reads a plain one-line literal, and read this instead: {literal:?}"
    );
    assert_eq!(
        crystalline_service::nudge::MCP_SHARE_NUDGE_REASON,
        literal,
        "the MCP receipt's sharing ask and the Stop hook's are one sentence; \
         change both or neither"
    );
}

// --- `crystalline hook prompt`: the per-prompt recall -----------------------

/// The exact first line of the block `recall.rs` prints, duplicated here for
/// the reason [`NUDGE_REASON`] is duplicated: this is a black-box check on
/// what the subprocess actually printed.
const RECALL_HEADER: &str = "Knowledge that may apply, from your Crystalline domains. Read an engram with read_engram (pass the crystalline:// address as identifier) before relying on it; ignore what does not fit.";

/// A `UserPromptSubmit` payload as Claude Code sends it.
fn prompt_payload(session_id: &str, prompt: &str) -> String {
    json!({
        "session_id": session_id,
        "prompt": prompt,
        "hook_event_name": "UserPromptSubmit",
    })
    .to_string()
}

/// Run `hook prompt` under an isolated `home` with an explicit config, and
/// return the child's output beside how long the whole spawn took.
fn run_prompt_hook(
    home: &Path,
    config: Option<&Path>,
    payload: &str,
) -> (std::process::Output, Duration) {
    let mut cmd = bin();
    isolate(&mut cmd, home);
    if let Some(config) = config {
        cmd.env("CRYSTALLINE_CONFIG", config);
    }
    let start = Instant::now();
    let out = cmd
        .args(["hook", "prompt", "--harness", "claude-code"])
        .write_stdin(payload.to_string())
        .output()
        .unwrap();
    (out, start.elapsed())
}

/// Exit 0, nothing on stdout and nothing on stderr: the only shape a
/// lifecycle hook may ever have when it has nothing to say.
fn assert_silent(out: &std::process::Output, case: &str) {
    assert!(
        out.status.success(),
        "{case}: a bail must exit 0, got {:?}",
        out.status.code()
    );
    assert!(
        out.stdout.is_empty(),
        "{case}: a bail must print nothing: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "{case}: a bail must log nothing: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Neither an unreadable payload nor a payload for another lifecycle event
/// earns a word. The event-name bail is defensive: a harness that ever wired
/// this command to its Stop event must get silence, not a block.
#[test]
fn prompt_hook_is_silent_on_malformed_stdin_and_on_a_stop_event_name() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);

    let (out, _) = run_prompt_hook(&home, Some(&config), "{not json at all");
    assert_silent(&out, "malformed stdin");

    let stop_shaped = json!({
        "session_id": "session-wrong-event",
        "prompt": "how does the vent driver retry",
        "hook_event_name": "Stop",
    })
    .to_string();
    let (out, _) = run_prompt_hook(&home, Some(&config), &stop_shaped);
    assert_silent(&out, "a Stop event name");
}

/// The gate decides before anything is loaded or connected: a typed slash
/// command and a prompt that carries no subject never become a search, and
/// neither leaves a state file behind.
#[test]
fn prompt_hook_is_silent_on_a_slash_command_and_a_short_prompt() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);

    for (session, prompt) in [
        ("session-slash", "/clear"),
        ("session-yes", "yes"),
        // Three words and thirteen characters: over the word bar, under the
        // character one.
        ("session-short", "run the tests"),
    ] {
        let (out, _) = run_prompt_hook(&home, Some(&config), &prompt_payload(session, prompt));
        assert_silent(&out, prompt);
        let state = state_hooks_dir(&home).join(format!("{session}.json"));
        assert!(
            !state.exists(),
            "a gated prompt must not write state: {}",
            state.display()
        );
    }
}

/// Two config bails: nothing registered to recall from, and an install that
/// turned the per-prompt recall off.
#[test]
fn prompt_hook_is_silent_without_a_config_or_with_recall_disabled() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let payload = prompt_payload("session-no-config", "how does the vent driver retry");

    // No CRYSTALLINE_CONFIG and no file at the default path: the effective
    // config registers no domain, so there is nothing to recall from.
    let (out, _) = run_prompt_hook(&home, None, &payload);
    assert_silent(&out, "no config");

    // A domain registered and the feature turned off: the ruling default is
    // on, so silence here can only come from the setting.
    let config = work.path().join("config.yaml");
    write_domain_config(&config);
    let mut yaml = std::fs::read_to_string(&config).unwrap();
    yaml.push_str("recall:\n  enabled: false\n");
    std::fs::write(&config, yaml).unwrap();
    let (out, _) = run_prompt_hook(
        &home,
        Some(&config),
        &prompt_payload("session-recall-off", "how does the vent driver retry"),
    );
    assert_silent(&out, "recall.enabled false");
    assert!(
        !state_hooks_dir(&home)
            .join("session-recall-off.json")
            .exists(),
        "a disabled recall must not write state"
    );
}

/// No daemon is silence, and cheap silence: the attach is passive, so it
/// never spawns one, never opens the index and never makes the person wait.
#[test]
fn prompt_hook_is_silent_and_fast_without_a_daemon() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);

    let (out, elapsed) = run_prompt_hook(
        &home,
        Some(&config),
        // Forty characters, well over both bars.
        &prompt_payload(
            "session-no-daemon",
            "how does the vent driver retry a write",
        ),
    );
    assert_silent(&out, "no daemon");
    // The handler's own budget is one second; what this measures is the
    // budget plus process start, and on a loaded machine the process start
    // dominates.
    assert!(
        elapsed < Duration::from_millis(3_000),
        "a daemonless prompt hook must not make the person wait: {elapsed:?}"
    );
}

/// A session id that tries to climb out of the hooks directory writes
/// nothing, anywhere: the same guard `hook stop` carries, on the handler that
/// now also writes a list of addresses.
#[test]
fn prompt_hook_never_writes_outside_the_hooks_dir_for_a_traversal_id() {
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let config = work.path().join("config.yaml");
    write_domain_config(&config);

    let (out, _) = run_prompt_hook(
        &home,
        Some(&config),
        &prompt_payload("../evil", "how does the vent driver retry"),
    );
    assert_silent(&out, "a traversal session id");

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

// --- the hook against a real daemon ----------------------------------------

/// How wide the stub's vectors are. Thirty-two buckets keep a prompt that
/// shares words with an engram close to it and a prompt that shares none far
/// from it, which is the whole geometry these tests need.
const STUB_DIMS: usize = 32;

/// The model id the stub answers for, and the one the daemon's config names.
const STUB_MODEL: &str = "bag-32";

/// FNV-1a over the bytes of one lowercased word. Spelled out rather than
/// taken from the standard library's hasher so the bucket a word lands in is
/// a fixed property of this file: the fixtures below were chosen against
/// exactly these numbers.
fn stub_hash(word: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in word.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A deterministic bag-of-words vector: every alphanumeric run, lowercased,
/// adds 1.0 at its bucket, and the vector is L2-normalised. Texts that share
/// words are close, texts that share none are not.
fn stub_vector(text: &str) -> Vec<f32> {
    let mut v = vec![0.0_f32; STUB_DIMS];
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let bucket = (stub_hash(&word.to_lowercase()) % STUB_DIMS as u64) as usize;
        v[bucket] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// A running embeddings stub. Stops its listener on drop, and counts the
/// requests it answered so a test can say how many searches a prompt cost.
struct StubHandle {
    stop: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StubHandle {
    /// How many embedding requests the stub has answered so far. After the
    /// backlog is drained this only moves for a query embedding, which is
    /// exactly one per search the hook runs.
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Drop for StubHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An HTTP/1.1 responder on an ephemeral localhost port answering POST
/// `/embeddings` the OpenAI-compatible way: `{model, input: [texts]}` in,
/// `data[].embedding` ordered by `data[].index` out. Returns its base URL and
/// the handle that stops it.
fn spawn_embeddings_stub() -> (String, StubHandle) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let thread = {
        let stop = Arc::clone(&stop);
        let calls = Arc::clone(&calls);
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // One thread per connection: the daemon embeds a
                        // backlog on one pooled connection while a search
                        // embeds its query on another, and a single-threaded
                        // responder would make the second wait on the first
                        // going idle.
                        let calls = Arc::clone(&calls);
                        std::thread::spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            serve_embeddings(stream, &calls);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        })
    };
    (
        format!("http://{addr}"),
        StubHandle {
            stop,
            calls,
            thread: Some(thread),
        },
    )
}

/// Answer every request on one keep-alive connection until the peer closes it.
fn serve_embeddings(mut stream: std::net::TcpStream, calls: &AtomicUsize) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        while let Some(body) = take_request(&mut buf) {
            calls.fetch_add(1, Ordering::SeqCst);
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let inputs: Vec<String> = request["input"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    json!({
                        "object": "embedding",
                        "index": index,
                        "embedding": stub_vector(text),
                    })
                })
                .collect();
            let payload =
                json!({ "object": "list", "model": STUB_MODEL, "data": data }).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
                payload.len()
            );
            if stream.write_all(response.as_bytes()).is_err() {
                return;
            }
            let _ = stream.flush();
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Take one complete request's body off the buffer, or `None` while the
/// request is still arriving.
fn take_request(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let len: usize = head
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .map(|v| v.trim().to_string())
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if buf.len() < head_end + len {
        return None;
    }
    let body = buf[head_end..head_end + len].to_vec();
    buf.drain(..head_end + len);
    Some(body)
}

/// The domain the daemon tests teach: one engram whose every word the
/// positive prompt names, so the lexical half of hybrid ranking matches it on
/// its own and the block never depends on the stub's geometry alone.
///
/// The wording is chosen against the stub's exact bag-of-words geometry, not
/// left to read naturally: `PLAIN_PROMPT` reaches the engram on the lexical
/// half alone (a sole, fully-matched lexical candidate scores `1.0`, scaled by
/// `SINGLE_SOURCE_PENALTY` to `0.85`), but `MESSAGE_PROMPT` never does - its
/// wrapper tokens (`cross`, `session`, `message`, `from`, `x`) keep the
/// AND-matched lexical half empty, so it reaches the engram through cosine
/// similarity alone and must clear `DEFAULT_MIN_SIMILARITY`
/// (`crates/index/src/turso/search.rs`, re-derived to 0.78 for the granite
/// model) before the floor even gets to weigh it. The body is mostly the
/// positive prompt's own words, repeated across short sentences, because the
/// stub is a literal word-occurrence count: repetition is what moves cosine.
///
/// Cosines against this fixture, recomputed with a Python reimplementation of
/// `stub_hash`/`stub_vector` (FNV-1a over lowercased alphanumeric runs, one
/// 32-dim bucket per word, L2-normalised) and confirmed by the passing tests:
///
/// | prompt | old engram (pre-granite) | new engram |
/// | --- | --- | --- |
/// | `PLAIN_PROMPT` | 0.729 | 0.876 |
/// | the negative prompt (`what colour is the sky today`) | 0.497 | 0.535 |
/// | the loop text (`check the build again and report`) | 0.397 | 0.357 |
/// | `MESSAGE_PROMPT` | 0.735 | 0.851 |
///
/// Both prompt-bearing cases now clear 0.78 with margin (`PLAIN_PROMPT` by
/// 0.096, `MESSAGE_PROMPT` by 0.071, and `MESSAGE_PROMPT` is the binding one:
/// it is the semantic-only case, scored `0.851 * 0.85 = 0.723` against the
/// `recall.min_score` floor of 0.5 pinned below); the negative prompt and the
/// loop text stay well clear of 0.78 on the other side.
const VENT_TITLE: &str = "How does the vent driver retry";
const VENT_BODY: &str = "How does the vent driver retry? How does the vent driver retry when the write fails. How does the vent driver retry when the write fails again. The vent driver retry answers how the vent driver retry works: the budget is three attempts, backing off 200 ms, then 400 ms, then 800 ms. A write that still fails does not retry again; the vent driver reports the fault and the vent stays closed until the driver retries.";
/// The address the block must name.
const VENT_ADDRESS: &str = "crystalline://vents/vent-driver-retry";

/// The daemon's config: one file domain, and an `embeddings` block naming the
/// remote provider. With an `endpoint` the daemon embeds through the stub;
/// without one the provider fails to build and the daemon serves text-only.
///
/// The endpointless arm is why there is an `embeddings` block at all: an
/// absent block does not mean "no embeddings", it means the built-in local
/// model, which a test must never reach for because building it downloads
/// one.
///
/// `recall.min_score` is pinned to the fixtures' original 0.5 rather than
/// left at the shipped default: the stub is a 32-bucket bag-of-words vector,
/// not any shipped embedding model, so its cosine geometry carries no
/// relationship to `recall.min_score`'s granite-derived default (see
/// `research/2026-09-22-granite-thresholds.md`). Pinning keeps every
/// assertion below testing what it was written to test.
fn write_daemon_config(path: &Path, domain_dir: &Path, endpoint: Option<&str>) {
    let mut yaml = format!(
        "service:\n  response_format: json\ndomains:\n  vents:\n    path: {}\nembeddings:\n  provider: openai-compatible\n  model: {STUB_MODEL}\n",
        domain_dir.display()
    );
    if let Some(endpoint) = endpoint {
        yaml.push_str(&format!("  endpoint: {endpoint}\n"));
    }
    yaml.push_str("recall:\n  min_score: 0.5\n");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, yaml).unwrap();
}

/// An isolated, short-path environment holding one daemon.
///
/// The base is `/tmp` rather than a `tempfile` directory for the reason
/// `crates/cli/tests/service.rs` gives: the daemon's unix socket path has to
/// stay inside the platform's 104-byte limit.
struct DaemonEnv {
    dir: PathBuf,
    serve: Option<std::process::Child>,
}

impl DaemonEnv {
    fn new(tag: &str) -> DaemonEnv {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = PathBuf::from("/tmp").join(format!("cqp-{tag}-{nanos}"));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::create_dir_all(dir.join("cache")).unwrap();
        DaemonEnv { dir, serve: None }
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("config/crystalline/config.yaml")
    }
    fn domain_dir(&self) -> PathBuf {
        self.dir.join("kb-vents")
    }
    fn state_dir(&self) -> PathBuf {
        self.dir.join("state/crystalline")
    }
    fn info_path(&self) -> PathBuf {
        self.state_dir().join("service.json")
    }
    fn lock_path(&self) -> PathBuf {
        self.state_dir().join("service.lock")
    }

    /// Isolate a child into this test's directories, with the HTTP endpoint
    /// off (a real port is the one thing a temp directory cannot isolate) and
    /// the real keychain refused.
    fn apply(&self, cmd: &mut std::process::Command) {
        cmd.env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("XDG_CACHE_HOME", self.dir.join("cache"))
            .env("CRYSTALLINE_SERVICE_HTTP", "false")
            .env("CRYSTALLINE_TEST_NO_KEYCHAIN", "1");
    }

    /// The domain on disk: a MANIFEST and the one engram the prompts recall.
    fn write_domain(&self) {
        let dir = self.domain_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: vents\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# vents\n\n## Scope\n\n- vents\n\n## When to Use\n\n- Route here for vents\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("vent-driver-retry.md"),
            format!(
                "---\ntype: engram\ntitle: {VENT_TITLE}\npermalink: vent-driver-retry\ntags:\n  - vents\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{VENT_BODY}\n"
            ),
        )
        .unwrap();
    }

    /// Start the daemon against this environment's config and wait until it
    /// answers.
    fn serve(&mut self) {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        self.apply(&mut cmd);
        let child = cmd
            .args(["serve", "--config"])
            .arg(self.config_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        self.serve = Some(child);
        let start = Instant::now();
        while !self.run(&["ctl", "status", "--json"]).0 {
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "the daemon did not become ready"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        // What every assertion below rests on: there is a real daemon here,
        // this one, and not a command that answered by opening the index
        // itself. The hook only ever speaks to a daemon that published this
        // record.
        let text =
            std::fs::read_to_string(self.info_path()).expect("the daemon published a record");
        let record: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(record["started_by"], json!("serve"), "{record}");
        assert!(
            record["pid"].as_u64().is_some_and(|pid| pid > 0),
            "{record}"
        );
    }

    /// Run a one-shot command, returning (success, stdout).
    fn run(&self, args: &[&str]) -> (bool, String) {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        self.apply(&mut cmd);
        let out = cmd.args(args).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }

    /// The daemon's status report.
    fn status(&self) -> Value {
        let (ok, out) = self.run(&["ctl", "status", "--json"]);
        assert!(ok, "ctl status failed: {out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("status json: {e}: {out}"))
    }

    /// One search through the daemon, the same default mode the agent's own
    /// tool asks for.
    fn search(&self, query: &str) -> Value {
        let (ok, out) = self.run(&["--json", "search", query]);
        assert!(ok, "search failed: {out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("search json: {e}: {out}"))
    }

    /// Wait until every chunk is embedded for the active model, so a hybrid
    /// search really ranks against vectors rather than falling back to text.
    fn wait_embedded(&self) {
        let start = Instant::now();
        loop {
            let status = self.status();
            if status["embeddings"]["hybrid_available"] == json!(true)
                && status["activity"]["embedding_backlog"] == json!(0)
            {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "the daemon never finished embedding: {status}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Run `hook prompt` against this environment, returning the child's
    /// output and how long the whole spawn took.
    fn hook_prompt(&self, session: &str, prompt: &str) -> (std::process::Output, Duration) {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        self.apply(&mut cmd);
        let start = Instant::now();
        let mut child = cmd
            .args(["hook", "prompt", "--harness", "claude-code"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(prompt_payload(session, prompt).as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (out, start.elapsed())
    }

    /// Tell the routing-prompt command its session was compacted, which is
    /// where the recalled list is emptied. Its own stdout (the routing block)
    /// is not this test's subject.
    fn compacted(&self, session: &str) {
        let payload = json!({
            "session_id": session,
            "source": "compact",
            "hook_event_name": "SessionStart",
        })
        .to_string();
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("crystalline"));
        self.apply(&mut cmd);
        let mut child = cmd
            .args(["prompt", "system", "--harness", "claude-code"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "prompt system failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// The session's state file, as the Stop hook and the prompt hook share it.
    fn session_state(&self, session: &str) -> Value {
        let path = self
            .state_dir()
            .join("hooks")
            .join(format!("{session}.json"));
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("no state file at {}: {e}", path.display()));
        serde_json::from_slice(&bytes).unwrap()
    }

    /// The addresses the session has already been shown.
    fn recalled(&self, session: &str) -> Vec<String> {
        self.session_state(session)["recalled"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stop the daemon and wait until the lock is free, so the temp directory
    /// goes away without a live writer in it.
    fn shutdown(&mut self) {
        let _ = self.run(&["ctl", "shutdown"]);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(8) {
            if !self.lock_path().exists() && !self.info_path().exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(mut child) = self.serve.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for DaemonEnv {
    fn drop(&mut self) {
        self.shutdown();
        if let Ok(text) = std::fs::read_to_string(self.info_path())
            && let Ok(v) = serde_json::from_str::<Value>(&text)
            && let Some(pid) = v.get("pid").and_then(Value::as_u64)
        {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The prompt the fixture engram answers. Every one of its words is a
/// substring of the engram's title or body, so the lexical half of hybrid
/// ranking finds it on its own.
const PLAIN_PROMPT: &str = "how does the vent driver retry";

/// The same question as a harness delivers a message from another session:
/// the wrapper puts tokens in the query that match no engram, so this one
/// reaches the engram through the semantic half alone.
const MESSAGE_PROMPT: &str = "<cross-session-message from=\"x\">how does the vent driver retry, and how does the vent driver retry when the write fails again</cross-session-message>";

/// Text mode is silence. A daemon with no vectors answers a hybrid search in
/// text mode, and a text score is an unbounded term frequency: unrankable
/// against the floor the block's quality rests on. The engram is found, and
/// the hook still says nothing.
#[test]
fn the_prompt_hook_is_silent_when_the_daemon_answers_in_text_mode() {
    let mut env = DaemonEnv::new("textmode");
    env.write_domain();
    write_daemon_config(&env.config_path(), &env.domain_dir(), None);
    env.serve();

    // The precondition, asserted rather than assumed: no provider was built,
    // so the search an agent would run comes back in text mode - and it does
    // find the engram, which is what makes the silence below a decision
    // rather than an empty result.
    let status = env.status();
    assert_eq!(
        status["embeddings"]["hybrid_available"],
        json!(false),
        "{status}"
    );
    let search = env.search(PLAIN_PROMPT);
    assert_eq!(search["mode"], json!("text"), "{search}");
    assert!(
        search["total"].as_u64().unwrap_or(0) >= 1,
        "the engram is there to be found: {search}"
    );

    let (out, elapsed) = env.hook_prompt("session-text-mode", PLAIN_PROMPT);
    assert_silent(&out, "a daemon in text mode");
    println!("hook prompt against a text-mode daemon: {elapsed:?}");
    assert!(
        !env.state_dir()
            .join("hooks")
            .join("session-text-mode.json")
            .exists(),
        "a mode bail must not write state"
    );

    env.shutdown();
}

/// The whole block, end to end against a real daemon ranking against real
/// vectors: what it names, that it names it once per session, that a
/// compaction makes it fair to name again, and that neither a wakeup nor a
/// message from another session costs a second search on a topic this
/// session already knows.
#[test]
fn the_prompt_hook_names_an_engram_once_per_session_and_again_after_a_compaction() {
    let (endpoint, stub) = spawn_embeddings_stub();
    let mut env = DaemonEnv::new("recall");
    env.write_domain();
    write_daemon_config(&env.config_path(), &env.domain_dir(), Some(&endpoint));
    env.serve();
    env.wait_embedded();

    // The precondition, and the warm-up: the search the hook runs really is
    // ranked against vectors, and the daemon's first round trip to the stub
    // is not charged to the hook's one-second budget.
    let search = env.search(PLAIN_PROMPT);
    assert_eq!(search["mode"], json!("hybrid"), "{search}");

    let session = "session-recall";

    // The block itself.
    let (out, elapsed) = env.hook_prompt(session, PLAIN_PROMPT);
    let block = expect_block(&out, "the first prompt");
    println!("hook prompt with a block: {elapsed:?}");
    assert!(
        block.starts_with(RECALL_HEADER),
        "the block opens with the header the agent is taught to read: {block:?}"
    );
    assert!(
        block
            .lines()
            .any(|l| l.starts_with("- ") && l.contains(VENT_ADDRESS)),
        "the block names the engram by its address: {block:?}"
    );

    // Written before the block was printed, and holding exactly what was
    // shown.
    assert_eq!(
        env.recalled(session),
        vec![VENT_ADDRESS.to_string()],
        "the session remembers what it was shown"
    );

    // The same prompt again: the search runs and the dedupe, not the gate,
    // keeps it quiet.
    let before = stub.calls();
    let (out, _) = env.hook_prompt(session, PLAIN_PROMPT);
    assert_silent(&out, "the same prompt twice in one session");
    assert_eq!(
        stub.calls(),
        before + 1,
        "the prompt was searched, and the dedupe silenced the answer"
    );

    // A compaction empties the list: the block that named the engram is gone
    // from the agent's context, so the engram is fair to name again.
    env.compacted(session);
    assert!(
        env.recalled(session).is_empty(),
        "a compaction empties what the session was shown"
    );
    let (out, _) = env.hook_prompt(session, PLAIN_PROMPT);
    let block = expect_block(&out, "after a compaction");
    assert!(
        block.contains(VENT_ADDRESS),
        "the engram is named again: {block:?}"
    );

    // A prompt that shares no word with the engram: the floor does its job.
    let before = stub.calls();
    let (out, _) = env.hook_prompt(session, "what colour is the sky today");
    assert_silent(&out, "a prompt about nothing this domain teaches");
    assert_eq!(
        stub.calls(),
        before + 1,
        "it was searched, and the floor left nothing to say"
    );

    // A scheduled wakeup's sentinel carries no subject, so it never becomes a
    // search at all: one word is under the gate's word bar.
    let before = stub.calls();
    let state_before = env.session_state(session);
    let (out, _) = env.hook_prompt(session, "<<autonomous-loop>>");
    assert_silent(&out, "a wakeup sentinel");
    assert_eq!(
        stub.calls(),
        before,
        "a gated prompt costs no search at all"
    );
    assert_eq!(
        env.session_state(session),
        state_before,
        "a gated prompt writes no state"
    );

    // A wakeup that does carry words goes the normal way and finds nothing
    // worth naming.
    let before = stub.calls();
    let (out, _) = env.hook_prompt(session, "check the build again and report");
    assert_silent(&out, "a plain wakeup prompt");
    assert_eq!(stub.calls(), before + 1, "it was searched");

    // A message from another session, about the topic this one already
    // recalled: searched, and silent because of the dedupe.
    let before = stub.calls();
    let (out, _) = env.hook_prompt(session, MESSAGE_PROMPT);
    assert_silent(&out, "a message-shaped prompt on a recalled topic");
    assert_eq!(stub.calls(), before + 1, "it was searched");

    // The same message in a session that has recalled nothing yet: the
    // wrapper never kept the prompt from reaching the search, so the silence
    // above was the dedupe and nothing else.
    let (out, elapsed) = env.hook_prompt("session-message", MESSAGE_PROMPT);
    let block = expect_block(&out, "a message-shaped prompt in a fresh session");
    println!("hook prompt with a message-shaped prompt: {elapsed:?}");
    assert!(
        block.contains(VENT_ADDRESS),
        "the same engram is named: {block:?}"
    );

    env.shutdown();
    drop(stub);
}

/// The one line a speaking prompt hook prints, parsed, with the event name
/// checked and the block handed back.
fn expect_block(out: &std::process::Output, case: &str) -> String {
    assert!(
        out.status.success(),
        "{case}: exit 0, got {:?}",
        out.status.code()
    );
    assert!(
        out.stderr.is_empty(),
        "{case}: a hook logs nothing: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = stdout.trim_end_matches('\n');
    assert!(
        !line.is_empty() && !line.contains('\n'),
        "{case}: the hook prints exactly one line: {stdout:?}"
    );
    let payload: Value =
        serde_json::from_str(line).unwrap_or_else(|e| panic!("{case}: not JSON: {e}: {line:?}"));
    assert_eq!(
        payload["hookSpecificOutput"]["hookEventName"],
        json!("UserPromptSubmit"),
        "{case}: {payload}"
    );
    payload["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{case}: no additionalContext: {payload}"))
        .to_string()
}
