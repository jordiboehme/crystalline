//! `crystalline connect <url>`, `crystalline disconnect <name|url>`, a local
//! rename of a mounted domain, and what `status` and `doctor` say about
//! every source.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use serde_json::Value;

use crate::common::{crystalline, isolate};
use crate::remote_server::CliServer;

fn bin(home: &Path) -> Command {
    let mut cmd = crystalline();
    isolate(&mut cmd, home);
    cmd
}

fn connect(server: &CliServer, home: &Path, name: &str) -> String {
    let out = bin(home)
        .args(["connect", &server.origin, "--name", name, "--token"])
        .write_stdin(format!("{}\n", server.token_for("keeper")))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn status_json(home: &Path) -> Value {
    let out = bin(home).args(["status", "--json"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn mounts(source: &Value) -> Vec<(String, String)> {
    source["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["local"].as_str().unwrap().to_string(),
                m["remote"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn connect_with_a_pasted_token_lists_the_source_in_status() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let said = connect(&server, home.path(), "acme");
    assert!(
        said.contains(&format!(
            "connected to {} as keeper (token); this machine calls it acme",
            server.origin
        )),
        "{said}"
    );
    let status = status_json(home.path());
    let acme = &status["sources"][0];
    assert_eq!(acme["name"], "acme");
    assert_eq!(acme["url"], server.origin.as_str());
    assert_eq!(acme["account"], "keeper");
    assert_eq!(acme["kind"], "token");
    assert_eq!(acme["reachable"], true, "{status}");
    assert_eq!(mounts(acme), vec![("open".to_string(), "open".to_string())]);
    assert!(
        status.get("indexed").is_some(),
        "the local report is still there: {status}"
    );
}

/// Review focus 2: the second server's `open` gets the suffix, said at once,
/// and the first keeps its name.
#[test]
fn connecting_a_second_server_announces_the_collision_name_and_keeps_the_first() {
    let first = CliServer::start();
    let second = CliServer::start_with(&["specs"]);
    let home = tempfile::tempdir().unwrap();
    connect(&first, home.path(), "acme");
    let said = connect(&second, home.path(), "beta");
    assert!(
        said.contains("beta: the domain 'open' is called 'open-beta' on this machine, because 'open' was already taken"),
        "{said}"
    );
    let status = status_json(home.path());
    assert_eq!(
        mounts(&status["sources"][0]),
        vec![("open".to_string(), "open".to_string())]
    );
    let beta = mounts(&status["sources"][1]);
    assert!(
        beta.contains(&("open-beta".to_string(), "open".to_string())),
        "{beta:?}"
    );
    assert!(
        beta.contains(&("specs".to_string(), "specs".to_string())),
        "{beta:?}"
    );
}

#[test]
fn a_token_on_the_command_line_is_refused() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .args([
            "connect",
            "https://kb.example",
            "--token",
            "cmt_on_the_command_line",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "refusing a token on the command line",
        ))
        .stderr(predicates::str::contains("cmt_on_the_command_line").not());
    // The `connect github --token <value>` habit: the token comes first.
    bin(home.path())
        .args([
            "connect",
            "--token",
            "cmt_on_the_command_line",
            "https://kb.example",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "refusing a token on the command line",
        ))
        .stderr(predicates::str::contains("cmt_on_the_command_line").not());
}

/// Spec A1 (ruling F16): a server without browser sign-in asks for a token
/// instead of stopping, and says why.
#[test]
fn a_server_without_browser_sign_in_asks_for_a_token() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let out = bin(home.path())
        .args(["connect", &server.origin, "--name", "acme"])
        .write_stdin(format!("{}\n", server.token_for("keeper")))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("does not offer browser sign-in"),
        "{stderr}"
    );
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("as keeper (token); this machine calls it acme"),
        "{said}"
    );
}

#[test]
fn plain_http_off_this_machine_is_refused() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .args(["connect", "http://kb.example", "--token"])
        .write_stdin("cmt_x\n")
        .assert()
        .failure()
        .stderr(predicates::str::contains("refusing plain http"));
}

#[test]
fn disconnect_removes_one_source_by_name_or_url() {
    let first = CliServer::start();
    let second = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&first, home.path(), "acme");
    connect(&second, home.path(), "beta");
    bin(home.path())
        .args(["disconnect", "acme"])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "disconnected from acme ({})",
            first.origin
        )));
    let status = status_json(home.path());
    let names: Vec<&str> = status["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["beta"]);
    bin(home.path())
        .args(["disconnect", &second.origin])
        .assert()
        .success();
    bin(home.path())
        .args(["disconnect", "acme"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "no connected server is called acme",
        ));
}

/// Spec A3: `domain rename --local` works on a mounted domain and is kept.
#[test]
fn a_local_rename_of_a_mounted_domain_is_kept_and_a_shared_rename_is_refused() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    bin(home.path())
        .args(["domain", "rename", "open", "acme-open", "--local"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "'open' from acme is called 'acme-open' on this machine now",
        ));
    let status = status_json(home.path());
    assert_eq!(
        mounts(&status["sources"][0]),
        vec![("acme-open".to_string(), "open".to_string())]
    );
    bin(home.path())
        .args(["domain", "rename", "acme-open", "other"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("'acme-open' comes from acme; only its name on this machine can change here: add --local"));
}

#[test]
fn doctor_lists_every_source() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    let out = bin(home.path())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["sources"][0]["name"], "acme");
    assert_eq!(report["sources"][0]["reachable"], true, "{report}");
    assert!(report["sources"][0]["problem"].is_null(), "{report}");
}

#[test]
fn status_with_a_server_down_names_it_and_keeps_the_others() {
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    connect(&server, home.path(), "acme");
    server.stop();
    let status = status_json(home.path());
    assert_eq!(status["sources"][0]["reachable"], false, "{status}");
    assert!(
        status["sources"][0]["error"]
            .as_str()
            .unwrap()
            .contains("cannot be reached right now"),
        "{status}"
    );
}

/// With a daemon running: `connect` tells it at once (the `sources_reload`
/// sender), and `status` shows the daemon's rows with what this run asked
/// each server added.
#[test]
fn a_running_daemon_learns_of_a_connect_at_once_and_status_merges_its_rows() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let server = CliServer::start();
    let home = tempfile::tempdir().unwrap();
    let mut serve = crate::common::crystalline_std();
    for (name, value) in crate::common::isolation_env(home.path()) {
        serve.env(name, value);
    }
    // No HTTP: the default port is shared by every daemon on this machine.
    let mut daemon = serve
        .args(["serve", "--http", "off"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon_status = || -> Option<Value> {
        let out = bin(home.path())
            .args(["ctl", "status", "--json"])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| serde_json::from_slice(&out.stdout).unwrap())
    };
    let start = Instant::now();
    while daemon_status().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "daemon not ready"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    connect(&server, home.path(), "acme");
    // Right after connect, before the daemon's own look at the file (5 s).
    let told = daemon_status().unwrap();
    let names: Vec<&str> = told["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["acme"], "{told}");

    let status = status_json(home.path());
    assert!(status.get("pid").is_some(), "the daemon answered: {status}");
    let acme = &status["sources"][0];
    assert_eq!(acme["name"], "acme");
    assert_eq!(acme["reachable"], true, "{status}");
    assert!(
        acme.get("failure").is_some(),
        "the daemon's key stays: {status}"
    );
    assert_eq!(mounts(acme), vec![("open".to_string(), "open".to_string())]);

    let _ = bin(home.path()).args(["ctl", "shutdown"]).output();
    let _ = daemon.wait();
}
