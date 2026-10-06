//! Session start, the per-prompt recall and the Stop hook on a machine with a
//! connected server and no daemon running: everything comes from the cache
//! files, and no hook process ever talks to a server itself.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::{Value, json};

use crate::common::{crystalline, isolate, isolated_state_dir};
use crate::remote_server::CliServer;

fn bin(home: &Path) -> Command {
    let mut cmd = crystalline();
    isolate(&mut cmd, home);
    cmd
}

fn connect(server: &CliServer, home: &Path) {
    bin(home)
        .args(["connect", &server.origin, "--name", "acme", "--token"])
        .write_stdin(format!("{}\n", server.token_for("keeper")))
        .assert()
        .success();
}

fn host_dir(server: &CliServer, home: &Path) -> PathBuf {
    isolated_state_dir(home)
        .join("remote")
        .join(crystalline_remote::server_key(&server.origin))
}

fn prompt_system(home: &Path) -> std::process::Output {
    bin(home)
        .args(["prompt", "system", "--harness", "claude-code"])
        .write_stdin("")
        .output()
        .unwrap()
}

#[test]
fn session_start_lists_the_mounted_domains() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path());
    let out = prompt_system(home.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("CRYSTALLINE KNOWLEDGE ROUTING"), "{stdout}");
    assert!(
        stdout.contains("Route here for shared open questions"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("may be out of date"),
        "fresh after connect: {stdout}"
    );
}

/// Review focus 4: the source's last refresh failed. Its cached part is
/// printed with one line saying so, in the fixed sentence for a server that
/// cannot be reached, never the recorded failure's own words.
#[test]
fn session_start_with_a_source_down_prints_its_cached_part_and_the_staleness_line() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path());
    server.stop();
    let routing = host_dir(&server, home.path()).join("routing.json");
    let mut cached: Value = serde_json::from_slice(&std::fs::read(&routing).unwrap()).unwrap();
    cached["last_failure"] = json!(format!(
        "acme ({}) cannot be reached right now: connection refused SERVER-SAID (check the VPN or the network)",
        server.origin
    ));
    std::fs::write(&routing, cached.to_string()).unwrap();
    let out = prompt_system(home.path());
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Route here for shared open questions"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "Note: acme ({}) cannot be reached right now, so its domains in this routing block are the copy from",
            server.origin
        )),
        "{stdout}"
    );
    assert!(
        !stdout.contains("SERVER-SAID"),
        "the recorded failure is never quoted: {stdout}"
    );
}

/// The JSON shape gains a `stale` array beside the mounted rows.
#[test]
fn session_start_as_json_names_the_mounted_domain_and_the_stale_source() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path());
    server.stop();
    let routing = host_dir(&server, home.path()).join("routing.json");
    let mut cached: Value = serde_json::from_slice(&std::fs::read(&routing).unwrap()).unwrap();
    cached["last_failure"] = json!("acme cannot be reached right now: refused");
    std::fs::write(&routing, cached.to_string()).unwrap();
    let out = bin(home.path())
        .args(["prompt", "system", "--format", "json"])
        .write_stdin("")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = value["domains"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["name"].as_str())
        .collect();
    assert_eq!(names, vec!["open"], "{value}");
    let stale = value["stale"].as_array().expect("a stale array");
    assert_eq!(stale.len(), 1, "{value}");
    assert!(stale[0].as_str().unwrap().starts_with("Note: acme ("));
}

