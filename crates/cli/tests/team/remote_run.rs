//! `crystalline evolve run` in remote mode, end to end: the CLI reaches a
//! served instance with a `cmt_` token from `CRYSTALLINE_REMOTE_URL` and
//! `CRYSTALLINE_REMOTE_TOKEN`, a fake `claude` on PATH stands in for the
//! agent, and the agent's one write goes over HTTP MCP with the bearer from
//! the config the run handed it. The run shares that write as a pull request
//! on an in-memory forge, records the run and releases the lease.
//!
//! The fake notes its MCP config and waits; the test makes the agent's write
//! with that config (the URL and the header, nothing else), then lets the
//! fake end with its summary block.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::forge::Forge;
use crate::remote_server::CliServer;

/// Carries the team marker, as every team repository does after its first
/// share from 0.30.0 on, so a run's share carries only the run's files.
fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: brand\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# brand\n\n## Scope\n\n- The brand team's knowledge\n\n## When to Use\n\n- Route here for brand questions\n".to_vec()
}

fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-09-01\n---\n\n# {title}\n\n{body}\n"
    )
    .into_bytes()
}

/// The repository: a summary without its reciprocal (V103), so the evolve
/// loop is due with one mechanical finding.
fn repository() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        ("MANIFEST.md".to_string(), manifest()),
        (
            "summary-doc.md".to_string(),
            engram(
                "Summary doc",
                "summary-doc",
                "The distilled version of the migration write-up.\n\n- summarizes [[Full text doc]]\n- [context] written after the migration closed",
            ),
        ),
        (
            "full-text-doc.md".to_string(),
            engram(
                "Full text doc",
                "full-text-doc",
                "The full migration write-up with the shard tables.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary",
            ),
        ),
    ])
}

const SUMMARY: &str =
    "Wrote the run fix.\\n\\nDone: 1\\nLeft for a person: 0\\nErrors: 0\\nThe reciprocal waits.";

struct Runner {
    home: tempfile::TempDir,
    url: String,
    token: String,
}

impl Runner {
    fn bin(&self) -> PathBuf {
        self.home.path().join("bin")
    }

    fn seen(&self) -> PathBuf {
        self.home.path().join("seen-config")
    }

    fn env_names(&self) -> PathBuf {
        self.home.path().join("agent-env")
    }

    fn go(&self) -> PathBuf {
        self.home.path().join("go")
    }

    /// A `claude` that notes the path of its `--mcp-config`, waits for the
    /// test to make the agent's write, then prints the stream of a session
    /// that ended with the summary block.
    fn fake_claude(&self) {
        std::fs::create_dir_all(self.bin()).unwrap();
        let script = format!(
            r#"#!/bin/sh
env | cut -d= -f1 > "{env}"
prev=""
for a in "$@"; do
  if [ "$prev" = "--mcp-config" ]; then printf '%s' "$a" > "{seen}.tmp"; mv "{seen}.tmp" "{seen}"; fi
  prev="$a"
done
echo '{{"type":"system","subtype":"init","tools":[]}}'
n=0
while [ ! -f "{go}" ] && [ $n -lt 600 ]; do sleep 0.1; n=$((n+1)); done
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{SUMMARY}"}}]}}}}'
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"result":"{SUMMARY}"}}'
exit 0
"#,
            seen = self.seen().display(),
            go = self.go().display(),
            env = self.env_names().display(),
        );
        let path = self.bin().join("claude");
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The CLI in remote mode, isolated, with the fake first on PATH.
    fn cli(&self, args: &[&str]) -> Command {
        let mut cmd = crate::common::crystalline_std();
        for (name, value) in crate::common::isolation_env(self.home.path()) {
            cmd.env(name, value);
        }
        cmd.env_remove("RUST_LOG")
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env("CRYSTALLINE_REMOTE_URL", &self.url)
            .env("CRYSTALLINE_REMOTE_TOKEN", &self.token)
            .args(args);
        cmd
    }

    fn status(&self) -> Value {
        let out = self
            .cli(&["--json", "evolve", "status", "--domain", "brand"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.trim())
            .unwrap_or_else(|e| panic!("status json: {e}: {}", explain(&out)))
    }
}

