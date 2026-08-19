//! Endpoint tests for the read-only evolve queue at `GET /api/v1/evolve`, the
//! surface the Fluid maintenance page reads.
//!
//! Driven over a live listener through the production router construction, like
//! the rest of the REST suite, so the mount point and the shared auth layers are
//! exercised rather than a hand-built sub-router. The fixture plants human
//! captures nobody has reviewed, which is the finding a person opening the page
//! is meant to see first.
//!
//! Every fixture holds a [`support::ScratchStateDir`]: the run recorder writes
//! under the state directory, and the point of the last test here is that this
//! route never writes there at all - which is only worth asserting if a failure
//! could not touch the developer's own state directory.

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{AuthConfig, DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use tokio::sync::Mutex;

/// What an evolve-test server varies.
#[derive(Default)]
struct Options {
    /// `auth.anonymous`: serve a request that carries no identity.
    anonymous: bool,
}

struct Fixture {
    addr: std::net::SocketAddr,
    auth: Arc<AuthStore>,
    /// Held for the test's duration: it redirects the state directory the
    /// maintenance file lives in into a scratch home, so nothing this suite
    /// does can reach the developer's own.
    state: support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

/// The day before today, which is what makes the fixture's captures visible to
/// `V006`: the rule leaves an engram recorded today alone, because it is still
/// being worked on. The route never takes a `today` parameter, so the fixture
/// dates itself against the real clock rather than pinning one.
fn yesterday() -> chrono::NaiveDate {
    chrono::Utc::now().date_naive() - chrono::Duration::days(1)
}

/// One engram file's markdown. `generated_by` writes the provenance block a
/// human capture carries; `None` leaves it out, which is what an engram nobody
/// claimed looks like.
fn engram(title: &str, permalink: &str, generated_by: Option<&str>, body: &str) -> String {
    let day = yesterday();
    // The flow form Crystalline's own emitter writes, so an edit that refreshes
    // the provenance line (every write does) rewrites one line rather than
    // orphaning an indented block.
    let generated = generated_by
        .map(|by| format!("generated: {{ by: \"{by}\", at: {day}T09:12:00+00:00 }}\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - reference\nstatus: stable\nrecorded_at: {day}\n{generated}---\n\n{body}"
    )
}

/// Serve the production router over a fixture the sweep has something to say
/// about: two engrams a person captured yesterday and nobody reviewed, each
/// linked into a live reference so the orphan and stub rules stay quiet, plus a
/// second registered domain that holds nothing.
async fn serve(opts: Options) -> Fixture {
    let state = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            trusted_header: None,
            anonymous: Some(opts.anonymous),
            max_users: None,
        }),
        ..GlobalConfig::default()
    };

    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("human-capture.md"),
        engram(
            "Incident capture",
            "human-capture",
            Some("human:jordi"),
            "Written straight after the incident call, in the words the responder used.\n\n- relates_to [[Live doc]]\n- [context] nobody has read it back since the call\n",
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("field-capture.md"),
        engram(
            "Field capture",
            "field-capture",
            Some("human:dominique"),
            "Dictated on the way back from the customer site, before anything was tidied up.\n\n- relates_to [[Live doc]]\n- [context] the wording is the customer's own\n",
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("live-doc.md"),
        engram(
            "Live doc",
            "live-doc",
            None,
            "The current reference for the migration, still cited by the runbooks.\n\n- [context] both captures point here\n- [lesson] keep the reference pointed at what holds now\n",
        ),
    )
    .unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    // A registered domain with nothing in it, so scoping the sweep to a name
    // that holds no knowledge is answered with an empty queue rather than an
    // error. Virtual, because a file domain always has its MANIFEST indexed.
    cfg.domains
        .insert("other".to_string(), DomainEntry::virtual_domain());

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
    let router = http_router(
        engine,
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
        auth,
        state,
        _tmp: tmp,
    }
}

/// The anonymous fixture: the shortest way past the guard and into the handler.
async fn serve_anonymous() -> Fixture {
    serve(Options { anonymous: true }).await
}

/// A client with proxy discovery disabled: the target is loopback, where a
/// system proxy must never be consulted anyway.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// GET a path off the test server.
async fn get(addr: std::net::SocketAddr, path: &str) -> reqwest::Response {
    client()
        .get(format!("http://{addr}{path}"))
        .send()
        .await
        .unwrap()
}

/// The queue as JSON, asserting the 200 on the way through.
async fn queue(addr: std::net::SocketAddr, query: &str) -> serde_json::Value {
    let resp = get(addr, &format!("/api/v1/evolve{query}")).await;
    assert_eq!(resp.status(), 200, "GET /api/v1/evolve{query}");
    resp.json().await.unwrap()
}

/// Log in, returning the session cookie value.
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> String {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({ "name": name, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login as {name} must succeed");
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

/// The rows of a queue payload.
fn rows(body: &serde_json::Value) -> &Vec<serde_json::Value> {
    body["queue"].as_array().expect("the queue is an array")
}

/// The whole envelope the page reads, over a domain carrying human captures
/// nobody has reviewed: the findings themselves plus the per-rule legend that
/// says what to do about each one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queue_answers_with_findings_and_legend() {
    let fixture = serve_anonymous().await;
    let body = queue(fixture.addr, "").await;

    for key in [
        "scope",
        "engrams_scanned",
        "total",
        "page",
        "limit",
        "count",
        "families",
        "queue",
        "actions",
        "guidance",
        "truncations",
    ] {
        assert!(
            body.get(key).is_some(),
            "the engine's envelope carries `{key}`: {body}"
        );
    }
    assert!(
        body["engrams_scanned"].as_u64().unwrap() >= 4,
        "the manifest and the three engrams were scanned: {body}"
    );

    let queue = rows(&body);
    assert!(!queue.is_empty(), "the fixture has work waiting: {body}");
    let human = queue
        .iter()
        .find(|row| row["rule"] == "V006" && row["permalink"] == "human-capture")
        .unwrap_or_else(|| panic!("the human capture is in the queue: {body}"));
    assert_eq!(
        human["class"], "judgment",
        "reviewing a person's words is a judgment call"
    );
    assert_eq!(human["domain"], "eng");
    assert_eq!(human["title"], "Incident capture");
    assert!(
        human["evidence"].as_str().unwrap().contains("human:jordi"),
        "the evidence names who captured it: {human}"
    );
    assert!(
        human["priority"].as_u64().unwrap() > 0,
        "every row is ranked: {human}"
    );

    let instruction = body["actions"]
        .as_array()
        .expect("actions is an array")
        .iter()
        .find(|action| action["rule"] == "V006")
        .map(|action| action["instruction"].as_str().unwrap().to_string())
        .unwrap_or_else(|| panic!("the legend carries V006: {body}"));
    assert_eq!(
        instruction,
        crystalline_index::rule_info("V006").unwrap().instruction,
        "the legend is the catalog's own instruction, passed through unchanged"
    );

    // The families summary counts the whole result rather than the page, which
    // is what the page's section headings are drawn from.
    let temporal = body["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["family"] == "temporal")
        .unwrap_or_else(|| panic!("the temporal family is counted: {body}"));
    assert_eq!(temporal["findings"], body["total"]);
}

/// The list parameters and the page window reach the engine: a scope naming a
/// domain that holds nothing answers an empty queue, a family nothing fired in
/// answers an empty queue, and a second page of one carries the second-ranked
/// row with its rank across the whole result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoping_and_paging_pass_through() {
    let fixture = serve_anonymous().await;

    let scoped = queue(fixture.addr, "?domains=other").await;
    assert_eq!(
        scoped["scope"]["domains"],
        serde_json::json!(["other"]),
        "the scope is echoed back: {scoped}"
    );
    assert_eq!(scoped["total"], 0);
    assert!(
        rows(&scoped).is_empty(),
        "an empty domain is quiet: {scoped}"
    );

    let by_family = queue(fixture.addr, "?families=structure").await;
    assert_eq!(
        by_family["total"], 0,
        "nothing structural fired on this fixture: {by_family}"
    );
    let by_rule = queue(fixture.addr, "?rules=V006").await;
    let all = queue(fixture.addr, "").await;
    assert_eq!(
        by_rule["total"], 2,
        "both captures are waiting, which is what makes a second page real: {by_rule}"
    );
    assert!(
        rows(&by_rule).iter().all(|row| row["rule"] == "V006"),
        "a rule filter narrows to that rule: {by_rule}"
    );
    assert!(
        all["total"].as_u64().unwrap() > by_rule["total"].as_u64().unwrap(),
        "and the unfiltered sweep sees more than the one rule does: {all}"
    );

    let second = queue(fixture.addr, "?limit=1&page=2").await;
    assert_eq!(second["limit"], 1);
    assert_eq!(second["page"], 2);
    assert_eq!(second["count"], 1);
    assert_eq!(
        second["total"], all["total"],
        "paging never changes the total"
    );
    assert_eq!(
        rows(&second)[0]["n"],
        2,
        "the rank is across the whole result, not within the page: {second}"
    );

    let above_everything = queue(fixture.addr, "?min_priority=100").await;
    assert_eq!(
        above_everything["total"], 0,
        "the priority floor reaches the engine: {above_everything}"
    );
}

/// The queue is read like any other read on this surface: no identity is 401 on
/// a closed instance, a signed-in viewer is served, and an anonymous instance
/// serves a caller who carries nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_matches_engram_reads() {
    let fixture = serve(Options::default()).await;
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Viewer, "s3cret")
        .await
        .unwrap();

    let resp = get(fixture.addr, "/api/v1/evolve").await;
    assert_eq!(resp.status(), 401, "a closed instance asks for an identity");
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 401);

    let token = login(fixture.addr, "ada", "s3cret").await;
    let resp = client()
        .get(format!("http://{}/api/v1/evolve", fixture.addr))
        .header("cookie", format!("fluid_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "a viewer reads the queue");

    let anonymous = serve_anonymous().await;
    let resp = get(anonymous.addr, "/api/v1/evolve").await;
    assert_eq!(
        resp.status(),
        200,
        "an anonymous instance serves the queue to a caller who carries nothing"
    );
}

/// Looking at the queue is not doing the work: the route runs detection only, so
/// the maintenance state file - the record a Stop hook nudges from - is byte for
/// byte what it was before the request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewing_the_queue_never_counts_as_a_run() {
    let fixture = serve_anonymous().await;
    let path = fixture.state.maintenance_path();
    assert!(
        path.starts_with(fixture.state.home()),
        "the maintenance file must resolve inside the scratch home, not the developer's: {}",
        path.display()
    );

    // A backlog a sweep would settle, so a run that did happen would be visible
    // as a cleared list and a stamped `last_run_at` rather than only as a file
    // appearing.
    crystalline_service::maintenance::save(&crystalline_service::maintenance::MaintenanceState {
        pending_domains: vec!["eng".to_string()],
        pending_since: Some("2026-08-01T10:00:00Z".parse().unwrap()),
        ..Default::default()
    })
    .unwrap();
    let before = std::fs::read(&path).unwrap();

    queue(fixture.addr, "").await;
    queue(fixture.addr, "?domains=eng").await;

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "viewing the queue wrote to the maintenance state"
    );
    let state = crystalline_service::maintenance::load();
    assert_eq!(state.pending_domains, vec!["eng".to_string()]);
    assert!(
        state.last_run_at.is_none(),
        "nothing here counts as a sweep having run"
    );
}

