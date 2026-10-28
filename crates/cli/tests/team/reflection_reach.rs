//! A Reflection page never cites a domain fewer people may read, on the
//! stacks that serve the machine owner without the HTTP surface: the
//! embedded stdio server (`mcp --embedded`, and the fallback a headless run
//! takes when no daemon answers) and a daemon started with `--http off`.
//! Both must know who reads each domain before they answer anything.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use serde_json::{Value, json};

use crate::common::{crystalline, crystalline_std, isolate, isolation_env};

const TOO_WIDE: &str = "this source is in a domain fewer people may read than this page's domain";

fn bin(home: &Path) -> Command {
    let mut cmd = crystalline();
    isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_CONFIG", home.join("config.yaml"));
    cmd
}

fn child(home: &Path, args: &[&str]) -> Child {
    let mut cmd = crystalline_std();
    for (name, value) in isolation_env(home) {
        cmd.env(name, value);
    }
    cmd.env_remove("RUST_LOG")
        .env("CRYSTALLINE_CONFIG", home.join("config.yaml"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn ok(home: &Path, args: &[&str], stdin: Option<&str>) {
    let mut cmd = bin(home);
    cmd.args(args);
    if let Some(input) = stdin {
        cmd.write_stdin(input.to_string());
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `eng` (Reflection on, shared) and `harbor` (one engram, private to
/// keeper), registered in an isolated home.
fn workspace() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    for name in ["eng", "harbor"] {
        let dir = h.join(name);
        ok(
            h,
            &["domain", "init", dir.to_str().unwrap(), "--name", name],
            None,
        );
    }
    let manifest = h.join("eng/MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replacen("---\n", "---\nreflection: on\n", 1),
    )
    .unwrap();
    std::fs::create_dir_all(h.join("harbor/team")).unwrap();
    std::fs::write(
        h.join("harbor/team/harbor-team.md"),
        "---\ntype: engram\ntitle: Harbor team\npermalink: team/harbor-team\ntags:\n  - harbor\nstatus: stable\nrecorded_at: 2026-09-01\n---\n\n# Harbor team\n\n- [fact] five people\n",
    )
    .unwrap();
    for name in ["eng", "harbor"] {
        let dir = h.join(name);
        ok(h, &["domain", "add", name, dir.to_str().unwrap()], None);
    }
    ok(
        h,
        &[
            "users",
            "add",
            "keeper",
            "--role",
            "editor",
            "--password-stdin",
        ],
        Some("s3cret-s3cret\n"),
    );
    ok(
        h,
        &[
            "domain",
            "visibility",
            "harbor",
            "private",
            "--owner",
            "keeper",
        ],
        None,
    );
    home
}

/// Send `write_reflection` naming harbor's engram as a source of a page in
/// `eng` over the stdio session of `child`; the answer as text.
fn cite_over(mut child: Child) -> String {
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut send = |msg: Value| writeln!(stdin, "{msg}").unwrap();
    send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "it", "version": "0" }
        }
    }));
    send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {
            "name": "write_reflection",
            "arguments": {
                "domain": "eng",
                "path": "overview.md",
                "markdown": "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n",
                "sources": { "Team": ["crystalline://harbor/team/harbor-team"] }
            }
        }
    }));
    let mut answer = String::new();
    loop {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            break;
        }
        if line.contains("\"id\":2") {
            answer = line;
            break;
        }
    }
    drop(stdin);
    let _ = child.wait();
    answer
}

#[test]
fn the_embedded_stack_refuses_a_source_fewer_people_may_read() {
    let home = workspace();
    let answer = cite_over(child(home.path(), &["mcp", "--embedded"]));
    assert!(answer.contains(TOO_WIDE), "{answer}");
    assert!(!home.path().join("eng/reflections/overview.md").exists());
}

#[test]
fn a_daemon_with_http_off_refuses_a_source_fewer_people_may_read() {
    let home = workspace();
    let h = home.path();
    let mut daemon = child(h, &["serve", "--http", "off"]);
    let start = Instant::now();
    while !bin(h)
        .args(["ctl", "status", "--json"])
        .output()
        .unwrap()
        .status
        .success()
    {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "daemon not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let answer = cite_over(child(h, &["mcp"]));
    let _ = bin(h).args(["ctl", "shutdown"]).output();
    let _ = daemon.wait();
    assert!(answer.contains(TOO_WIDE), "{answer}");
    assert!(!h.join("eng/reflections/overview.md").exists());
}
