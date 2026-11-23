//! Invite links over the JSON API: who may make which link, the caps, the
//! address the link is built at, the list and the revoke, and a read-only
//! instance.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, InvitesConfig, LoginConfig, ResponseFormat,
    ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use reqwest::Method;
use serde_json::{Value, json};
use tokio::sync::Mutex;

/// What an invite-test server varies.
#[derive(Default)]
pub(super) struct Options {
    /// `auth.invites.managers`.
    pub(super) managers: bool,
    pub(super) read_only: bool,
    /// `service.public_url`.
    pub(super) public_url: Option<&'static str>,
    /// `auth.max_users`.
    pub(super) max_users: Option<u32>,
    /// `auth.proxy_headers`.
    pub(super) proxy_headers: bool,
    /// `auth.login`, which the invite-token throttle shares.
    pub(super) login: Option<LoginConfig>,
}

pub(super) struct Fixture {
    pub(super) addr: std::net::SocketAddr,
    pub(super) auth: Arc<AuthStore>,
    _state: crate::support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

const ACCOUNTS: usize = 6;

fn manifest(name: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
    )
}

/// A server with the shared domain `eng` and the private domain `team`,
/// owned by `olga` (an instance viewer) with `mona` its manager and `eve`
/// an editor there. `root` is the admin, `eddy` an editor and `vera` a
/// viewer, neither of them in `team`. Every password is `pw12345678`.
pub(super) async fn serve(opts: Options) -> Fixture {
    let state = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            max_users: opts.max_users,
            proxy_headers: opts.proxy_headers.then_some(true),
            login: opts.login,
            invites: opts.managers.then_some(InvitesConfig {
                managers: Some(true),
            }),
            ..AuthConfig::default()
        }),
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            read_only: Some(opts.read_only),
            public_url: opts.public_url.map(str::to_string),
            ..ServiceConfig::default()
        }),
        ..GlobalConfig::default()
    };
    for name in ["eng", "team"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), manifest(name)).unwrap();
        cfg.domains.insert(name.to_string(), DomainEntry::file(dir));
    }
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(store)),
            cfg,
            None,
            Some(config_path.clone()),
        )
        .with_read_only(opts.read_only)
        .with_token_store_dir(root.join("tokens"))
        .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role) in [
        ("root", Role::Admin),
        ("eddy", Role::Editor),
        ("vera", Role::Viewer),
        ("olga", Role::Viewer),
        ("mona", Role::Editor),
        ("eve", Role::Editor),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    assert_eq!(auth.user_count().await.unwrap(), ACCOUNTS);
    auth.set_domain_visibility("team", true, "olga")
        .await
        .unwrap();
    auth.upsert_domain_member("team", "mona", MemberLevel::Manager, None, "olga")
        .await
        .unwrap();
    auth.upsert_domain_member("team", "eve", MemberLevel::Editor, None, "olga")
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
        auth,
        _state: state,
        _tmp: tmp,
    }
}

pub(super) fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token).
pub(super) async fn login(fx: &Fixture, name: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{}/api/v1/auth/login", fx.addr))
        .json(&json!({"name": name, "password": "pw12345678"}))
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
    let body: Value = resp.json().await.unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_string())
}

