//! A redeploy, end to end with the real binary (spec sections D and F).
//!
//! On Postgres an instance keeps nothing it needs on its own disk: an
//! account, an OAuth client with a completed grant, a virtual domain with
//! an engram, a saved setting and a sealed GitHub token all survive a stop
//! and a fresh start with empty state and config folders, with the same
//! database, schema and key. A wrong or missing key stops that start and
//! keeps every row. On Turso the state folder is the instance, and a fresh
//! one starts without the accounts and the index.
//!
//! The Postgres tests skip with a note unless `CRYSTALLINE_TEST_POSTGRES_URL`
//! is set. Each runs on a scratch schema of its own ([`PgScratch`]), with the
//! owner timings divided by 10.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

#[cfg(feature = "postgres")]
use crate::pg_owner::{
    NO_SECRET_KEY, failed_start, has_secret_row, instance_id, local_github_files, owner_row,
    run_vars, secret_rows, test_key_id,
};
use crate::pg_owner::{Serve, run_with_stdin, shown_setting, sign_in};
use crate::service::Env;
#[cfg(feature = "postgres")]
use crate::service::PgScratch;

/// The account every test signs in with. Made with `users add`, never from
/// the admin variables, so a fresh start cannot make it again.
const NAME: &str = "alice";
const PASSWORD: &str = "alice-password";

/// The RFC 7636 appendix B pair: the verifier and its S256 challenge.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const REDIRECT: &str = "http://127.0.0.1:33418/callback";

/// The personal access token `connect github --token` seals for the
/// instance.
#[cfg(feature = "postgres")]
const GITHUB_TOKEN: &str = "ghp_redeploy";

/// The token `connect github --personal --as alice` seals for the person.
const PERSONAL_TOKEN: &str = "ghp_personal";

/// A stand-in GitHub on a loopback port: `GET /user` answers the login
/// `alice-gh` for [`PERSONAL_TOKEN`] and `ada-gh` for any other token,
/// everything else 404. It records the path and the `Authorization` header
/// of every request.
struct RecordingGithub {
    url: String,
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    asked: Arc<Mutex<Vec<(String, String)>>>,
    _thread: std::thread::JoinHandle<()>,
}

impl RecordingGithub {
    fn start() -> RecordingGithub {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 16384];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let path = path.split('?').next().unwrap_or("/").to_string();
                let auth = request
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| l.split_once(':').unwrap().1.trim().to_string())
                    .unwrap_or_default();
                seen.lock().unwrap().push((path.clone(), auth.clone()));
                let (status, body) = match path.as_str() {
                    "/user" if auth.ends_with(PERSONAL_TOKEN) => {
                        ("200 OK", r#"{"login":"alice-gh","id":4343}"#)
                    }
                    "/user" => ("200 OK", r#"{"login":"ada-gh","id":4242}"#),
                    _ => ("404 Not Found", r#"{"message":"Not Found"}"#),
                };
                let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT");
                let response = format!(
                    "HTTP/1.1 {status}\r\nDate: {date}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        RecordingGithub {
            url,
            asked,
            _thread: thread,
        }
    }

    /// The host part of the URL, which names the row the token is sealed in.
    #[cfg(feature = "postgres")]
    fn host(&self) -> &str {
        self.url.trim_start_matches("http://")
    }

    /// Whether any request since the last [`RecordingGithub::forget`]
    /// carried `token`.
    #[cfg(feature = "postgres")]
    fn was_asked_with(&self, token: &str) -> bool {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .any(|(_, auth)| auth.ends_with(token))
    }

    #[cfg(feature = "postgres")]
    fn forget(&self) {
        self.asked.lock().unwrap().clear();
    }
}

/// The variables every instance and every command of a deployment gets:
/// collaboration on with GitHub at `github`, and agents signing in, so the
/// OAuth surface is served. No admin variables: the account is made once.
fn deploy_vars(github: &RecordingGithub) -> Vec<(&'static str, String)> {
    vec![
        ("CRYSTALLINE_GITHUB_ENABLED", "true".to_string()),
        ("CRYSTALLINE_GITHUB_API_URL", github.url.clone()),
        ("CRYSTALLINE_AUTH_MCP", "true".to_string()),
    ]
}

fn borrowed<'a>(vars: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    vars.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

/// A client with no cookie jar and redirects off, as an OAuth client is.
fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

/// `value` percent-encoded for a query or a form body.
fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encoded(k), encoded(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The value of `name` in the query of `url`: a request id or a code, hex
/// either way, so nothing needs decoding.
fn query_value(url: &str, name: &str) -> String {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| panic!("no {name} in {url}"))
}

/// A signed-in browser of `NAME` at `serve`: its cookie header and its CSRF
/// token.
fn session(serve: &Serve) -> (String, String) {
    let login = client()
        .post(format!("http://{}/api/v1/auth/login", serve.addr))
        .json(&serde_json::json!({ "name": NAME, "password": PASSWORD }))
        .send()
        .unwrap();
    assert_eq!(login.status().as_u16(), 200, "the sign-in of {NAME}");
    let cookies: Vec<String> = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next().map(str::to_string))
        .collect();
    let body: Value = login.json().unwrap();
    (
        cookies.join("; "),
        body["csrf"].as_str().unwrap().to_string(),
    )
}