/// Log in, returning (session cookie value, CSRF token): the pair a browser
/// carries on every write.
async fn login_session(
    addr: std::net::SocketAddr,
    name: &str,
    password: &str,
) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({ "name": name, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login as {name} must succeed");
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .and_then(|v| v.split(';').next())
        .and_then(|v| v.strip_prefix("fluid_session="))
        .unwrap()
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_string())
}

/// A request to the ack endpoint carrying a session and its token.
fn ack_request(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    session: &(String, String),
    body: serde_json::Value,
) -> reqwest::RequestBuilder {
    client()
        .request(
            method,
            format!("http://{addr}/api/v1/domains/eng/evolve/ack"),
        )
        .header("cookie", format!("fluid_session={}", session.0))
        .header("x-csrf-token", &session.1)
        .json(&body)
}

/// The queue read with a session, for a closed instance.
async fn queue_as(
    addr: std::net::SocketAddr,
    session: &(String, String),
    query: &str,
) -> serde_json::Value {
    let resp = client()
        .get(format!("http://{addr}/api/v1/evolve{query}"))
        .header("cookie", format!("fluid_session={}", session.0))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "GET /api/v1/evolve{query}");
    resp.json().await.unwrap()
}

/// The round trip a person makes on the maintenance page: acknowledge a
/// finding, watch it leave the queue counted, see it again under the audit
/// toggle, then un-acknowledge it and watch it come back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledging_removes_a_finding_and_withdrawing_brings_it_back() {
    let fixture = serve(Options::default()).await;
    fixture
        .auth
        .add_user("ada", "Ada", None, Role::Editor, "s3cret")
        .await
        .unwrap();
    let session = login_session(fixture.addr, "ada", "s3cret").await;

    let before = queue_as(fixture.addr, &session, "").await;
    assert!(
        rows(&before)
            .iter()
            .any(|r| r["permalink"] == "human-capture" && r["rule"] == "V006"),
        "{before}"
    );
    assert_eq!(before["acknowledged"]["total"], 0);

    let resp = ack_request(
        fixture.addr,
        reqwest::Method::POST,
        &session,
        serde_json::json!({
            "permalink": "human-capture",
            "rule": "V006",
            "note": "the responder's own words, reviewed offline"
        }),
    )
    .send()
    .await
    .unwrap();
    let status = resp.status();
    let entry: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "{entry}");
    assert_eq!(entry["rule"], "V006");
    assert_eq!(entry["by"], "human:ada", "the session user acknowledged it");
    assert_eq!(entry["note"], "the responder's own words, reviewed offline");
    assert!(entry["at"].as_str().unwrap().contains('T'), "{entry}");

    let after = queue_as(fixture.addr, &session, "").await;
    assert!(
        !rows(&after)
            .iter()
            .any(|r| r["permalink"] == "human-capture" && r["rule"] == "V006"),
        "{after}"
    );
    assert_eq!(after["acknowledged"]["total"], 1);
    assert_eq!(after["acknowledged"]["by_family"]["temporal"], 1);

    let audited = queue_as(fixture.addr, &session, "?include_acknowledged=true").await;
    let row = rows(&audited)
        .iter()
        .find(|r| r["permalink"] == "human-capture" && r["rule"] == "V006")
        .expect("the audit view returns what it suppressed");
    assert_eq!(row["acknowledged"], true);
    assert_eq!(row["ack_note"], "the responder's own words, reviewed offline");

    let resp = ack_request(
        fixture.addr,
        reqwest::Method::DELETE,
        &session,
        serde_json::json!({ "permalink": "human-capture", "rule": "V006" }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 204);

    let back = queue_as(fixture.addr, &session, "").await;
    assert!(
        rows(&back)
            .iter()
            .any(|r| r["permalink"] == "human-capture" && r["rule"] == "V006"),
        "{back}"
    );
    assert_eq!(back["acknowledged"]["total"], 0);

    // Withdrawing again says so rather than reporting a removal that did not
    // happen.
    let resp = ack_request(
        fixture.addr,
        reqwest::Method::DELETE,
        &session,
        serde_json::json!({ "permalink": "human-capture", "rule": "V006" }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
}

/// The write matrix the endpoint is held to: identity, role, CSRF, and the two
/// ways a body can be wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ack_endpoint_holds_the_write_rules() {
    let fixture = serve(Options::default()).await;
    for (name, role) in [("ada", Role::Editor), ("view", Role::Viewer)] {
        fixture
            .auth
            .add_user(name, name, None, role, "s3cret")
            .await
            .unwrap();
    }
    let body = serde_json::json!({ "permalink": "human-capture", "rule": "V006" });

    // No identity at all.
    let resp = client()
        .post(format!(
            "http://{}/api/v1/domains/eng/evolve/ack",
            fixture.addr
        ))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // A viewer never writes.
    let viewer = login_session(fixture.addr, "view", "s3cret").await;
    let resp = ack_request(fixture.addr, reqwest::Method::POST, &viewer, body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    let editor = login_session(fixture.addr, "ada", "s3cret").await;

    // A session without its CSRF token is a cross-site request.
    let resp = client()
        .post(format!(
            "http://{}/api/v1/domains/eng/evolve/ack",
            fixture.addr
        ))
        .header("cookie", format!("fluid_session={}", editor.0))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    // A rule the catalog does not hold.
    let resp = ack_request(
        fixture.addr,
        reqwest::Method::POST,
        &editor,
        serde_json::json!({ "permalink": "human-capture", "rule": "V999" }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 422);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");

    // An engram nobody has.
    let resp = ack_request(
        fixture.addr,
        reqwest::Method::POST,
        &editor,
        serde_json::json!({ "permalink": "not-a-thing", "rule": "V006" }),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);

    // A domain nobody registered.
    let resp = client()
        .post(format!(
            "http://{}/api/v1/domains/nope/evolve/ack",
            fixture.addr
        ))
        .header("cookie", format!("fluid_session={}", editor.0))
        .header("x-csrf-token", &editor.1)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
