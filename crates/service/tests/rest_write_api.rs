//! Endpoint tests for the Group A write surface: engram create/save/retire/
//! move/delete, manifest save, the validation dry-run, user admin, and the
//! auth/CSRF and If-Match matrices over all of them.

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// What a write-test server varies.
#[derive(Default)]
struct Options {
    anonymous: bool,
    read_only: bool,
    trusted_header: Option<&'static str>,
}

struct Fixture {
    addr: std::net::SocketAddr,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

async fn serve(opts: Options) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            trusted_header: opts.trusted_header.map(str::to_string),
            anonymous: Some(opts.anonymous),
            max_users: None,
        }),
        ..GlobalConfig::default()
    };
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        read_only: Some(opts.read_only),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    // `service.read_only` is resolved by the daemon rather than by the engine's
    // constructor, so the fixture applies it the way `serve` does.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_read_only(opts.read_only),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    // One account per role, so every matrix row has a caller.
    auth.add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();
    auth.add_user("eddy", "Eddy", None, Role::Editor, "eddypw")
        .await
        .unwrap();
    auth.add_user("vera", "Vera", None, Role::Viewer, "verapw")
        .await
        .unwrap();
    // Two accounts nobody logs in as: the user-admin matrix mutates THESE, so
    // a password reset or deletion never revokes a session the matrix's own
    // callers are still using.
    auth.add_user("mark", "Mark", None, Role::Editor, "markpw")
        .await
        .unwrap();
    auth.add_user("tina", "Tina", None, Role::Viewer, "tinapw")
        .await
        .unwrap();

    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth.clone()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, router).await.unwrap();
    });
    Fixture {
        addr,
        auth,
        _tmp: tmp,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token).
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": password}))
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