/// One request as `session`: the status, the `Cache-Control` header and the
/// body as JSON (`null` for an empty one).
pub(super) async fn call(
    fx: &Fixture,
    session: &(String, String),
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let mut request = client()
        .request(method, format!("http://{}{path}", fx.addr))
        .header("cookie", format!("fluid_session={}", session.0))
        .header("x-csrf-token", &session.1);
    if let Some(body) = body {
        request = request.json(&body);
    } else {
        request = request.header("content-type", "application/json");
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    let cache = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let text = resp.text().await.unwrap();
    let value = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    (status, cache, value)
}

pub(super) async fn create(fx: &Fixture, session: &(String, String), body: Value) -> (u16, Value) {
    let (status, _, body) = call(fx, session, Method::POST, "/api/v1/invites", Some(body)).await;
    (status, body)
}

async fn list(fx: &Fixture, session: &(String, String), query: &str) -> (u16, Value) {
    let (status, _, body) = call(
        fx,
        session,
        Method::GET,
        &format!("/api/v1/invites{query}"),
        None,
    )
    .await;
    (status, body)
}

async fn revoke(fx: &Fixture, session: &(String, String), id: i64) -> u16 {
    call(
        fx,
        session,
        Method::DELETE,
        &format!("/api/v1/invites/{id}"),
        None,
    )
    .await
    .0
}

/// An admin makes a link with every field, sees the token and the address
/// once (kept under the base path of `service.public_url`), lists it without
/// the token, filters by state and revokes it. Shapes that cannot work are
/// 422, a shared domain 409 and an unknown one 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admin_makes_lists_and_revokes_invite_links() {
    let fx = serve(Options {
        public_url: Some("https://kb.example.com/crystalline"),
        ..Options::default()
    })
    .await;
    let root = login(&fx, "root").await;
    let (status, cache, made) = call(
        &fx,
        &root,
        Method::POST,
        "/api/v1/invites",
        Some(json!({
            "role": "editor",
            "domain": "team",
            "level": "manager",
            "label": "for Bo",
            "local_account": true,
            "expires_in_days": 3,
        })),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    assert_eq!(cache, "no-store");
    let token = made["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("cin_"), "{token}");
    assert_eq!(
        made["link"],
        format!("https://kb.example.com/crystalline/invite#{token}")
    );
    assert_eq!(made["relative"], false);
    let invite = &made["invite"];
    assert_eq!(invite["role"], "editor");
    assert_eq!(invite["domain"], "team");
    assert_eq!(invite["level"], "manager");
    assert_eq!(invite["label"], "for Bo");
    assert_eq!(invite["local_account"], true);
    assert_eq!(invite["created_by"], "root");
    assert_eq!(invite["state"], "pending");
    let id = invite["id"].as_i64().unwrap();

    // Defaults: an admin's link is for a viewer, and a domain without a level
    // gives viewer there.
    let (status, plain) = create(&fx, &root, json!({})).await;
    assert_eq!(status, 201, "{plain}");
    assert_eq!(plain["invite"]["role"], "viewer");
    assert!(plain["invite"]["domain"].is_null());
    assert!(plain["invite"]["level"].is_null());
    let (status, to_team) = create(&fx, &root, json!({"role": "none", "domain": "team"})).await;
    assert_eq!(status, 201, "{to_team}");
    assert_eq!(to_team["invite"]["level"], "viewer");

    let (status, listed) = list(&fx, &root, "").await;
    assert_eq!(status, 200);
    assert_eq!(listed["invites"].as_array().unwrap().len(), 3);
    assert!(
        !listed.to_string().contains(&token),
        "never the token again"
    );

    for (body, want) in [
        (json!({"role": "admin"}), 422),
        (json!({"level": "editor"}), 422),
        (json!({"domain": "team", "level": "none"}), 422),
        (json!({"expires_in_days": 0}), 422),
        (json!({"expires_in_days": 31}), 422),
        (json!({"label": "x".repeat(101)}), 422),
        (json!({"surprise": true}), 422),
        (json!({"domain": "eng"}), 409),
        (json!({"domain": "nowhere"}), 404),
    ] {
        let (status, answer) = create(&fx, &root, body.clone()).await;
        assert_eq!(status, want, "{body}: {answer}");
    }

    assert_eq!(revoke(&fx, &root, id).await, 204);
    assert_eq!(revoke(&fx, &root, id).await, 404, "already revoked");
    assert_eq!(revoke(&fx, &root, 9_999).await, 404);
    let (_, revoked) = list(&fx, &root, "?state=revoked").await;
    let revoked = revoked["invites"].as_array().unwrap();
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0]["id"], id);
    assert!(revoked[0]["revoked_at"].is_string());
    let (_, pending) = list(&fx, &root, "?state=pending").await;
    assert_eq!(pending["invites"].as_array().unwrap().len(), 2);
    let (status, _) = list(&fx, &root, "?state=lost").await;
    assert_eq!(status, 400);
}