/// Spec A5 Testing: the Stop hook combines local and remote evolve status;
/// the cooldown is stamped locally.
#[test]
fn the_stop_hook_combines_the_local_and_the_remote_evolve_status() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path());
    let host = host_dir(&server, home.path());
    std::fs::write(
        host.join("hook_status.json"),
        json!({
            "account": "keeper",
            "etag": "cached",
            "fetched_at": chrono::Utc::now().to_rfc3339(),
            "data": { "evolve": { "pending_domains": ["open"] } },
        })
        .to_string(),
    )
    .unwrap();
    let work = tempfile::tempdir().unwrap();
    let transcript = work.path().join("transcript.jsonl");
    std::fs::write(
        &transcript,
        (0..25)
            .map(|i| format!("{{\"turn\":{i}}}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let out = bin(home.path())
        .args(["hook", "stop"])
        .write_stdin(
            json!({
                "session_id": "sources-stop",
                "transcript_path": transcript.display().to_string(),
                "hook_event_name": "Stop",
            })
            .to_string(),
        )
        .output()
        .unwrap();
    assert!(out.status.success());
    let decision: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("no nudge ({e}): {:?}", String::from_utf8_lossy(&out.stdout)));
    let reason = decision["reason"].as_str().unwrap();
    assert!(reason.contains("Focus domains: open."), "{reason}");
    assert!(
        !reason.contains("share_changes"),
        "no local team domain, no share nudge: {reason}"
    );
    let local =
        std::fs::read_to_string(isolated_state_dir(home.path()).join("hooks/maintenance.json"))
            .unwrap();
    assert!(
        local.contains("last_nudge_at"),
        "the cooldown is this machine's: {local}"
    );
}

/// A source that accepts a connection and never answers, at the address the
/// connected server had. Counts every connection it is offered, so a hook
/// that talked to the source itself is caught, not only one that hung.
struct Blackhole {
    offered: Arc<AtomicUsize>,
    _listener: Arc<TcpListener>,
}

impl Blackhole {
    fn at(origin: &str) -> Blackhole {
        let addr = origin.trim_start_matches("http://");
        let listener = Arc::new(TcpListener::bind(addr).unwrap());
        let offered = Arc::new(AtomicUsize::new(0));
        let (held, counted) = (listener.clone(), offered.clone());
        std::thread::spawn(move || {
            let mut open = Vec::new();
            for stream in held.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                // Held open and never answered.
                open.push(stream);
            }
        });
        Blackhole {
            offered,
            _listener: listener,
        }
    }

    fn offered(&self) -> usize {
        self.offered.load(Ordering::SeqCst)
    }
}

fn recall(home: &Path, config: Option<&Path>) -> (std::process::Output, Duration) {
    let mut cmd = bin(home);
    if let Some(config) = config {
        cmd.env("CRYSTALLINE_CONFIG", config);
    }
    let start = Instant::now();
    let out = cmd
        .args(["hook", "prompt", "--harness", "claude-code"])
        .write_stdin(
            json!({
                "session_id": "sources-recall",
                "prompt": "how does the vent driver retry a write",
                "hook_event_name": "UserPromptSubmit",
            })
            .to_string(),
        )
        .output()
        .unwrap();
    (out, start.elapsed())
}

/// Recall with sources but no daemon: silent, as without them. Ruling F20:
/// the source is configured but cannot be reached (it accepts and never
/// answers), and the test runs both with no local domain and with one, so
/// the "nothing registered" bail is not what keeps it quiet. A hook that
/// asked the source itself would be offered to the blackhole and caught.
#[test]
fn the_prompt_hook_stays_silent_without_a_daemon() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path());
    server.stop();
    let blackhole = Blackhole::at(&server.origin);

    let notes = home.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(
        notes.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: notes\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# notes\n\n## When to Use\n\n- Route here for notes\n",
    )
    .unwrap();
    let config = home.path().join("with-local.yaml");
    std::fs::write(
        &config,
        format!("domains:\n  notes:\n    path: {}\n", notes.display()),
    )
    .unwrap();

    for (case, config) in [
        ("source only", None),
        ("local and source", Some(config.as_path())),
    ] {
        let (out, took) = recall(home.path(), config);
        assert!(out.status.success(), "{case}");
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "{case}: {:?} {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            took < Duration::from_secs(3),
            "{case}: the hook waited on the source: {took:?}"
        );
    }
    assert_eq!(
        blackhole.offered(),
        0,
        "no hook process ever connects to a source itself"
    );
    assert!(
        !isolated_state_dir(home.path())
            .join("service.json")
            .exists(),
        "the recall never starts a daemon"
    );
}
