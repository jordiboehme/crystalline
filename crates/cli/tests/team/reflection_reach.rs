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

/// `eng` (Reflection on) and `harbor` (one engram) registered in an
/// isolated home, both shared, with no account.
fn registered() -> tempfile::TempDir {
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
    home
}

/// [`registered`] with keeper as an account and `harbor` private to keeper.
fn workspace() -> tempfile::TempDir {
    let home = registered();
    let h = home.path();
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

/// [`registered`] with `eng` connected to a GitHub repository and its
/// Reflection at home here; `harbor` stays a local domain, shared, so the
/// accounts allow the citation and only the GitHub rule refuses it.
fn team_workspace() -> tempfile::TempDir {
    use crystalline_core::config::{GlobalConfig, OriginConfig};
    let home = registered();
    let h = home.path();
    let manifest = h.join("eng/MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replacen(
            "reflection: on\n",
            "reflection: on\nreflection_home: https://kb.example.com\n",
            1,
        ),
    )
    .unwrap();
    let path = h.join("config.yaml");
    let mut cfg: GlobalConfig = crystalline_core::config::load_yaml(&path).unwrap();
    cfg.domains.get_mut("eng").unwrap().origin = Some(OriginConfig {
        repo: "acme/kb".into(),
        path: None,
        branch: Some("main".into()),
        poll_secs: None,
    });
    let mut service = cfg.service.clone().unwrap_or_default();
    service.public_url = Some("https://kb.example.com".into());
    cfg.service = Some(service);
    crystalline_core::config::save_yaml(&path, &cfg).unwrap();
    ok(h, &["sync"], None);
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

#[test]
fn the_embedded_stack_refuses_a_local_source_on_a_github_page() {
    let home = team_workspace();
    let answer = cite_over(child(home.path(), &["mcp", "--embedded"]));
    assert!(
        answer.contains("this source is in a domain that is not shared through GitHub"),
        "{answer}"
    );
    assert!(!home.path().join("eng/reflections/overview.md").exists());
}

/// [`cite_over`] from a legacy client that declared it can put a question to
/// its user: every `elicitation/create` the server sends is answered with
/// `yes` (a ticked box and Accept) or a decline. Answers whether a question
/// came, and the call's answer as text.
fn cite_eliciting(mut child: Child, yes: bool) -> (bool, String) {
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut send = |msg: Value| writeln!(stdin, "{msg}").unwrap();
    send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": { "elicitation": {} },
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
    let mut asked = false;
    let mut answer = String::new();
    loop {
        let mut line = String::new();
        if out.read_line(&mut line).unwrap() == 0 {
            break;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message["method"] == "elicitation/create" {
            asked = true;
            let result = if yes {
                json!({ "action": "accept", "content": { "confirm": true } })
            } else {
                json!({ "action": "decline" })
            };
            send(json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }));
            continue;
        }
        if message["id"] == 2 {
            answer = line;
            break;
        }
    }
    drop(stdin);
    let _ = child.wait();
    (asked, answer)
}

/// Start a daemon with HTTP off in `h` and wait until it answers.
fn daemon(h: &Path) -> Child {
    let daemon = child(h, &["serve", "--http", "off"]);
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
    daemon
}

/// Through the real bridge and daemon relay: the person behind a client that
/// can ask is asked, and their yes lands the write.
#[test]
fn a_person_is_asked_through_the_bridge_and_their_yes_lands_the_write() {
    let home = workspace();
    let h = home.path();
    let mut serving = daemon(h);
    let (asked, answer) = cite_eliciting(child(h, &["mcp"]), true);
    let _ = bin(h).args(["ctl", "shutdown"]).output();
    let _ = serving.wait();
    assert!(asked, "the question reached the client: {answer}");
    assert!(!answer.contains("\"error\""), "{answer}");
    assert!(h.join("eng/reflections/overview.md").exists(), "{answer}");
}

/// The session a headless run opens (`mcp --unattended`) is never asked,
/// though its client could, and the write is refused.
#[test]
fn an_unattended_session_through_the_bridge_is_never_asked() {
    let home = workspace();
    let h = home.path();
    let mut serving = daemon(h);
    let (asked, answer) = cite_eliciting(child(h, &["mcp", "--unattended"]), true);
    let _ = bin(h).args(["ctl", "shutdown"]).output();
    let _ = serving.wait();
    assert!(!asked, "{answer}");
    // It does not even see harbor: refused, as a domain that does not exist.
    assert!(
        answer.contains("\"error\"") || answer.contains("\"isError\":true"),
        "{answer}"
    );
    assert!(!h.join("eng/reflections/overview.md").exists());
}

/// The embedded stack, which a headless run falls back to, keeps the mark.
#[test]
fn an_unattended_embedded_session_is_never_asked() {
    let home = workspace();
    let (asked, answer) = cite_eliciting(
        child(home.path(), &["mcp", "--embedded", "--unattended"]),
        true,
    );
    assert!(!asked, "{answer}");
    // It does not even see harbor: refused, as a domain that does not exist.
    assert!(
        answer.contains("\"error\"") || answer.contains("\"isError\":true"),
        "{answer}"
    );
}