/// An OAuth client and the grant it holds: what a redeploy must keep.
struct Grant {
    client_id: String,
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    access_token: String,
    refresh_token: String,
}

/// Register a client at `serve`, walk the authorization with `NAME`'s
/// consent and exchange the code: a completed grant. The pending
/// authorization and the code live in memory only, so all of it happens on
/// one instance.
fn complete_a_grant(serve: &Serve) -> Grant {
    let api = format!("http://{}/api/v1", serve.addr);
    let registered = client()
        .post(format!("{api}/oauth/register"))
        .json(&serde_json::json!({ "client_name": "redeploy", "redirect_uris": [REDIRECT] }))
        .send()
        .unwrap();
    assert_eq!(registered.status().as_u16(), 201);
    let registered: Value = registered.json().unwrap();
    let client_id = registered["client_id"].as_str().unwrap().to_string();

    let query = form(&[
        ("response_type", "code"),
        ("client_id", &client_id),
        ("redirect_uri", REDIRECT),
        ("code_challenge", CHALLENGE),
        ("code_challenge_method", "S256"),
    ]);
    let started = client()
        .get(format!("{api}/oauth/authorize?{query}"))
        .send()
        .unwrap();
    assert_eq!(started.status().as_u16(), 302, "the consent page");
    let consent = started.headers()["location"].to_str().unwrap().to_string();
    let request = query_value(&consent, "request");

    let (cookies, csrf) = session(serve);
    let decided = client()
        .post(format!("{api}/oauth/authorizations/{request}"))
        .header("cookie", cookies)
        .header("x-csrf-token", csrf)
        .json(&serde_json::json!({ "decision": "allow" }))
        .send()
        .unwrap();
    assert_eq!(decided.status().as_u16(), 200, "the consent is allowed");
    let decided: Value = decided.json().unwrap();
    let code = query_value(decided["location"].as_str().unwrap(), "code");

    let exchanged = token_request(
        serve,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", REDIRECT),
            ("code_verifier", VERIFIER),
            ("client_id", &client_id),
        ],
    );
    assert_eq!(exchanged["token_type"], "Bearer", "{exchanged}");
    Grant {
        client_id,
        access_token: exchanged["access_token"].as_str().unwrap().to_string(),
        refresh_token: exchanged["refresh_token"].as_str().unwrap().to_string(),
    }
}