/// A request carrying a session and its token, the browser shape.
fn as_session(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    session: &(String, String),
) -> reqwest::RequestBuilder {
    client()
        .request(method, format!("http://{addr}{path}"))
        .header("cookie", format!("fluid_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

/// The alpha detail read: (etag token unquoted, full content).
async fn read_alpha(addr: std::net::SocketAddr, session: &(String, String)) -> (String, String) {
    let resp = as_session(
        addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/engrams/alpha",
        session,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let etag = resp.headers()["etag"]
        .to_str()
        .unwrap()
        .trim_matches('"')
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    (etag, body["content"].as_str().unwrap().to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_editor_creates_an_engram_and_gets_the_detail_back() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/engrams",
        &editor,
    )
    .json(&serde_json::json!({
        "title": "Beta",
        "folder": "notes",
        "type": "guide",
        "tags": ["eng"],
        "content": "# Beta\n\nThe next rule.\n"
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 201);
    assert!(
        resp.headers().contains_key("etag"),
        "the created detail carries its ETag"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["permalink"], "notes/beta");
    assert_eq!(body["type"], "guide");
    // The file landed with provenance naming the account.
    let on_disk = std::fs::read_to_string(fx._tmp.path().join("eng/notes/beta.md")).unwrap();
    assert!(on_disk.contains("human:eddy"), "{on_disk}");

    // A second create at the same permalink is a 409.
    let dup = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/engrams",
        &editor,
    )
    .json(&serde_json::json!({"title": "Beta", "folder": "notes", "content": "again"}))
    .send()
    .await
    .unwrap();
    assert_eq!(dup.status(), 409);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_walks_the_if_match_contract() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let (etag, content) = read_alpha(fx.addr, &editor).await;

    // 428: the header is missing.
    let missing = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .json(&serde_json::json!({"content": content}))
    .send()
    .await
    .unwrap();
    assert_eq!(missing.status(), 428);
    assert_eq!(
        missing.headers()["content-type"],
        "application/problem+json",
        "even the precondition answers are problem details"
    );

    // Happy path: the edit lands and the answer carries the new ETag.
    let edited = content.replace("A rule about alpha.", "A sharper rule.");
    let saved = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({"content": edited}))
    .send()
    .await
    .unwrap();
    assert_eq!(saved.status(), 200);
    let new_etag = saved.headers()["etag"].to_str().unwrap().to_string();
    assert_ne!(new_etag, format!("\"{etag}\""));

    // 412: the old token is stale, and the payload carries the current truth.
    let stale = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({"content": "---\ntitle: X\n---\nwhatever"}))
    .send()
    .await
    .unwrap();
    assert_eq!(stale.status(), 412);
    assert_eq!(stale.headers()["content-type"], "application/problem+json");
    let conflict: serde_json::Value = stale.json().await.unwrap();
    assert_eq!(conflict["current_etag"].as_str().unwrap(), new_etag);
    assert!(
        conflict["current_content"]
            .as_str()
            .unwrap()
            .contains("A sharper rule.")
    );
    // And the stale body never landed.
    let (_final_etag, final_content) = read_alpha(fx.addr, &editor).await;
    assert!(final_content.contains("A sharper rule."));
}

/// Two tabs saving the same engram from the same read. Exactly one lands and
/// the other is told it is stale, and the file on disk holds one writer's bytes
/// whole rather than a blend of both.
///
/// End to end, this is the contract; it is not what pins the ordering. Measured
/// against a build with the per-file lock removed, two saves sent together over
/// HTTP still serialize on their own - the round trip is longer than the window
/// between one save's comparison and its write - so this passed there too. What
/// pins it is `engine::lock_tests::a_save_compares_inside_the_file_lock`, which
/// holds the lock from outside and rewrites the file underneath a blocked save.
/// Both are worth keeping: that one proves the order, this one proves the
/// order is what a client actually gets.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_saves_settle_as_one_winner_and_one_conflict() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let (etag, content) = read_alpha(fx.addr, &editor).await;

    let first = content.replace("A rule about alpha.", "The first writer got there.");
    let second = content.replace("A rule about alpha.", "The second writer got there.");
    let put = |body: String| {
        as_session(
            fx.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/eng/engrams/alpha",
            &editor,
        )
        .header("if-match", format!("\"{etag}\""))
        .json(&serde_json::json!({ "content": body }))
        .send()
    };
    let (a, b) = tokio::join!(put(first.clone()), put(second.clone()));
    let mut codes = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    codes.sort_unstable();
    assert_eq!(
        codes,
        [200, 412],
        "one save lands and the other is told to re-read"
    );

    let on_disk = std::fs::read_to_string(fx._tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        on_disk == first || on_disk == second,
        "one writer's bytes, whole: {on_disk}"
    );
}

/// The save writes exactly the bytes it was given, frontmatter included, even
/// when that frontmatter's `permalink` no longer matches the URL the save came
/// in on - and then follows the engram to where it moved.
///
/// Rewriting the caller's document to keep the URL and the file in step is not
/// this layer's call to make: an editor that saved something other than what
/// its author typed would be worse than an engram whose address moved. So the
/// answer follows instead, and a rename that also changes the title - the case
/// with no name left in common - still answers 200 at the new address rather
/// than 404 over a write that landed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_save_writes_verbatim_and_answers_at_the_new_address() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let (etag, content) = read_alpha(fx.addr, &editor).await;

    let diverged = content
        .replace("permalink: alpha", "permalink: renamed")
        .replace("title: Alpha", "title: Renamed");
    let saved = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({ "content": diverged }))
    .send()
    .await
    .unwrap();
    assert_eq!(saved.status(), 200);
    let new_etag = saved.headers()["etag"].to_str().unwrap().to_string();
    let body: serde_json::Value = saved.json().await.unwrap();
    assert_eq!(
        body["permalink"], "renamed",
        "the answer is the read at the address the engram now has"
    );

    let on_disk = std::fs::read_to_string(fx._tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, diverged, "byte for byte what was sent");

    // And the token that came back is the one the next save of it carries.
    let again = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/renamed",
        &editor,
    )
    .header("if-match", &new_etag)
    .json(&serde_json::json!({ "content": diverged.replace("A rule", "Another rule") }))
    .send()
    .await
    .unwrap();
    assert_eq!(again.status(), 200);
}

/// A body past the API's limit is refused with 413 rather than truncated or
/// hung on, and the refusal is a problem detail like every other one.
///
/// The limit is set explicitly (`rest::MAX_BODY_BYTES`) because axum's 2 MiB
/// default would leave an engram past that size readable but unsavable, which
/// is a trap an author only finds after making their edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_past_the_limit_is_refused_with_413() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let (etag, content) = read_alpha(fx.addr, &editor).await;

    // Comfortably past the limit, and past axum's default several times over.
    let huge = format!(
        "{content}{}",
        "x".repeat(crystalline_service::rest::MAX_BODY_BYTES)
    );
    let resp = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({ "content": huge }))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 413);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");

    // A document that is merely large still saves: the limit is generous on
    // purpose, and this is the size that broke under axum's default.
    let big = format!("{}\n{}\n", content.trim_end(), "padding ".repeat(400_000));
    let ok = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({ "content": big }))
    .send()
    .await
    .unwrap();
    assert_eq!(ok.status(), 200, "a 3 MiB engram is an engram");
}