/// Without `service.public_url` the answer is the path alone, under the
/// base path, for the browser to complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_public_url_the_link_is_a_path() {
    let fx = serve(Options::default()).await;
    let root = login(&fx, "root").await;
    let (status, made) = create(&fx, &root, json!({"role": "viewer"})).await;
    assert_eq!(status, 201, "{made}");
    let token = made["token"].as_str().unwrap();
    assert_eq!(made["link"], format!("/invite#{token}"));
    assert_eq!(made["relative"], true);
}

/// With `auth.invites.managers` off only an admin makes a link. With it on,
/// the owner or a manager of a private domain makes one to that domain only,
/// for role `none`, and never a local-account one; an editor member, a
/// stranger (who is not even told the domain exists) and a shared domain
/// are refused. A maker lists and revokes only their own links; an admin
/// lists and revokes every one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owners_and_managers_make_links_only_where_the_instance_allows() {
    let off = serve(Options::default()).await;
    for name in ["mona", "olga", "eddy", "vera"] {
        let session = login(&off, name).await;
        let (status, body) = create(&off, &session, json!({"domain": "team"})).await;
        assert_eq!(status, 403, "{name}: {body}");
        assert!(
            !body.to_string().contains("auth.invites"),
            "no setting named to a person who cannot change it: {body}"
        );
    }
    assert!(off.auth.invites(None, None).await.unwrap().is_empty());

    let fx = serve(Options {
        managers: true,
        ..Options::default()
    })
    .await;
    let root = login(&fx, "root").await;
    let mona = login(&fx, "mona").await;
    let olga = login(&fx, "olga").await;
    let eve = login(&fx, "eve").await;
    let vera = login(&fx, "vera").await;
    let eddy = login(&fx, "eddy").await;

    let (status, by_mona) = create(
        &fx,
        &mona,
        json!({"domain": "team", "level": "manager", "label": "new lead"}),
    )
    .await;
    assert_eq!(status, 201, "{by_mona}");
    assert_eq!(by_mona["invite"]["role"], "none");
    assert_eq!(by_mona["invite"]["level"], "manager");
    assert_eq!(by_mona["invite"]["local_account"], false);
    let (status, by_olga) = create(&fx, &olga, json!({"domain": "team", "role": "none"})).await;
    assert_eq!(status, 201, "{by_olga}");
    assert_eq!(by_olga["invite"]["level"], "viewer");

    for (session, body, want) in [
        (&mona, json!({"domain": "team", "role": "viewer"}), 403),
        (&mona, json!({"domain": "team", "local_account": true}), 403),
        (&mona, json!({}), 403),
        (&mona, json!({"domain": "eng"}), 409),
        (&eve, json!({"domain": "team"}), 403),
        (&vera, json!({"domain": "team"}), 404),
        (&eddy, json!({"domain": "eng"}), 409),
        (&eddy, json!({}), 403),
    ] {
        let (status, answer) = create(&fx, session, body.clone()).await;
        assert_eq!(status, want, "{body}: {answer}");
    }

    // Each maker sees their own; an admin sees every one; anyone else none.
    let (_, mine) = list(&fx, &mona, "").await;
    let mine = mine["invites"].as_array().unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0]["created_by"], "mona");
    let (_, all) = list(&fx, &root, "").await;
    assert_eq!(all["invites"].as_array().unwrap().len(), 2);
    let (status, none) = list(&fx, &vera, "").await;
    assert_eq!(status, 200);
    assert!(none["invites"].as_array().unwrap().is_empty());

    let mona_id = by_mona["invite"]["id"].as_i64().unwrap();
    let olga_id = by_olga["invite"]["id"].as_i64().unwrap();
    assert_eq!(revoke(&fx, &mona, olga_id).await, 404, "not hers");
    assert_eq!(revoke(&fx, &vera, mona_id).await, 404, "not hers");
    assert_eq!(revoke(&fx, &mona, mona_id).await, 204);
    assert_eq!(revoke(&fx, &root, olga_id).await, 204);
}