/// `POST /api/v1/oauth/token` at `serve` with `pairs`; panics unless 200.
fn token_request(serve: &Serve, pairs: &[(&str, &str)]) -> Value {
    let response = client()
        .post(format!("http://{}/api/v1/oauth/token", serve.addr))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(pairs))
        .send()
        .unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().unwrap_or(Value::Null);
    assert_eq!(status, 200, "the token endpoint: {body}");
    body
}

/// `crystalline users add NAME --role admin` in `env`.
fn add_the_account(env: &Env) {
    let (ok, out, err) = run_with_stdin(
        env,
        &["users", "add", NAME, "--role", "admin", "--password-stdin"],
        &format!("{PASSWORD}\n"),
    );
    assert!(ok, "users add {NAME}: {out}{err}");
}

/// A first deployment at `serve` in `env`: the account, a completed OAuth
/// grant, a virtual domain with one engram and a saved setting. On
/// Postgres also two GitHub tokens sealed in the pod (`seal_token`): the
/// instance's and the person's.
fn first_deploy(env: &Env, serve: &Serve, vars: &[(&str, &str)], seal_token: bool) -> Grant {
    add_the_account(env);
    assert_eq!(sign_in(serve, NAME, PASSWORD), 200);
    let grant = complete_a_grant(serve);
    let (ok, out, err) = env.run_full(&["domain", "add", "--virtual", "tides"]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = env.run_full(&[
        "write",
        "tides",
        "Tide Tables",
        "--content",
        "High water comes at noon.",
    ]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = env.run_full(&["config", "set", "service.response_format", "json"]);
    assert!(ok, "{out}{err}");
    #[cfg(feature = "postgres")]
    if seal_token {
        let (ok, out, err) = run_vars(env, &["connect", "github", "--token", GITHUB_TOKEN], vars);
        assert!(ok, "{out}{err}");
        assert!(out.contains("(vault token store)"), "{out}");
        let (ok, out, err) = run_vars(
            env,
            &[
                "connect",
                "github",
                "--personal",
                "--as",
                NAME,
                "--token",
                PERSONAL_TOKEN,
            ],
            vars,
        );
        assert!(ok, "{out}{err}");
        assert!(out.contains("(vault token store)"), "{out}");
    }
    #[cfg(not(feature = "postgres"))]
    let _ = (vars, seal_token);
    grant
}

/// Remove the state and config folders of `env`: the empty disk of a
/// redeployed container.
#[cfg(feature = "postgres")]
fn empty_the_disk(env: &Env) {
    std::fs::remove_dir_all(env.state_dir()).unwrap();
    let config_dir = env.config_path().parent().unwrap().to_path_buf();
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir).unwrap();
    }
    assert!(!env.state_dir().exists() && !config_dir.exists());
}

/// The rows a first deployment seals in `pg`: the instance's token and the
/// person's.
#[cfg(feature = "postgres")]
fn sealed_rows_of(pg: &PgScratch, github: &RecordingGithub) -> [bool; 2] {
    [
        has_secret_row(pg, &format!("github:{}", github.host())),
        has_secret_row(pg, &format!("github-personal:{NAME}:{}", github.host())),
    ]
}

/// The durable-state line of a start that took over `pg`: it names the key
/// and the two sealed rows, the App source, and no row name or token.
#[cfg(feature = "postgres")]
fn assert_start_line(log: &str) {
    let line = log
        .lines()
        .find(|l| l.contains("durable state: owner of postgres"))
        .unwrap_or_else(|| panic!("no durable state line:\n{log}"));
    assert!(
        line.contains(&format!(
            "secrets sealed in postgres with key {} (2 rows)",
            test_key_id()
        )),
        "{line}"
    );
    assert!(line.ends_with("; GitHub App none"), "{line}");
    assert!(!log.contains("github:"), "a row name in the log:\n{log}");
    assert!(
        !log.contains("github-personal"),
        "a row name in the log:\n{log}"
    );
    assert!(!log.contains(GITHUB_TOKEN), "the token in the log");
    assert!(!log.contains(PERSONAL_TOKEN), "the token in the log");
}