/// The reserved OKF names are not documents: `index.md` is generated from the
/// folder and `log.md` is reserved beside it, so neither may be created nor
/// saved through this surface whatever the caller's role.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reserved_index_and_log_names_are_refused() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    for (title, folder) in [("Index", None), ("Log", Some("notes"))] {
        let resp = as_session(
            fx.addr,
            reqwest::Method::POST,
            "/api/v1/domains/eng/engrams",
            &editor,
        )
        .json(&serde_json::json!({
            "title": title,
            "folder": folder,
            "content": "# Nope\n"
        }))
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 422, "creating '{title}' is refused");
    }

    for permalink in ["index", "notes/log"] {
        let resp = as_session(
            fx.addr,
            reqwest::Method::PUT,
            &format!("/api/v1/domains/eng/engrams/{permalink}"),
            &editor,
        )
        .header("if-match", "\"deadbeef\"")
        .json(&serde_json::json!({"content": "---\ntitle: X\n---\n\nnope\n"}))
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), 422, "saving '{permalink}' is refused");
    }
}

/// Who may not write, in every identity mode the surface has: a viewer
/// account, the anonymous viewer, and a session that does not echo its CSRF
/// token. None of them reaches the engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_routes_refuse_viewers_the_anonymous_and_the_tokenless() {
    let fx = serve(Options {
        anonymous: true,
        ..Options::default()
    })
    .await;
    let viewer = login(fx.addr, "vera", "verapw").await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let (etag, content) = read_alpha(fx.addr, &editor).await;

    // A viewer is forbidden: logging in again would not help.
    let created = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/engrams",
        &viewer,
    )
    .json(&serde_json::json!({"title": "Gamma", "content": "no"}))
    .send()
    .await
    .unwrap();
    assert_eq!(created.status(), 403);
    let saved = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &viewer,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({"content": content}))
    .send()
    .await
    .unwrap();
    assert_eq!(saved.status(), 403);
    let retired = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/retire",
        &viewer,
    )
    .json(&serde_json::json!({"permalink": "alpha", "status": "deprecated"}))
    .send()
    .await
    .unwrap();
    assert_eq!(retired.status(), 403);
    let moved = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/move",
        &viewer,
    )
    .json(&serde_json::json!({"permalink": "alpha", "destination": "moved"}))
    .send()
    .await
    .unwrap();
    assert_eq!(moved.status(), 403);
    let deleted = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng/engrams/alpha",
        &viewer,
    )
    .header("if-match", format!("\"{etag}\""))
    .send()
    .await
    .unwrap();
    assert_eq!(deleted.status(), 403);
    let manifest_saved = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/manifest",
        &viewer,
    )
    .header("if-match", "\"deadbeef\"")
    .json(&serde_json::json!({"markdown": "no"}))
    .send()
    .await
    .unwrap();
    assert_eq!(manifest_saved.status(), 403);
    // /validate writes nothing, but it is held to the same editor-only rule
    // as every other write on this surface: see the standing lesson at the
    // top of this matrix - a dry run that bypassed the gate would let a
    // viewer run the rule engine over arbitrary content this route does not
    // mean to open.
    let validated = as_session(fx.addr, reqwest::Method::POST, "/api/v1/validate", &viewer)
        .json(&serde_json::json!({"content": ALPHA}))
        .send()
        .await
        .unwrap();
    assert_eq!(validated.status(), 403);

    // Each route's own valid body, so a schema mismatch never masks the
    // identity check: `ApiJson` extraction runs ahead of the handler and
    // would otherwise answer 422 before `require_editor` is ever reached.
    let bodies = || {
        [
            (
                reqwest::Method::POST,
                "/api/v1/domains/eng/engrams",
                serde_json::json!({"title": "Gamma", "content": "no"}),
            ),
            (
                reqwest::Method::PUT,
                "/api/v1/domains/eng/engrams/alpha",
                serde_json::json!({"content": "no"}),
            ),
            (
                reqwest::Method::POST,
                "/api/v1/domains/eng/retire",
                serde_json::json!({"permalink": "alpha", "status": "deprecated"}),
            ),
            (
                reqwest::Method::POST,
                "/api/v1/domains/eng/move",
                serde_json::json!({"permalink": "alpha", "destination": "moved"}),
            ),
            (
                reqwest::Method::DELETE,
                "/api/v1/domains/eng/engrams/alpha",
                serde_json::json!({}),
            ),
            (
                reqwest::Method::PUT,
                "/api/v1/domains/eng/manifest",
                serde_json::json!({"markdown": "no"}),
            ),
            (
                reqwest::Method::POST,
                "/api/v1/validate",
                serde_json::json!({"content": ALPHA}),
            ),
        ]
    };

    // The anonymous viewer is told to log in: an anonymous identity never
    // writes, whatever the deployment mode allows it to read.
    for (method, path, body) in bodies() {
        let resp = client()
            .request(method.clone(), format!("http://{}{path}", fx.addr))
            .header("if-match", "\"deadbeef\"")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401, "{method} {path} anonymously");
    }

    // A real editor session that does not echo its token is refused ahead of
    // the handler, so the CSRF rule covers the content routes like every other
    // unsafe method.
    for (method, path, body) in bodies() {
        let resp = client()
            .request(method.clone(), format!("http://{}{path}", fx.addr))
            .header("cookie", format!("fluid_session={}", editor.0))
            .header("if-match", format!("\"{etag}\""))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "{method} {path} without the token");
    }

    // Nothing above touched the domain.
    let (_still, unchanged) = read_alpha(fx.addr, &editor).await;
    assert!(unchanged.contains("A rule about alpha."));
}