fn explain(out: &Output) -> String {
    format!(
        "code {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn wait_for(what: &str, limit: Duration, mut ready: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ready() {
        assert!(start.elapsed() < limit, "{what} did not happen in time");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One raw HTTP/1.1 POST to the MCP endpoint `url` (the server root), read
/// for a bounded window: a `tools/call` answer is a stream the transport
/// keeps open, so there is no end to wait for.
fn mcp_post(url: &str, auth: &str, session: Option<&str>, body: &Value) -> String {
    let rest = url.strip_prefix("http://").expect("an http url");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut stream = TcpStream::connect(host).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let body = body.to_string();
    let mut request = format!(
        "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nAuthorization: {auth}\r\n"
    );
    if let Some(session) = session {
        request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    stream.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_millis(2500);
    while Instant::now() < deadline {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // A whole JSON-RPC answer has arrived.
                if String::from_utf8_lossy(&buf).contains("\"jsonrpc\"")
                    && String::from_utf8_lossy(&buf).contains("\"result\"")
                {
                    break;
                }
            }
            Err(_) => {}
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn session_id(raw: &str) -> String {
    raw.split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_string())
        })
        .unwrap_or_else(|| panic!("no mcp-session-id in:\n{raw}"))
}

/// The agent's write: an MCP session with the config's URL and header, and
/// one `write_engram` in `brand`.
fn agent_writes(config: &Value) {
    let entry = &config["mcpServers"]["crystalline"];
    let url = entry["url"].as_str().unwrap();
    let auth = entry["headers"]["Authorization"].as_str().unwrap();
    let opened = mcp_post(
        url,
        auth,
        None,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "fake-claude", "version": "0.0.0" },
            },
        }),
    );
    assert!(opened.starts_with("HTTP/1.1 200 "), "{opened}");
    let session = session_id(&opened);
    let ready = mcp_post(
        url,
        auth,
        Some(&session),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert!(ready.starts_with("HTTP/1.1 2"), "{ready}");
    let wrote = mcp_post(
        url,
        auth,
        Some(&session),
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {
                "name": "write_engram",
                "arguments": {
                    "domain": "brand",
                    "title": "Run Fix",
                    "tags": ["eng"],
                    "content": "What the headless run fixed.\n\n- [fact] the run wrote this over MCP\n- [context] written by the evolve runner",
                },
            },
        }),
    );
    assert!(
        wrote.contains("run-fix") && !wrote.contains("\"isError\":true"),
        "the agent's write was not taken:\n{wrote}"
    );
}