/// The admin's GitHub settings at `serve`.
#[cfg(feature = "postgres")]
fn github_settings(serve: &Serve) -> Value {
    let (cookies, _) = session(serve);
    client()
        .get(format!("http://{}/api/v1/settings/github", serve.addr))
        .header("cookie", cookies)
        .send()
        .unwrap()
        .json()
        .unwrap()
}

/// Everything [`first_deploy`] made, asked of `serve` and of the commands
/// of `env` (the instance that serves now): the grant refreshes to a new
/// pair, the account signs in, the domain is listed and its engram read,
/// the setting holds, the instance's sealed token is opened and used for a
/// call to GitHub, and the person's sealed token is opened for them.
#[cfg(feature = "postgres")]
fn assert_everything_kept(env: &Env, serve: &Serve, grant: &Grant, github: &RecordingGithub) {
    // The refresh grant: a new pair. No `resource`, as a client sends it:
    // the grant keeps the audience it was issued for.
    let refreshed = token_request(
        serve,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &grant.refresh_token),
            ("client_id", &grant.client_id),
        ],
    );
    let access = refreshed["access_token"].as_str().unwrap();
    let refresh = refreshed["refresh_token"].as_str().unwrap();
    assert!(access.starts_with("coa_") && access != grant.access_token);
    assert!(refresh.starts_with("cor_") && refresh != grant.refresh_token);

    assert_eq!(sign_in(serve, NAME, PASSWORD), 200, "the account is kept");
    assert_eq!(sign_in(serve, NAME, "not-the-password"), 401);

    let (ok, out, err) = env.run_full(&["--json", "domain", "list"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("tides"), "the domain is listed: {out}");
    let (ok, out, err) = env.run_full(&["read", "tide-tables", "--domain", "tides"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("High water comes at noon."), "{out}");

    assert_eq!(shown_setting(env, "service.response_format"), "json");
    assert!(!env.config_path().exists(), "no config.yaml was written");

    // The token: opened from the vault, and sent to GitHub by the daemon.
    let settings = github_settings(serve);
    assert_eq!(settings["token_store"], "vault", "{settings}");
    assert_eq!(
        settings["user"], "ada-gh",
        "the token is opened: {settings}"
    );
    github.forget();
    let (cookies, _) = session(serve);
    let _ = client()
        .get(format!(
            "http://{}/api/v1/github/domain-name?repo=acme/team",
            serve.addr
        ))
        .header("cookie", cookies)
        .send()
        .unwrap();
    assert!(
        github.was_asked_with(GITHUB_TOKEN),
        "the daemon called GitHub with the sealed token: {:?}",
        github.asked.lock().unwrap()
    );
    // The person's token: its row opens, and the person sees their login.
    let (cookies, _) = session(serve);
    let identity: Value = client()
        .get(format!("http://{}/api/v1/me/github-identity", serve.addr))
        .header("cookie", cookies)
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(identity["connected"], true, "{identity}");
    assert_eq!(identity["login"], "alice-gh", "{identity}");
    assert_eq!(local_github_files(env), Vec::<std::path::PathBuf>::new());
}

/// How many accounts `pg` holds, as text.
#[cfg(feature = "postgres")]
fn accounts(pg: &PgScratch) -> String {
    pg.text(&format!("SELECT count(*)::text FROM {}.users", pg.schema))
}

/// A first deployment that stops gracefully, then the empty disk: the
/// state a redeploy starts from. Answers the grant.
#[cfg(feature = "postgres")]
fn deployed_and_stopped(
    pg: &PgScratch,
    env: &Env,
    github: &RecordingGithub,
    vars: &[(&str, &str)],
) -> Grant {
    let mut serve = Serve::start_with(env, Some("info"), vars);
    serve.wait_ready();
    let grant = first_deploy(env, &serve, vars, true);
    assert_eq!(sealed_rows_of(pg, github), [true, true]);
    assert_eq!(secret_rows(pg), "2");
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));
    empty_the_disk(env);
    grant
}