/// A read-only instance refuses a content write before it looks at the write's
/// preconditions: the answer is 403, never the 428 a missing `If-Match` would
/// otherwise earn. An instance that refuses writes refuses them whatever
/// headers arrive, and a client that fetched an ETag first would otherwise be
/// sent round a loop that cannot end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_refuses_before_the_precondition_check() {
    let fx = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let no_if_match = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .json(&serde_json::json!({"content": ALPHA}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        no_if_match.status(),
        403,
        "read-only answers 403, not the 428 a missing If-Match would earn"
    );
    assert_eq!(
        no_if_match.headers()["content-type"],
        "application/problem+json"
    );

    let created = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/engrams",
        &editor,
    )
    .json(&serde_json::json!({"title": "Gamma", "content": "no"}))
    .send()
    .await
    .unwrap();
    assert_eq!(created.status(), 403);

    // /validate refuses too, even though it writes nothing: it checks
    // `read_only` itself, since no engine verb runs underneath it to answer
    // that for free.
    let validated = as_session(fx.addr, reqwest::Method::POST, "/api/v1/validate", &editor)
        .json(&serde_json::json!({"content": ALPHA}))
        .send()
        .await
        .unwrap();
    assert_eq!(validated.status(), 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retire_move_and_delete_run_through_their_endpoints() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    // Seed a successor through the create endpoint.
    let created = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/engrams",
        &editor,
    )
    .json(&serde_json::json!({"title": "Beta", "content": "# Beta\n\nSharper.\n"}))
    .send()
    .await
    .unwrap();
    assert_eq!(created.status(), 201);

    // Retire alpha in favor of beta.
    let retired = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/retire",
        &editor,
    )
    .json(&serde_json::json!({
        "permalink": "alpha",
        "status": "superseded",
        "successor": "beta"
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(retired.status(), 200);
    let alpha = std::fs::read_to_string(fx._tmp.path().join("eng/alpha.md")).unwrap();
    assert!(alpha.contains("status: superseded"), "{alpha}");
    assert!(alpha.contains("- superseded_by [[Beta]]"), "{alpha}");

    // An invalid retirement status is a 422 with the engine's words.
    let bad = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/retire",
        &editor,
    )
    .json(&serde_json::json!({"permalink": "alpha", "status": "stable"}))
    .send()
    .await
    .unwrap();
    assert_eq!(bad.status(), 422);

    // Move beta into a folder.
    let moved = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/domains/eng/move",
        &editor,
    )
    .json(&serde_json::json!({"permalink": "beta", "destination": "guides/beta"}))
    .send()
    .await
    .unwrap();
    assert_eq!(moved.status(), 200);
    assert!(fx._tmp.path().join("eng/guides/beta.md").exists());
    assert!(!fx._tmp.path().join("eng/beta.md").exists());

    // Hard delete demands If-Match, then honors it.
    let bare = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(bare.status(), 428);
    let (etag, _) = read_alpha(fx.addr, &editor).await;
    let stale = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header(
        "if-match",
        "\"0000000000000000000000000000000000000000000000000000000000000000\"",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(stale.status(), 412);
    let gone = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng/engrams/alpha",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .send()
    .await
    .unwrap();
    assert_eq!(gone.status(), 204);
    assert!(!fx._tmp.path().join("eng/alpha.md").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_manifest_reads_with_an_etag_and_saves_under_if_match() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let read = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/manifest",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(read.status(), 200);
    let etag = read.headers()["etag"]
        .to_str()
        .unwrap()
        .trim_matches('"')
        .to_string();
    let body: serde_json::Value = read.json().await.unwrap();
    assert_eq!(
        body["checksum"].as_str().unwrap(),
        etag,
        "header and body agree"
    );
    let markdown = body["markdown"].as_str().unwrap().to_string();

    // No header: 428. Stale: 412 with the current manifest. Fresh: 200.
    let missing = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/manifest",
        &editor,
    )
    .json(&serde_json::json!({"markdown": markdown}))
    .send()
    .await
    .unwrap();
    assert_eq!(missing.status(), 428);

    let edited = markdown.replace(
        "Route here for eng questions",
        "Route here for all things eng",
    );
    let saved = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/manifest",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({"markdown": edited}))
    .send()
    .await
    .unwrap();
    assert_eq!(saved.status(), 200);
    let saved_body: serde_json::Value = saved.json().await.unwrap();
    assert_ne!(saved_body["checksum"].as_str().unwrap(), etag);
    assert_eq!(
        std::fs::read_to_string(fx._tmp.path().join("eng/MANIFEST.md")).unwrap(),
        edited
    );

    let stale = as_session(
        fx.addr,
        reqwest::Method::PUT,
        "/api/v1/domains/eng/manifest",
        &editor,
    )
    .header("if-match", format!("\"{etag}\""))
    .json(&serde_json::json!({"markdown": "---\ntitle: hijack\n---\n"}))
    .send()
    .await
    .unwrap();
    assert_eq!(stale.status(), 412);
    let conflict: serde_json::Value = stale.json().await.unwrap();
    assert!(
        conflict["current_content"]
            .as_str()
            .unwrap()
            .contains("all things eng")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validate_reports_findings_without_writing() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    // A superseded engram without a successor trips T005.
    let resp = as_session(fx.addr, reqwest::Method::POST, "/api/v1/validate", &editor)
        .json(&serde_json::json!({
            "domain": "eng",
            "path": "alpha.md",
            "content": ALPHA.replace("status: stable", "status: superseded")
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rules: Vec<&str> = body["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(rules.contains(&"T005"), "expected T005 in {rules:?}");

    // A clean document has no findings, and nothing was ever written.
    let clean = as_session(fx.addr, reqwest::Method::POST, "/api/v1/validate", &editor)
        .json(&serde_json::json!({"content": ALPHA}))
        .send()
        .await
        .unwrap();
    assert_eq!(clean.status(), 200);
    let clean: serde_json::Value = clean.json().await.unwrap();
    assert_eq!(clean["errors"], 0);
    assert_eq!(
        std::fs::read_to_string(fx._tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA,
        "a dry run writes nothing"
    );
}

/// The trusted-header mode, end to end on a write: a proxy identity is
/// provisioned at viewer and so cannot write, the CSRF token it needs comes
/// from `/auth/me` and is required here like anywhere else, and promoting the
/// account through the store the `crystalline users` CLI shares is what opens
/// the route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proxy_identity_writes_once_it_is_an_editor_carrying_its_token() {
    let fx = serve(Options {
        trusted_header: Some("remote-user"),
        ..Options::default()
    })
    .await;
    let probe = client()
        .get(format!("http://{}/api/v1/auth/me", fx.addr))
        .header("remote-user", "dana")
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), 200);
    let me: serde_json::Value = probe.json().await.unwrap();
    assert_eq!(me["user"]["role"], "viewer", "provisioned at viewer");
    let csrf = me["csrf"].as_str().unwrap().to_string();

    let create = |csrf: Option<&str>| {
        let mut req = client()
            .post(format!("http://{}/api/v1/domains/eng/engrams", fx.addr))
            .header("remote-user", "dana");
        if let Some(csrf) = csrf {
            req = req.header("x-csrf-token", csrf);
        }
        req.json(&serde_json::json!({"title": "Delta", "content": "# Delta\n"}))
            .send()
    };

    assert_eq!(
        create(Some(&csrf)).await.unwrap().status(),
        403,
        "a viewer the proxy names is still a viewer"
    );
    fx.auth.set_role("dana", Role::Editor).await.unwrap();
    assert_eq!(
        create(None).await.unwrap().status(),
        403,
        "and the CSRF rule has no trusted-header exemption"
    );
    let created = create(Some(&csrf)).await.unwrap();
    assert_eq!(created.status(), 201);
    let body: serde_json::Value = created.json().await.unwrap();
    assert_eq!(body["permalink"], "delta");
    let on_disk = std::fs::read_to_string(fx._tmp.path().join("eng/delta.md")).unwrap();
    assert!(on_disk.contains("human:dana"), "{on_disk}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_edits_display_names_and_resets_passwords() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    // Display name set and cleared.
    let patched = as_session(
        fx.addr,
        reqwest::Method::PATCH,
        "/api/v1/users/eddy",
        &admin,
    )
    .json(&serde_json::json!({"display": "Eddy the Editor"}))
    .send()
    .await
    .unwrap();
    assert_eq!(patched.status(), 200);
    let body: serde_json::Value = patched.json().await.unwrap();
    assert_eq!(body["user"]["display"], "Eddy the Editor");
    let cleared = as_session(
        fx.addr,
        reqwest::Method::PATCH,
        "/api/v1/users/eddy",
        &admin,
    )
    .json(&serde_json::json!({"display": ""}))
    .send()
    .await
    .unwrap();
    let cleared: serde_json::Value = cleared.json().await.unwrap();
    assert_eq!(
        cleared["user"]["display"], "eddy",
        "clear resets to the login name"
    );

    // Password now travels its own route, and PATCH refuses it.
    let old_shape = as_session(
        fx.addr,
        reqwest::Method::PATCH,
        "/api/v1/users/eddy",
        &admin,
    )
    .json(&serde_json::json!({"password": "newpw"}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        old_shape.status(),
        422,
        "password is no longer a PATCH field"
    );

    let reset = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/users/eddy/password",
        &admin,
    )
    .json(&serde_json::json!({"password": "newpw"}))
    .send()
    .await
    .unwrap();
    assert_eq!(reset.status(), 200);

    // The reset revoked eddy's sessions: the old login is dead, the new works.
    let editor = login(fx.addr, "eddy", "newpw").await;
    let probe = as_session(fx.addr, reqwest::Method::GET, "/api/v1/auth/me", &editor)
        .send()
        .await
        .unwrap();
    assert_eq!(probe.status(), 200);
}

/// Session revocation through the API, both triggers (spec section 10).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deactivation_and_reset_revoke_live_sessions_through_the_api() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;
    let eddy = login(fx.addr, "eddy", "eddypw").await;

    // Deactivate: eddy's live session stops resolving mid-flight.
    let off = as_session(
        fx.addr,
        reqwest::Method::PATCH,
        "/api/v1/users/eddy",
        &admin,
    )
    .json(&serde_json::json!({"disabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(off.status(), 200);
    let dead = as_session(fx.addr, reqwest::Method::GET, "/api/v1/domains", &eddy)
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status(), 401, "the revoked session no longer resolves");

    // Reactivate, log in again, then reset the password: same eviction.
    as_session(
        fx.addr,
        reqwest::Method::PATCH,
        "/api/v1/users/eddy",
        &admin,
    )
    .json(&serde_json::json!({"disabled": false}))
    .send()
    .await
    .unwrap();
    let eddy = login(fx.addr, "eddy", "eddypw").await;
    as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/users/eddy/password",
        &admin,
    )
    .json(&serde_json::json!({"password": "rotated"}))
    .send()
    .await
    .unwrap();
    let dead = as_session(fx.addr, reqwest::Method::GET, "/api/v1/domains", &eddy)
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status(), 401);
}

/// NOT_LAST_ADMIN through the API: absolute, server-side, 409 with the
/// store's own explanation (spec section 10).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_admin_is_protected_through_the_api() {
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;

    for (method, path, body) in [
        (
            reqwest::Method::PATCH,
            "/api/v1/users/root",
            Some(serde_json::json!({"role": "viewer"})),
        ),
        (reqwest::Method::DELETE, "/api/v1/users/root", None),
    ] {
        let mut req = as_session(fx.addr, method, path, &admin);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), 409);
        let problem: serde_json::Value = resp.json().await.unwrap();
        let detail = problem["detail"].as_str().unwrap();
        assert!(
            detail.contains("last admin") || detail.contains("your own account"),
            "a refusal in words, never a raw error: {detail}"
        );
    }
}

/// Every mutating user-admin route is refused on a read-only instance, ahead
/// of the checks each one would otherwise run: an unknown account is never
/// probed, and a change that would otherwise be well-formed is refused before
/// it reaches the store. `crystalline users` on the server is the recovery
/// path (spec section 10, resolved ambiguity 7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_refuses_user_mutations() {
    let fx = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;

    let created = as_session(fx.addr, reqwest::Method::POST, "/api/v1/users", &admin)
        .json(&serde_json::json!({"name": "gina", "role": "viewer", "password": "hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 403);
    assert_eq!(
        created.headers()["content-type"],
        "application/problem+json"
    );

    let patched = as_session(
        fx.addr,
        reqwest::Method::PATCH,
        "/api/v1/users/eddy",
        &admin,
    )
    .json(&serde_json::json!({"display": "Nope"}))
    .send()
    .await
    .unwrap();
    assert_eq!(patched.status(), 403);

    let reset = as_session(
        fx.addr,
        reqwest::Method::POST,
        "/api/v1/users/eddy/password",
        &admin,
    )
    .json(&serde_json::json!({"password": "newpw"}))
    .send()
    .await
    .unwrap();
    assert_eq!(reset.status(), 403);

    let removed = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/users/eddy",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(removed.status(), 403);

    // `GET /users` stays served: read-only refuses writes, not reads.
    let listed = as_session(fx.addr, reqwest::Method::GET, "/api/v1/users", &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);

    assert_eq!(
        fx.auth.list_users().await.unwrap().len(),
        5,
        "nothing above changed anything"
    );
}

/// One write operation as the matrix drives it.
struct WriteOp {
    method: reqwest::Method,
    path: &'static str,
    /// A body that passes validation when the caller is allowed.
    body: Option<serde_json::Value>,
    /// Whether the route demands admin (403 for an editor).
    admin_only: bool,
}

/// Every mutating route the `/api/v1` surface mounts, spec section 10's write
/// matrix as data. A route added to the router without a row here fails
/// `write_ops_covers_every_mutating_route_mounted` below, by name, rather
/// than depending on a reviewer noticing anything.
fn write_ops() -> Vec<WriteOp> {
    use reqwest::Method;
    vec![
        WriteOp {
            method: Method::POST,
            path: "/api/v1/domains/eng/engrams",
            body: Some(serde_json::json!({"title": "Fresh", "content": "# Fresh\n"})),
            admin_only: false,
        },
        WriteOp {
            method: Method::PUT,
            path: "/api/v1/domains/eng/engrams/alpha",
            body: Some(serde_json::json!({"content": "x"})),
            admin_only: false,
        },
        WriteOp {
            method: Method::POST,
            path: "/api/v1/domains/eng/retire",
            body: Some(serde_json::json!({"permalink": "alpha", "status": "deprecated"})),
            admin_only: false,
        },
        WriteOp {
            method: Method::POST,
            path: "/api/v1/domains/eng/move",
            body: Some(serde_json::json!({"permalink": "alpha", "destination": "moved/alpha"})),
            admin_only: false,
        },
        WriteOp {
            method: Method::DELETE,
            path: "/api/v1/domains/eng/engrams/alpha",
            body: None,
            admin_only: false,
        },
        WriteOp {
            method: Method::PUT,
            path: "/api/v1/domains/eng/manifest",
            body: Some(serde_json::json!({"markdown": "x"})),
            admin_only: false,
        },
        WriteOp {
            method: Method::POST,
            path: "/api/v1/validate",
            body: Some(serde_json::json!({"content": "x"})),
            admin_only: false,
        },
        WriteOp {
            method: Method::POST,
            path: "/api/v1/users",
            body: Some(serde_json::json!({"name": "new", "role": "viewer", "password": "pw"})),
            admin_only: true,
        },
        WriteOp {
            method: Method::PATCH,
            path: "/api/v1/users/mark",
            body: Some(serde_json::json!({"display": "M"})),
            admin_only: true,
        },
        WriteOp {
            method: Method::POST,
            path: "/api/v1/users/mark/password",
            body: Some(serde_json::json!({"password": "pw2"})),
            admin_only: true,
        },
        WriteOp {
            method: Method::DELETE,
            path: "/api/v1/users/tina",
            body: None,
            admin_only: true,
        },
    ]
}

fn request_for(
    addr: std::net::SocketAddr,
    op: &WriteOp,
    session: Option<&(String, String)>,
    csrf: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut req = client().request(op.method.clone(), format!("http://{addr}{}", op.path));
    if let Some((cookie, own_csrf)) = session {
        req = req.header("cookie", format!("fluid_session={cookie}"));
        req = req.header("x-csrf-token", csrf.unwrap_or(own_csrf));
    }
    if let Some(body) = &op.body {
        req = req.json(body);
    } else {
        // A bodyless op still needs to not trip 415 on anything; DELETE sends none.
        req = req.header("content-type", "application/json");
    }
    req
}

/// The spec's matrix: anonymous never writes, viewer 403, admin-only routes
/// 403 for an editor, missing/wrong CSRF 403, read_only refuses all. "Allowed"
/// is asserted as "gets past authorization", i.e. anything but 401/403 - the
/// per-endpoint tests own the happy-path semantics (ETags, bodies, 4xx
/// preconditions).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_write_matrix_holds_on_every_route() {
    // Plain instance: role and CSRF rows.
    let fx = serve(Options::default()).await;
    let admin = login(fx.addr, "root", "rootpw").await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let viewer = login(fx.addr, "vera", "verapw").await;

    for op in write_ops() {
        let label = format!("{} {}", op.method, op.path);

        // No identity at all: 401 ahead of everything.
        let resp = request_for(fx.addr, &op, None, None).send().await.unwrap();
        assert_eq!(resp.status(), 401, "{label} with no identity");

        // A viewer session: authenticated, refused.
        let resp = request_for(fx.addr, &op, Some(&viewer), None)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "{label} as viewer");

        // Missing and wrong CSRF: refused before any handler logic.
        let session = if op.admin_only { &admin } else { &editor };
        let no_token = client()
            .request(op.method.clone(), format!("http://{}{}", fx.addr, op.path))
            .header("cookie", format!("fluid_session={}", session.0));
        let no_token = match &op.body {
            Some(body) => no_token.json(body),
            None => no_token,
        };
        assert_eq!(
            no_token.send().await.unwrap().status(),
            403,
            "{label} without csrf"
        );
        let resp = request_for(fx.addr, &op, Some(session), Some("wrong"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "{label} with wrong csrf");

        // Admin-only routes refuse an editor.
        if op.admin_only {
            let resp = request_for(fx.addr, &op, Some(&editor), None)
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 403, "{label} as editor on an admin route");
        }

        // The allowed caller gets past authorization: whatever the endpoint
        // answers (2xx, or a 4xx precondition like 428), it is not 401/403.
        let resp = request_for(fx.addr, &op, Some(session), None)
            .send()
            .await
            .unwrap();
        assert!(
            resp.status() != 401 && resp.status() != 403,
            "{label} as its minimum role must pass authorization, got {}",
            resp.status()
        );
    }

    // Anonymous instance: the anonymous viewer never writes.
    let anon = serve(Options {
        anonymous: true,
        ..Options::default()
    })
    .await;
    for op in write_ops() {
        let resp = request_for(anon.addr, &op, None, None)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            401,
            "{} {} as the anonymous viewer: told to log in, never served",
            op.method,
            op.path
        );
    }

    // Read-only instance: every write refuses for the strongest caller.
    let ro = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let admin = login(ro.addr, "root", "rootpw").await;
    for op in write_ops() {
        let resp = request_for(ro.addr, &op, Some(&admin), None)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            403,
            "{} {} under read_only",
            op.method,
            op.path
        );
    }
}