/// A share the forge refuses does not vanish: the recorded run and the JSON
/// both say so, with the fix.
#[test]
fn a_refused_share_is_in_the_recorded_run_and_the_json() {
    let forge = Arc::new(Forge::with_main(repository()));
    let refusal = "This instance shares with personal GitHub identities through its GitHub App.";
    forge.refuse_proposals(refusal);
    let server = CliServer::start_team(forge.clone());
    let runner = Runner {
        home: tempfile::tempdir().unwrap(),
        url: server.origin.clone(),
        token: server.agent_token("runner"),
    };
    runner.fake_claude();
    let start = Instant::now();
    while runner.status()["domains"][0]["due"] != true {
        assert!(start.elapsed() < Duration::from_secs(30), "never due");
        std::thread::sleep(Duration::from_millis(200));
    }
    let logs = runner.home.path().join("logs");
    let run = runner
        .cli(&[
            "--json",
            "evolve",
            "run",
            "--domain",
            "brand",
            "--runner",
            "k8s",
            "--log-dir",
            logs.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let seen = runner.seen();
    wait_for("the agent start", Duration::from_secs(60), || seen.exists());
    let config_path = PathBuf::from(std::fs::read_to_string(&seen).unwrap());
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    agent_writes(&config);
    std::fs::write(runner.go(), b"").unwrap();
    let out = run.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let answer: Value = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap_or_else(|| panic!("no json answer: {}", explain(&out)));
    assert_eq!(answer["share"], Value::Null, "{answer}");
    let error = answer["share_error"].as_str().unwrap_or_default();
    assert!(error.contains(refusal), "{answer}");
    assert!(error.contains("not shared"), "{answer}");
    // A remote runner's token need not be an admin's: no command line for it.
    assert!(!error.contains("crystalline "), "{answer}");
    assert!(!error.contains("github."), "{answer}");
    // The recorded run is read by anybody who may run the loop: plain words,
    // no command line and no setting.
    let summary = answer["run"]["summary"].as_str().unwrap_or_default();
    assert!(
        summary.starts_with("The changes of this run could not be shared"),
        "{answer}"
    );
    assert!(!summary.contains("crystalline "), "{answer}");
    assert!(!summary.contains("github."), "{answer}");
    assert!(!summary.contains(refusal), "{answer}");
}

#[test]
fn a_remote_evolve_run_shares_the_agents_mcp_write_as_a_pull_request() {
    let forge = Arc::new(Forge::with_main(repository()));
    let server = CliServer::start_team(forge.clone());
    let runner = Runner {
        home: tempfile::tempdir().unwrap(),
        url: server.origin.clone(),
        token: server.agent_token("runner"),
    };
    assert!(runner.token.starts_with("cmt_"), "{}", runner.token);
    runner.fake_claude();

    // Status, as the token's account: due with the one mechanical finding.
    let start = Instant::now();
    loop {
        let s = runner.status();
        if s["domains"][0]["due"] == true {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "brand never became due: {s}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    let logs = runner.home.path().join("logs");
    let mut run = runner
        .cli(&[
            "--json",
            "evolve",
            "run",
            "--domain",
            "brand",
            "--runner",
            "k8s",
            "--log-dir",
            logs.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // The run took the lease and started the agent with its config.
    let seen = runner.seen();
    wait_for("the agent start", Duration::from_secs(60), || seen.exists());
    let config_path = PathBuf::from(std::fs::read_to_string(&seen).unwrap());
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"]["crystalline"]["url"],
        format!("{}/", server.origin),
        "the MCP endpoint is the server root: {config}"
    );
    assert_eq!(
        config["mcpServers"]["crystalline"]["headers"]["Authorization"],
        format!("Bearer {}", runner.token),
        "{config}"
    );
    let during = runner.status();
    assert_eq!(during["domains"][0]["due"], false, "{during}");
    assert_eq!(during["domains"][0]["running"]["runner"], "k8s", "{during}");
    assert_eq!(
        during["domains"][0]["running"]["account"], "runner",
        "{during}"
    );

    // The bearer is in the config only, never in the agent's environment.
    let names = std::fs::read_to_string(runner.env_names()).unwrap();
    assert!(
        names.lines().any(|n| n == "CRYSTALLINE_REMOTE_URL"),
        "the fake saw its environment: {names}"
    );
    assert!(
        !names
            .lines()
            .any(|n| n.starts_with("CRYSTALLINE_REMOTE_TOKEN")),
        "the agent got the remote token: {names}"
    );

    agent_writes(&config);
    std::fs::write(runner.go(), b"").unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    while run.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the run did not end");
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = run.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", explain(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let answer: Value = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap_or_else(|| panic!("no json answer: {}", explain(&out)));

    // Shared: only the agent's MCP write, attributed to the token's account,
    // as a pull request on the forge.
    let share = &answer["share"];
    assert_eq!(share["outcome"], "shared", "{answer}");
    assert_eq!(share["attributed"], true, "{answer}");
    let files: Vec<String> = share["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().map_or_else(|| f.to_string(), str::to_string))
        .collect();
    assert_eq!(files, vec!["run-fix.md"], "{answer}");
    assert_eq!(share["url"], "https://github.test/acme/pull/1", "{answer}");
    assert!(
        forge
            .calls()
            .iter()
            .any(|c| c.starts_with("create_proposal")),
        "{:?}",
        forge.calls()
    );
    assert!(
        forge.proposal_files(1).contains(&"run-fix.md".to_string()),
        "{:?}",
        forge.proposal_files(1)
    );

    // Recorded and released.
    assert_eq!(answer["run"]["outcome"], "done", "{answer}");
    assert_eq!(answer["run"]["runner"], "k8s", "{answer}");
    assert_eq!(answer["run"]["account"], "runner", "{answer}");
    assert_eq!(
        answer["run"]["pr_url"], "https://github.test/acme/pull/1",
        "{answer}"
    );
    let after = runner.status();
    let d = &after["domains"][0];
    assert_eq!(d["running"], Value::Null, "the lease was released: {after}");
    assert_eq!(d["last_run"]["outcome"], "done", "{after}");

    // The config that held the bearer is gone with its folder.
    assert!(!config_path.exists(), "the MCP config was left behind");
    assert!(
        !config_path.parent().is_some_and(Path::exists),
        "the config's folder was left behind"
    );
}
