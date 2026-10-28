//! `crystalline github app` and the App checks in `doctor`.
//!
//! The binary runs as a child process, so the seams are the ones that reach
//! it: `CRYSTALLINE_TEST_TOKEN_STORE_DIR` keeps the saved App in a file under
//! a tempdir (no keychain is read or written), and `github.api_url` points
//! every App request at a stand-in GitHub this file starts on a loopback
//! port. The stand-in answers `GET /app` and `GET /app/installations` with a
//! `Date` header the test decides, which is what the clock check reads.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

use crate::common::isolate;

/// The throwaway test key of the remote crate's App tests; it belongs to no
/// App.
const PEM: &str = include_str!("../../../remote/tests/fixtures/app-key-pkcs1.pem");
const CLIENT_SECRET: &str = "CLIENTSECRETVALUE";
const WEBHOOK_SECRET: &str = "WEBHOOKSECRETVALUE";
const SLUG: &str = "crystalline-acme";

/// One line of the private key's body, which no output may carry.
fn key_body() -> &'static str {
    PEM.lines().nth(1).unwrap()
}

/// A stand-in GitHub answering the App routes, its `Date` header
/// `skew_secs` away from this machine's clock.
struct FakeGithub {
    url: String,
    _thread: std::thread::JoinHandle<()>,
}

impl FakeGithub {
    fn start(skew_secs: i64) -> FakeGithub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let body = if path.starts_with("/app/installations") {
                    r#"[{"id":1,"account":{"login":"acme"}}]"#.to_string()
                } else if path == "/app" {
                    format!(r#"{{"id":7,"slug":"{SLUG}","owner":{{"login":"acme"}}}}"#)
                } else {
                    r#"{"message":"Not Found"}"#.to_string()
                };
                let status = if path.starts_with("/app") {
                    "200 OK"
                } else {
                    "404 Not Found"
                };
                let date = (chrono::Utc::now() + chrono::Duration::seconds(skew_secs))
                    .format("%a, %d %b %Y %H:%M:%S GMT");
                let response = format!(
                    "HTTP/1.1 {status}\r\nDate: {date}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeGithub {
            url,
            _thread: thread,
        }
    }
}

/// A config pointing `github.api_url` at `api_url`, with collaboration on.
fn write_config(dir: &Path, api_url: &str) -> PathBuf {
    let config = dir.join("config.yaml");
    std::fs::write(
        &config,
        format!("github:\n  enabled: true\n  api_url: {api_url}\n"),
    )
    .unwrap();
    config
}

/// The three secret files `github app set` reads.
fn write_secret_files(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let key = dir.join("app.pem");
    let client = dir.join("client-secret");
    let webhook = dir.join("webhook-secret");
    std::fs::write(&key, PEM).unwrap();
    std::fs::write(&client, format!("{CLIENT_SECRET}\n")).unwrap();
    std::fs::write(&webhook, format!("{WEBHOOK_SECRET}\n")).unwrap();
    (key, client, webhook)
}

/// A `crystalline` child isolated under `home`, its saved App kept in a file
/// under `tokens`.
fn bin(home: &Path, tokens: &Path) -> Command {
    let mut cmd = crate::common::crystalline();
    isolate(&mut cmd, home);
    cmd.env("CRYSTALLINE_TEST_TOKEN_STORE_DIR", tokens);
    cmd
}