/// A redeploy keeps everything: stopped gracefully, its state and config
/// folders deleted, a fresh instance on the same database, schema and key
/// serves the grant, the account, the domain with its engram, the setting
/// and the sealed token. Its start line names the key and the rows.
#[cfg(feature = "postgres")]
#[test]
fn redeploy_keeps_everything() {
    let Some(pg) = PgScratch::new("rdkeep") else {
        return;
    };
    let github = RecordingGithub::start();
    let env = Env::new("rdkeep").on_postgres(&pg);
    let vars = deploy_vars(&github);
    let vars = borrowed(&vars);
    let grant = deployed_and_stopped(&pg, &env, &github, &vars);
    assert!(!env.state_dir().exists());

    let mut serve = Serve::start_with(&env, Some("info"), &vars);
    serve.wait_ready();
    assert_everything_kept(&env, &serve, &grant, &github);
    assert_start_line(&serve.log());
    assert_eq!(sealed_rows_of(&pg, &github), [true, true]);
    assert_eq!(accounts(&pg), "1", "the fresh start made no account");
}

/// The same after a crash: the first instance is killed with SIGKILL, a
/// standby in a state folder of its own takes over after the grace, and it
/// serves everything the first one made.
#[cfg(feature = "postgres")]
#[test]
fn redeploy_after_kill_with_standby() {
    let Some(pg) = PgScratch::new("rdkill") else {
        return;
    };
    let github = RecordingGithub::start();
    let vars = deploy_vars(&github);
    let vars = borrowed(&vars);
    let active_env = Env::new("rdkilla").on_postgres(&pg);
    let mut active = Serve::start_with(&active_env, Some("info"), &vars);
    active.wait_ready();
    let standby_env = Env::new("rdkills").on_postgres(&pg);
    let mut standby = Serve::start_with(&standby_env, Some("info"), &vars);
    standby.wait_role("standby");
    let grant = first_deploy(&active_env, &active, &vars, true);
    assert_eq!(sealed_rows_of(&pg, &github), [true, true]);

    unsafe {
        libc::kill(active.pid(), libc::SIGKILL);
    }
    let _ = active.wait_exit(Duration::from_secs(10));
    empty_the_disk(&active_env);
    standby.wait_ready();
    assert_eq!(
        owner_row(&pg),
        format!("{}|false", instance_id(&standby_env))
    );
    let log = standby.log();
    assert!(log.contains("after a grace of"), "{log}");
    assert_start_line(&log);
    assert_everything_kept(&standby_env, &standby, &grant, &github);
}

/// A redeploy with another key stops after the takeover, naming the id of
/// the key the rows were sealed with and of the one given, and keeps every
/// row. The right key then serves everything again.
#[cfg(feature = "postgres")]
#[test]
fn redeploy_with_wrong_key_fails() {
    use crystalline_remote::sealed::SealKey;

    let Some(pg) = PgScratch::new("rdwrong") else {
        return;
    };
    let github = RecordingGithub::start();
    let env = Env::new("rdwrong").on_postgres(&pg);
    let vars = deploy_vars(&github);
    let vars = borrowed(&vars);
    let grant = deployed_and_stopped(&pg, &env, &github, &vars);
    let (rows, people) = (secret_rows(&pg), accounts(&pg));

    let other = "11".repeat(32);
    let other_id = SealKey::from_bytes([0x11; 32]).key_id();
    let mut extra = vars.clone();
    extra.push(("CRYSTALLINE_SECRET_KEY", other.as_str()));
    let (code, log) = failed_start(&env, &extra);
    assert_ne!(code, Some(0), "{log}");
    assert!(log.contains("CRYSTALLINE_SECRET_KEY"), "{log}");
    assert!(log.contains(&test_key_id()), "the rows' key id: {log}");
    assert!(log.contains(&other_id), "the given key id: {log}");
    assert!(!log.contains(&other), "the key itself is never logged");
    assert_eq!(secret_rows(&pg), rows, "the rows are kept:\n{log}");
    assert_eq!(accounts(&pg), people, "the accounts are kept");

    let mut serve = Serve::start_with(&env, Some("info"), &vars);
    serve.wait_ready();
    assert_everything_kept(&env, &serve, &grant, &github);
}