/// A domain manager holds at most 10 pending links and an admin 50; the
/// accounts plus the pending links stay below `auth.max_users`, and that
/// refusal names no setting. A revoked link frees its place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_links_are_capped() {
    let fx = serve(Options {
        managers: true,
        ..Options::default()
    })
    .await;
    let mona = login(&fx, "mona").await;
    let mut first = None;
    for _ in 0..10 {
        let (status, made) = create(&fx, &mona, json!({"domain": "team"})).await;
        assert_eq!(status, 201, "{made}");
        first.get_or_insert(made["invite"]["id"].as_i64().unwrap());
    }
    let (status, refused) = create(&fx, &mona, json!({"domain": "team"})).await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(revoke(&fx, &mona, first.unwrap()).await, 204);
    let (status, made) = create(&fx, &mona, json!({"domain": "team"})).await;
    assert_eq!(status, 201, "a revoked link frees its place: {made}");

    let root = login(&fx, "root").await;
    for _ in 0..50 {
        let (status, made) = create(&fx, &root, json!({})).await;
        assert_eq!(status, 201, "{made}");
    }
    let (status, refused) = create(&fx, &root, json!({})).await;
    assert_eq!(status, 409, "{refused}");

    let small = serve(Options {
        max_users: Some(u32::try_from(ACCOUNTS).unwrap() + 2),
        ..Options::default()
    })
    .await;
    let root = login(&small, "root").await;
    let (_, one) = create(&small, &root, json!({})).await;
    let (status, _) = create(&small, &root, json!({})).await;
    assert_eq!(status, 201);
    let (status, full) = create(&small, &root, json!({})).await;
    assert_eq!(status, 409, "{full}");
    assert!(!full.to_string().contains("auth.max_users"), "{full}");
    assert_eq!(
        revoke(&small, &root, one["invite"]["id"].as_i64().unwrap()).await,
        204
    );
    let (status, made) = create(&small, &root, json!({})).await;
    assert_eq!(status, 201, "{made}");
}

/// A read-only instance refuses both changes, for an admin too, and still
/// serves the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_refuses_invite_changes() {
    let fx = serve(Options {
        read_only: true,
        managers: true,
        ..Options::default()
    })
    .await;
    let root = login(&fx, "root").await;
    let (status, body) = create(&fx, &root, json!({})).await;
    assert_eq!(status, 403, "{body}");
    let mona = login(&fx, "mona").await;
    let (status, body) = create(&fx, &mona, json!({"domain": "team"})).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(revoke(&fx, &root, 1).await, 403);
    let (status, listed) = list(&fx, &root, "").await;
    assert_eq!(status, 200, "{listed}");
    assert!(fx.auth.invites(None, None).await.unwrap().is_empty());
}

/// The list names the way in this instance offers a link's holder, so the
/// maker's screen knows whether a local-account link means anything: `local`
/// on a plain instance, `proxy` behind a forward-auth proxy. Every caller who
/// may list sees it, a manager's own list included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_names_the_way_in() {
    let fx = serve(Options::default()).await;
    let root = login(&fx, "root").await;
    let (status, body) = list(&fx, &root, "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["sign_in"], "local", "{body}");
    let mona = login(&fx, "mona").await;
    let (status, body) = list(&fx, &mona, "?state=pending").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["sign_in"], "local", "{body}");

    let fx = serve(Options {
        proxy_headers: true,
        ..Options::default()
    })
    .await;
    let resp = client()
        .get(format!("http://{}/api/v1/invites", fx.addr))
        .header("remote-user", "root")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["sign_in"], "proxy", "{body}");
}
