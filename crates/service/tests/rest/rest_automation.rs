//! `GET /api/v1/domains/{domain}/automation`: when the loops last ran, what
//! waits for a person and whether the trigger webhook works, read with the
//! session cookie by the Fluid pages.
//!
//! Driven over a live listener through the production router. The runs are
//! recorded through the engine, the way a runner's lease and record verbs do.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::automation::{Loop, RunOutcome, RunReport};
use crystalline_core::config::{
    AuthConfig, AutomationConfig, DomainEntry, GlobalConfig, WebhookConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

struct Fixture {
    addr: std::net::SocketAddr,
    engine: Arc<Engine>,
    auth: Arc<AuthStore>,
    _state: crate::support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

fn manifest(extra: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n{extra}---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n"
    )
}

/// `eng` with the evolve webhook trigger on and a webhook URL without a
/// secret, and a viewer `ada`.
async fn serve() -> Fixture {
    serve_with(false).await
}

/// `anonymous` serves a caller who carries nothing as a viewer.
async fn serve_with(anonymous: bool) -> Fixture {
    let state = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        manifest("evolve_trigger: webhook\n"),
    )
    .unwrap();
    // A mechanical finding (V103: a summary the full text never mentions) and
    // a judgment one (V006: a human capture nobody reviewed).
    let yesterday = (crystalline_core::time::today_utc() - crystalline_core::time::days(1)).ymd();
    let note = |title: &str, permalink: &str, by: Option<&str>, body: &str| {
        let generated = by
            .map(|by| format!("generated: {{ by: \"{by}\", at: {yesterday}T09:12:00+00:00 }}\n"))
            .unwrap_or_default();
        format!(
            "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - reference\nstatus: stable\nrecorded_at: {yesterday}\n{generated}---\n\n# {title}\n\n{body}\n"
        )
    };
    for (file, text) in [
        (
            "summary-doc.md",
            note(
                "Summary doc",
                "summary-doc",
                None,
                "The distilled version of the migration write-up.\n\n- summarizes [[Full text doc]]\n- [context] written after the migration closed",
            ),
        ),
        (
            "full-text-doc.md",
            note(
                "Full text doc",
                "full-text-doc",
                None,
                "The full migration write-up with the shard tables.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary",
            ),
        ),
        (
            "capture.md",
            note(
                "Incident capture",
                "capture",
                Some("human:jordi"),
                "Written straight after the incident call.\n\n- relates_to [[Full text doc]]\n- [context] nobody has read it back",
            ),
        ),
    ] {
        std::fs::write(dir.join(file), text).unwrap();
    }
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            trusted_header: None,
            proxy_headers: None,
            anonymous: Some(anonymous),
            mcp: None,
            oauth: None,
            max_users: None,
            oidc: None,
            login: None,
            invites: None,
        }),
        automation: Some(AutomationConfig {
            webhook: Some(WebhookConfig {
                url: Some("https://hooks.example.test/crystalline".to_string()),
                secret: None,
            }),
            evolve_threshold: None,
        }),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();
    auth.add_user("root", "Root", None, Role::Admin, "s3cret")
        .await
        .unwrap();
    let router = http_router(
        engine.clone(),
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        addr,
        engine,
        auth,
        _state: state,
        _tmp: tmp,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

async fn login(addr: std::net::SocketAddr) -> String {
    login_as(addr, "ada").await
}

async fn login_as(addr: std::net::SocketAddr, name: &str) -> String {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({ "name": name, "password": "s3cret" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .and_then(|v| v.split(';').next())
        .and_then(|v| v.strip_prefix("fluid_session="))
        .unwrap()
        .to_string()
}

async fn read(addr: std::net::SocketAddr, cookie: Option<&str>, domain: &str) -> (u16, Value) {
    read_query(addr, cookie, domain, "").await
}

async fn read_query(
    addr: std::net::SocketAddr,
    cookie: Option<&str>,
    domain: &str,
    query: &str,
) -> (u16, Value) {
    let mut req = client().get(format!(
        "http://{addr}/api/v1/domains/{domain}/automation{query}"
    ));
    if let Some(cookie) = cookie {
        req = req.header("cookie", format!("fluid_session={cookie}"));
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

async fn record_done(fx: &Fixture, lp: Loop, summary: &str) {
    let owner = Scope::Unrestricted;
    let token = fx
        .engine
        .automation_lease_take(lp, "eng", "n8n", &owner)
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    fx.engine
        .automation_run_record(
            lp,
            "eng",
            &token,
            &RunReport {
                started_at: String::new(),
                outcome: RunOutcome::Done,
                done: 2,
                summary: summary.to_string(),
            },
            &owner,
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loop_that_never_ran_has_no_last_run() {
    let fx = serve().await;
    let cookie = login_as(fx.addr, "root").await;
    let (status, body) = read(fx.addr, Some(&cookie), "eng").await;
    assert_eq!(status, 200, "{body}");
    for lp in ["reflect", "evolve"] {
        assert!(body[lp]["last_run"].is_null(), "{lp}: {body}");
        assert!(body[lp]["running"].is_null(), "{lp}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_sees_the_run_without_its_text_and_nothing_counted() {
    let fx = serve().await;
    record_done(&fx, Loop::Evolve, "fixed two").await;
    let cookie = login(fx.addr).await;
    let (status, body) = read(fx.addr, Some(&cookie), "eng").await;
    assert_eq!(status, 200, "{body}");
    let run = &body["evolve"]["last_run"];
    assert_eq!(run["runner"], "n8n", "{body}");
    assert_eq!(run["outcome"], "done", "{body}");
    assert_eq!(run["done"], 2, "{body}");
    for hidden in ["summary", "pr_url", "account", "left_items"] {
        assert!(
            run.get(hidden).is_none_or(Value::is_null),
            "{hidden}: {body}"
        );
    }
    assert!(body["evolve"]["left_for_person"].is_null(), "{body}");
    assert!(body["evolve"]["judgment_waiting"].is_null(), "{body}");
    assert!(body["evolve"]["delivery"].is_null(), "{body}");
    assert!(body["reflect"]["last_run"].is_null(), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_who_may_run_the_loop_gets_the_text_and_the_counts() {
    let fx = serve().await;
    record_done(&fx, Loop::Evolve, "fixed two").await;
    let cookie = login_as(fx.addr, "root").await;
    let (status, body) = read(fx.addr, Some(&cookie), "eng").await;
    assert_eq!(status, 200, "{body}");
    let evolve = &body["evolve"];
    assert_eq!(evolve["last_run"]["summary"], "fixed two", "{body}");
    assert!(evolve["last_run"].get("left_items").is_none(), "{body}");
    // The mechanical finding the run did not fix is left for a person.
    assert_eq!(evolve["left_for_person"], 1, "{body}");
    assert!(evolve["judgment_waiting"].as_u64().unwrap() > 0, "{body}");

    // Without counts the sweep is skipped and the fields say so.
    let (_, light) = read_query(fx.addr, Some(&cookie), "eng", "?counts=false&loop=evolve").await;
    assert!(light["evolve"]["left_for_person"].is_null(), "{light}");
    assert!(light["evolve"]["judgment_waiting"].is_null(), "{light}");
    assert_eq!(
        light["evolve"]["last_run"]["summary"], "fixed two",
        "{light}"
    );
    assert!(
        light["reflect"].is_null(),
        "only the loop asked for: {light}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_lease_shows_as_running() {
    let fx = serve().await;
    fx.engine
        .automation_lease_take(Loop::Evolve, "eng", "k8s", &Scope::Unrestricted)
        .await
        .unwrap();
    let cookie = login(fx.addr).await;
    let (_, body) = read(fx.addr, Some(&cookie), "eng").await;
    assert_eq!(body["evolve"]["running"]["runner"], "k8s", "{body}");
    assert!(body["evolve"]["running"].get("account").is_none(), "{body}");
    assert!(body["reflect"]["running"].is_null(), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_manager_or_admin_sees_the_delivery() {
    let fx = serve().await;
    fx.engine.record_delivery(
        "eng",
        Loop::Evolve,
        Err("the receiver refused it".to_string()),
    );
    let viewer = login(fx.addr).await;
    let (_, body) = read(fx.addr, Some(&viewer), "eng").await;
    assert!(body["evolve"]["delivery"].is_null(), "viewer: {body}");

    let admin = login_as(fx.addr, "root").await;
    let (_, body) = read(fx.addr, Some(&admin), "eng").await;
    assert!(body["reflect"]["delivery"].is_null(), "off: {body}");
    let delivery = &body["evolve"]["delivery"];
    assert_eq!(delivery["configured"], false, "{body}");
    assert_eq!(delivery["last_error"], "the receiver refused it", "{body}");
    assert!(delivery["last_sent"].is_null(), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_url_without_a_secret_says_so_before_anything_was_tried() {
    let fx = serve().await;
    let admin = login_as(fx.addr, "root").await;
    let (_, body) = read(fx.addr, Some(&admin), "eng").await;
    let delivery = &body["evolve"]["delivery"];
    assert_eq!(delivery["configured"], false, "{body}");
    assert_eq!(
        delivery["last_error"],
        crystalline_service::engine::WEBHOOK_NO_SECRET,
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_anonymous_visitor_of_a_public_domain_gets_the_reader_share() {
    let fx = serve_with(true).await;
    record_done(&fx, Loop::Evolve, "fixed two").await;
    let (status, body) = read(fx.addr, None, "eng").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["evolve"]["last_run"]["outcome"], "done", "{body}");
    assert!(
        body["evolve"]["last_run"]
            .get("summary")
            .is_none_or(Value::is_null),
        "{body}"
    );
    assert!(body["evolve"]["delivery"].is_null(), "{body}");
    assert!(body["evolve"]["left_for_person"].is_null(), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_domain_a_missing_session_and_a_bad_loop_are_refused() {
    let fx = serve().await;
    let cookie = login(fx.addr).await;
    let (unknown, _) = read(fx.addr, Some(&cookie), "nope").await;
    assert_eq!(unknown, 404);
    let (anonymous, _) = read(fx.addr, None, "eng").await;
    assert_eq!(anonymous, 401);
    let (bad, _) = read_query(fx.addr, Some(&cookie), "eng", "?loop=sing").await;
    assert_eq!(bad, 400);
}

/// A run by an agent of the loops shows the agent's unique name and its display
/// name, read when the answer is made, and no account; a viewer sees neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_by_an_agent_shows_the_agent_and_its_display_name() {
    use crystalline_service::rest::AgentLoops;
    let fx = serve().await;
    fx.auth
        .add_agent(
            "nightly",
            Some("Nightly Curator"),
            AgentLoops::Both,
            "cli",
            "",
            30 * 86_400,
        )
        .await
        .unwrap();
    let nightly = Scope::Agent {
        name: "nightly".to_string(),
        loops: AgentLoops::Both,
        token_id: 1,
        token_expires_at: "2099-01-01T00:00:00Z".to_string(),
    };
    let token = fx
        .engine
        .automation_lease_take(Loop::Evolve, "eng", "n8n", &nightly)
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    fx.engine
        .automation_run_record(
            Loop::Evolve,
            "eng",
            &token,
            &RunReport {
                started_at: String::new(),
                outcome: RunOutcome::Done,
                done: 1,
                summary: "fixed one".to_string(),
            },
            &nightly,
        )
        .await
        .unwrap();

    let cookie = login_as(fx.addr, "root").await;
    let (status, body) = read(fx.addr, Some(&cookie), "eng").await;
    assert_eq!(status, 200, "{body}");
    let run = &body["evolve"]["last_run"];
    assert_eq!(run["agent"], "nightly", "{body}");
    assert_eq!(run["agent_display"], "Nightly Curator", "{body}");
    assert!(run["account"].is_null(), "{body}");

    let cookie = login(fx.addr).await;
    let (_, body) = read(fx.addr, Some(&cookie), "eng").await;
    let run = &body["evolve"]["last_run"];
    for hidden in ["agent", "agent_display", "account"] {
        assert!(
            run.get(hidden).is_none_or(Value::is_null),
            "{hidden}: {body}"
        );
    }
}