fn assert_no_secret(text: &str) {
    for secret in [CLIENT_SECRET, WEBHOOK_SECRET, key_body(), "PRIVATE KEY"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
}

#[test]
fn github_app_status_json_never_contains_a_secret() {
    let github = FakeGithub::start(0);
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let tokens = work.path().join("tokens");
    let config = write_config(work.path(), &github.url);
    let (key, client, webhook) = write_secret_files(work.path());

    let set = bin(home.path(), &tokens)
        .args(["--json", "github", "app", "set", "--app-id", "7"])
        .args(["--client-id", "Iv23.test", "--private-key-file"])
        .arg(&key)
        .arg("--client-secret-file")
        .arg(&client)
        .arg("--webhook-secret-file")
        .arg(&webhook)
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");
    assert_no_secret(&String::from_utf8_lossy(&set.stdout));
    assert_no_secret(&String::from_utf8_lossy(&set.stderr));
    assert!(tokens.join("github-app.json").exists(), "the App was saved");

    let status = bin(home.path(), &tokens)
        .args(["--json", "github", "app", "status", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(stdout.contains(r#""slug":"crystalline-acme""#), "{stdout}");
    assert_no_secret(&stdout);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["configured"], serde_json::json!(true));
    assert_eq!(report["client_secret_set"], serde_json::json!(true));
    assert_eq!(report["webhook_secret_set"], serde_json::json!(true));
    assert_eq!(report["installations"], serde_json::json!(["acme"]));

    let human = bin(home.path(), &tokens)
        .args(["github", "app", "status", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(human.contains(SLUG), "{human}");
    assert_no_secret(&human);

    let removed = bin(home.path(), &tokens)
        .args(["github", "app", "remove", "--config"])
        .arg(&config)
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
    assert!(!tokens.join("github-app.json").exists(), "the App is gone");
}

#[test]
fn github_app_set_refuses_when_env_supplies_the_app() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let tokens = work.path().join("tokens");
    // Nothing listens here: the refusal comes before GitHub is asked.
    let config = write_config(work.path(), "http://127.0.0.1:1");
    let (key, _, _) = write_secret_files(work.path());

    let out = bin(home.path(), &tokens)
        .env("CRYSTALLINE_GITHUB_APP_ID", "7")
        .env("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.test")
        .env("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", PEM)
        .args(["github", "app", "set", "--app-id", "7"])
        .args(["--client-id", "Iv23.test", "--private-key-file"])
        .arg(&key)
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("CRYSTALLINE_GITHUB_APP_"), "{stderr}");
    assert!(!tokens.join("github-app.json").exists(), "nothing saved");
}

#[test]
fn github_app_set_takes_no_secret_on_the_command_line() {
    let out = crate::common::crystalline()
        .args(["github", "app", "set", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("--private-key-file"), "{help}");
    assert!(help.contains("--client-secret-file"), "{help}");
    assert!(help.contains("--webhook-secret-file"), "{help}");
    assert!(!help.contains("--private-key "), "{help}");
    assert!(!help.contains("--client-secret "), "{help}");
}

/// `doctor --json` with the App from the environment, `extra` variables
/// added, against `github`.
fn doctor_with_app(github: &FakeGithub, extra: &[(&str, &str)]) -> Value {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let config = write_config(work.path(), &github.url);
    let mut cmd = crate::common::crystalline();
    isolate(&mut cmd, home.path());
    cmd.env("CRYSTALLINE_GITHUB_APP_ID", "7")
        .env("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.test")
        .env("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", PEM);
    for (name, value) in extra {
        cmd.env(name, value);
    }
    let out = cmd
        .args(["--json", "doctor", "--config"])
        .arg(&config)
        .arg("--db")
        .arg(work.path().join("index.db"))
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_no_secret(&stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
}

#[test]
fn doctor_flags_clock_skew_over_a_minute() {
    let github = FakeGithub::start(120);
    let report = doctor_with_app(&github, &[]);
    let app = &report["github"]["app"];
    assert_eq!(app["credentials_parse"], serde_json::json!(true), "{app}");
    assert_eq!(app["jwt_accepted"], serde_json::json!(true), "{app}");
    assert_eq!(app["installations_reachable"], serde_json::json!(true));
    let skew = app["clock_skew_secs"].as_i64().unwrap();
    assert!((110..=130).contains(&skew), "{app}");
    let problem = app["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("clock"), "{app}");
}

#[test]
fn doctor_flags_a_public_url_without_webhook_secret() {
    let github = FakeGithub::start(0);
    let report = doctor_with_app(
        &github,
        &[("CRYSTALLINE_SERVICE_PUBLIC_URL", "https://kb.example.com")],
    );
    let app = &report["github"]["app"];
    assert_eq!(
        app["webhook_secret_missing"],
        serde_json::json!(true),
        "{app}"
    );
    let skew = app["clock_skew_secs"].as_i64().unwrap();
    assert!(skew.abs() <= 10, "{app}");
    let problem = app["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("webhook secret"), "{app}");
    assert!(!problem.contains("clock"), "{app}");
}

#[test]
fn doctor_says_connected_through_a_healthy_app() {
    let github = FakeGithub::start(0);
    let report = doctor_with_app(&github, &[]);
    assert_eq!(report["github"]["app"]["problem"], Value::Null, "{report}");
    assert_eq!(report["github"]["connected"], serde_json::json!(true));
    assert_eq!(
        report["github"]["connected_through_app"],
        serde_json::json!(true)
    );

    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let config = write_config(work.path(), &github.url);
    let mut cmd = crate::common::crystalline();
    isolate(&mut cmd, home.path());
    let out = cmd
        .env("CRYSTALLINE_GITHUB_APP_ID", "7")
        .env("CRYSTALLINE_GITHUB_APP_CLIENT_ID", "Iv23.test")
        .env("CRYSTALLINE_GITHUB_APP_PRIVATE_KEY", PEM)
        .args(["doctor", "--config"])
        .arg(&config)
        .arg("--db")
        .arg(work.path().join("index.db"))
        .output()
        .unwrap();
    let human = String::from_utf8(out.stdout).unwrap();
    assert!(
        human.contains("connected through the GitHub App"),
        "{human}"
    );
    assert!(
        !human.contains("Run: crystalline connect github"),
        "{human}"
    );
}