/// A redeploy without the key stops before it takes the database, naming
/// the variable, and keeps every row. The key then serves everything again.
#[cfg(feature = "postgres")]
#[test]
fn redeploy_with_missing_key_fails() {
    let Some(pg) = PgScratch::new("rdmiss") else {
        return;
    };
    let github = RecordingGithub::start();
    let env = Env::new("rdmiss").on_postgres(&pg);
    let vars = deploy_vars(&github);
    let vars = borrowed(&vars);
    let grant = deployed_and_stopped(&pg, &env, &github, &vars);
    let (rows, people, owner) = (secret_rows(&pg), accounts(&pg), owner_row(&pg));

    let mut extra = vars.clone();
    extra.extend(NO_SECRET_KEY);
    let (code, log) = failed_start(&env, &extra);
    assert_ne!(code, Some(0), "{log}");
    assert!(log.contains("CRYSTALLINE_SECRET_KEY"), "{log}");
    assert!(log.contains("openssl rand -base64 32"), "{log}");
    assert_eq!(secret_rows(&pg), rows, "the rows are kept:\n{log}");
    assert_eq!(accounts(&pg), people, "the accounts are kept");
    assert_eq!(owner_row(&pg), owner, "the keyless start never took over");

    let mut serve = Serve::start_with(&env, Some("info"), &vars);
    serve.wait_ready();
    assert_everything_kept(&env, &serve, &grant, &github);
}

/// On Turso the state folder is the instance, and that stays so: a fresh
/// state folder starts with no accounts and no index, so the account no
/// longer signs in and the virtual domain's engram is gone, while
/// config.yaml in the config folder still holds the setting and the
/// domain's registration.
#[test]
fn per_state_dir_behavior_pinned() {
    let github = RecordingGithub::start();
    let env = Env::new("rdturso");
    let vars = deploy_vars(&github);
    let vars = borrowed(&vars);
    let mut serve = Serve::start_with(&env, Some("info"), &vars);
    serve.wait_ready();
    let grant = first_deploy(&env, &serve, &vars, false);
    assert!(env.state_dir().join("web-auth.db").exists());
    assert!(
        env.config_path().exists(),
        "the setting went to config.yaml"
    );
    serve.terminate();
    assert_eq!(serve.wait_exit(Duration::from_secs(15)), Some(0));

    std::fs::remove_dir_all(env.state_dir()).unwrap();
    let mut serve = Serve::start_with(&env, Some("info"), &vars);
    serve.wait_ready();
    assert_eq!(
        sign_in(&serve, NAME, PASSWORD),
        401,
        "the accounts lived in the state folder"
    );
    let refreshed = client()
        .post(format!("http://{}/api/v1/oauth/token", serve.addr))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &grant.refresh_token),
            ("client_id", &grant.client_id),
        ]))
        .send()
        .unwrap();
    assert_eq!(
        refreshed.status().as_u16(),
        400,
        "the grant lived in the state folder"
    );
    assert_eq!(shown_setting(&env, "service.response_format"), "json");
    let (ok, out, err) = env.run_full(&["--json", "domain", "list"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("tides"), "config.yaml registers it: {out}");
    let (ok, out, err) = env.run_full(&["read", "tide-tables", "--domain", "tides"]);
    assert!(
        !ok || !out.contains("High water comes at noon."),
        "the index lived in the state folder: {out}{err}"
    );
    assert!(!serve.log().contains("durable state"), "no block on Turso");
}