/// Maps a matrix fixture's concrete path to the template form
/// `support::MOUNTED_OPERATIONS` spells operation paths in, e.g.
/// `/api/v1/domains/eng/engrams/alpha` becomes
/// `/api/v1/domains/{domain}/engrams/{permalink}`. `write_ops()` has exactly
/// three fixture names in play - `eng` the one domain, `alpha` the one
/// engram, `mark`/`tina` the two user-admin targets - so a fixed per-segment
/// substitution is enough; nothing here needs to be a general router.
fn canonicalize(path: &str) -> String {
    path.split('/')
        .map(|segment| match segment {
            "eng" => "{domain}",
            "alpha" => "{permalink}",
            "mark" | "tina" => "{name}",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The enumeration property: `write_ops()` covers every mutating route this
/// surface mounts, and nothing else.
///
/// Built from `support::MOUNTED_OPERATIONS` - the same list
/// `openapi_snapshot.rs`'s `the_document_covers_every_mounted_path` already
/// checks against the served document - rather than a second hand-kept copy.
/// A route added to the router is forced into that list by the OpenAPI
/// coverage check already; this test then forces it into `write_ops()` too,
/// so a write route that lands without a matrix row fails here by name
/// instead of relying on a reviewer noticing a pointer comment (this is
/// exactly how Task 13 shipped three routes uncovered by the matrix).
///
/// Two mounted mutating routes are named exemptions rather than matrix rows,
/// both resting on `check_csrf` in `rest/auth.rs`:
/// - `POST /auth/login` is CSRF-exempt by design: `check_csrf` waves through
///   any request whose path is `LOGIN_PATH` unconditionally, because login is
///   what mints the token a later request would echo - there is no session
///   yet to carry one.
/// - `POST /auth/logout` is a safe no-op for a tokenless caller: `check_csrf`
///   only demands a token when `identity.user.is_some()`; an identity with no
///   resolved account (no cookie, or a cookie the server has forgotten)
///   passes with no token at all, and logout has nothing to revoke for it
///   either way. A *real* session's logout still enforces the same token
///   match as every other unsafe request - only the account-less case is
///   exempt, which is why this is a logout-specific carve-out and not a
///   second CSRF-exempt path in `check_csrf` itself.
#[test]
fn write_ops_covers_every_mutating_route_mounted() {
    use std::collections::BTreeSet;

    const EXEMPT: &[&str] = &["POST /api/v1/auth/login", "POST /api/v1/auth/logout"];

    let mutating: BTreeSet<String> = support::MOUNTED_OPERATIONS
        .iter()
        .filter(|op| {
            op.starts_with("POST ")
                || op.starts_with("PUT ")
                || op.starts_with("PATCH ")
                || op.starts_with("DELETE ")
        })
        .filter(|op| !EXEMPT.contains(op))
        .map(|op| op.to_string())
        .collect();

    let covered: BTreeSet<String> = write_ops()
        .iter()
        .map(|op| format!("{} {}", op.method, canonicalize(op.path)))
        .collect();

    let missing: Vec<&String> = mutating.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "these mounted write routes have no write_ops() row in the auth/CSRF \
         matrix: {missing:?}"
    );
    let extra: Vec<&String> = covered.difference(&mutating).collect();
    assert!(
        extra.is_empty(),
        "these write_ops() rows match no mounted route: {extra:?}"
    );
}
