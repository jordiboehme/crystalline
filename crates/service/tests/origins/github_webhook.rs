//! The GitHub App's webhook, `POST /api/v1/github/webhook`, end to end: the
//! JSON API's router served on loopback, an engine with an App pointed at
//! the forge, and deliveries signed the way GitHub signs them.
//!
//! The route is public: no session, no CSRF token, no account. The
//! signature over the raw body, made with the App's webhook secret, is the
//! only proof, and it is checked before anything else is read. A delivery
//! only ever makes the governor forget (or close) something; the next
//! question asks GitHub. No role is ever read from a payload.
//!
//! Lives in the origins binary, not the rest one, because it needs the
//! forge `origins::github_app` serves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use crystalline_remote::UserTokenGrant;
use crystalline_remote::reflections_backup::BACKUP_BRANCH;
use crystalline_service::params::WriteReflectionParams;
use crystalline_service::rest::{AuthStore, RestState, Role, router};
use crystalline_service::{DomainAccess, Engine, ReflectionCaller, Scope};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};

use super::github_app::{Forge, app_env, connect, engram, files, manifest, owner_id, spawn_forge};
use super::github_identity::{MOVED, closed_reason, eventually, restart_engine};
use crate::support::MockProvider;

pub(crate) const SECRET: &str = "a webhook secret only the tests know";
const ADA_ID: u64 = 1001;
const PATH: &str = "/api/v1/github/webhook";

/// The MANIFEST lines that switch the Reflection on and home it here.
const REFLECTING: &str = "reflection: on\nreflection_home: https://kb.example.com\n";

/// An engram of `vault`.
const ROTA: &str = "crystalline://vault/team/rota";

const OVERVIEW: &str = "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n";

/// `X-Hub-Signature-256` for `body` under `secret`, computed here on its own
/// rather than with the server's helper, so a mistake in one is not
/// repeated in the other.
fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let code = mac.finalize().into_bytes();
    let hex: String = code.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

/// A repository whose root holds a MANIFEST that switches the Reflection on
/// and the engram `team/rota`.
fn reflecting_repo() -> Arc<MockProvider> {
    let mock = Arc::new(MockProvider::new());
    let head = mock.add_commit(files(&[
        ("MANIFEST.md", manifest(REFLECTING)),
        (
            "team/rota.md",
            engram("Rota", "team/rota", "Five people take turns."),
        ),
    ]));
    mock.set_branch("main", &head);
    mock
}

pub(crate) struct Fx {
    pub(crate) forge: Arc<Forge>,
    pub(crate) eng: Arc<Engine>,
    pub(crate) auth: Arc<AuthStore>,
    kb: Arc<MockProvider>,
    addr: std::net::SocketAddr,
    url: String,
    pub(crate) tmp: tempfile::TempDir,
    /// The engine's environment, for an engine started again on the same
    /// state.
    pub(crate) env: Vec<(&'static str, &'static str)>,
}

impl Fx {
    fn tokens_dir(&self) -> PathBuf {
        self.tmp.path().join("tokens")
    }
}

/// An engine with an App pointed at the forge (with `secret` as its webhook
/// secret, when given), `acme/kb` connected as `kb` and `acme/vault` as
/// `vault`, the account `ada` linked to GitHub id 1001 and `root` an admin,
/// and the JSON API served on loopback.
pub(crate) async fn setup(secret: Option<&'static str>) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let kb = reflecting_repo();
    let (forge, api) = spawn_forge(
        &[("acme/kb", kb.clone()), ("acme/vault", reflecting_repo())],
        &[("acme/kb", 1), ("acme/vault", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    let mut env = app_env();
    env.push(("CRYSTALLINE_GITHUB_APP_CLIENT_SECRET", "app-client-secret"));
    if let Some(secret) = secret {
        env.push(("CRYSTALLINE_GITHUB_APP_WEBHOOK_SECRET", secret));
    }
    let eng = Arc::new(super::github_app::engine(tmp.path(), &api, &env).await);
    let engine_env = env.clone();
    connect(&eng, "acme/kb", "kb", &tmp.path().join("kb")).await;
    connect(&eng, "acme/vault", "vault", &tmp.path().join("vault")).await;
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    auth.add_user("root", "Root", None, Role::Admin, "rootpw12345")
        .await
        .unwrap();
    auth.add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.link_github("ada", &host, ADA_ID, "ada-gh")
        .await
        .unwrap();
    eng.set_domain_access(Arc::new(
        DomainAccess::new(auth.clone()).with_governor(eng.knowledge_governor()),
    ));
    collaborators(&forge, "acme/kb", &[(ADA_ID, "ada-gh", "read")]);
    collaborators(&forge, "acme/vault", &[(ADA_ID, "ada-gh", "read")]);

    let state = RestState::new(eng.clone(), auth.clone(), &[]).unwrap();
    let app = axum::Router::new().nest("/api/v1", router(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fx {
        forge,
        eng,
        auth,
        kb,
        addr,
        url: format!("http://{addr}"),
        tmp,
        env: engine_env,
    }
}

/// Sets `repo`'s collaborators: (id, login, role).
pub(crate) fn collaborators(forge: &Forge, repo: &str, people: &[(u64, &str, &str)]) {
    forge.collaborators.lock().unwrap().insert(
        repo.to_string(),
        people
            .iter()
            .map(|(id, login, role)| json!({"id": id, "login": login, "role_name": role}))
            .collect(),
    );
}

/// How many collaborator listings of `repo` the forge answered.
fn listings(forge: &Forge, repo: &str) -> usize {
    forge
        .hits()
        .iter()
        .filter(|h| h.path == format!("/repos/{repo}/collaborators"))
        .count()
}

/// One delivery, as GitHub sends it.
pub(crate) struct Delivery<'a> {
    event: &'a str,
    id: Option<&'a str>,
    body: Vec<u8>,
    signature: Option<String>,
    cookie: Option<String>,
}

impl<'a> Delivery<'a> {
    /// `payload` as event `event` with delivery id `id`, signed with
    /// [`SECRET`].
    pub(crate) fn signed(event: &'a str, id: &'a str, payload: &Value) -> Delivery<'a> {
        let body = serde_json::to_vec(payload).unwrap();
        Delivery {
            event,
            id: Some(id),
            signature: Some(sign(SECRET, &body)),
            body,
            cookie: None,
        }
    }
}

pub(crate) async fn deliver(fx: &Fx, d: Delivery<'_>) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(format!("{}{PATH}", fx.url))
        .header("content-type", "application/json")
        .header("x-github-event", d.event)
        .header("user-agent", "GitHub-Hookshot/test");
    if let Some(id) = d.id {
        req = req.header("x-github-delivery", id);
    }
    if let Some(signature) = d.signature {
        req = req.header("x-hub-signature-256", signature);
    }
    if let Some(cookie) = d.cookie {
        req = req.header("cookie", cookie);
    }
    req.body(d.body).send().await.unwrap()
}

/// A `member` event for `repo`.
fn member_event(repo: &str) -> Value {
    json!({
        "action": "edited",
        "member": {"id": ADA_ID, "login": "ada-gh"},
        "changes": {"permission": {"from": "read", "to": "admin"}},
        "repository": {"full_name": repo, "name": repo.split('/').nth(1).unwrap()},
        "installation": {"id": 1},
        "sender": {"id": 1, "login": "octo"},
    })
}

/// Asks the governor about `kb`, so it holds a snapshot (or uses it).
pub(crate) async fn ask(fx: &Fx) -> Vec<crystalline_service::engine::GithubPerson> {
    fx.eng.github_people("kb").await.unwrap()
}

#[tokio::test]
async fn a_valid_signature_is_accepted() {
    let fx = setup(Some(SECRET)).await;
    let resp = deliver(
        &fx,
        Delivery::signed("ping", "d-ping", &json!({"zen": "Keep it simple."})),
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn a_wrong_signature_is_401_and_does_nothing() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 1);
    let body = serde_json::to_vec(&member_event("acme/kb")).unwrap();
    let resp = deliver(
        &fx,
        Delivery {
            event: "member",
            id: Some("d-wrong"),
            signature: Some(sign("another secret", &body)),
            body,
            cookie: None,
        },
    )
    .await;
    assert_eq!(resp.status(), 401);
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 1, "nothing was forgotten");
    // Not remembered either: the same id, signed right, still works.
    let resp = deliver(
        &fx,
        Delivery::signed("member", "d-wrong", &member_event("acme/kb")),
    )
    .await;
    assert_eq!(resp.status(), 200);
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 2);
}

#[tokio::test]
async fn a_missing_signature_is_401() {
    let fx = setup(Some(SECRET)).await;
    let mut d = Delivery::signed("member", "d-none", &member_event("acme/kb"));
    d.signature = None;
    assert_eq!(deliver(&fx, d).await.status(), 401);
    // A signature in another scheme (the legacy SHA-1 header's form) is no
    // signature either.
    let mut d = Delivery::signed("member", "d-sha1", &member_event("acme/kb"));
    d.signature = Some(format!("sha1={}", "0".repeat(40)));
    assert_eq!(deliver(&fx, d).await.status(), 401);
}

#[tokio::test]
async fn no_webhook_secret_configured_is_401() {
    let fx = setup(None).await;
    ask(&fx).await;
    let resp = deliver(
        &fx,
        Delivery::signed("member", "d-1", &member_event("acme/kb")),
    )
    .await;
    assert_eq!(resp.status(), 401);
    // Signed with an empty key, which is what a missing secret would be
    // read as by a careless check.
    let body = serde_json::to_vec(&member_event("acme/kb")).unwrap();
    let resp = deliver(
        &fx,
        Delivery {
            event: "member",
            id: Some("d-2"),
            signature: Some(sign("", &body)),
            body,
            cookie: None,
        },
    )
    .await;
    assert_eq!(resp.status(), 401);
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 1);
}

/// Without a GitHub App the route is not there for the internet: a 404,
/// answered before the body is read, and no log line, so a stranger cannot
/// fill the log of an instance that never set an App up.
#[tokio::test]
async fn without_an_app_the_route_is_404_and_logs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = Arc::new(super::github_app::engine(tmp.path(), "http://127.0.0.1:1", &[]).await);
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    let state = RestState::new(eng.clone(), auth, &[]).unwrap();
    let app = axum::Router::new().nest("/api/v1", router(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (logs, _guard) = crate::support::capture_logs();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let body = serde_json::to_vec(&member_event("acme/kb")).unwrap();
    for signature in [None, Some(sign(SECRET, &body))] {
        let mut req = reqwest::Client::new()
            .post(format!("http://{addr}{PATH}"))
            .header("content-type", "application/json")
            .header("x-github-event", "member")
            .header("x-github-delivery", "d-1");
        if let Some(signature) = signature {
            req = req.header("x-hub-signature-256", signature);
        }
        let resp = req.body(body.clone()).send().await.unwrap();
        assert_eq!(resp.status(), 404);
    }
    assert!(
        !logs.any_contains("webhook"),
        "no log line: {:?}",
        logs.lines()
    );
}

#[tokio::test]
async fn a_delivery_without_an_id_is_400_but_only_after_the_signature() {
    let fx = setup(Some(SECRET)).await;
    let mut d = Delivery::signed("member", "unused", &member_event("acme/kb"));
    d.id = None;
    assert_eq!(deliver(&fx, d).await.status(), 400);
    let mut d = Delivery::signed("member", "unused", &member_event("acme/kb"));
    d.id = None;
    d.signature = Some(sign("another secret", &d.body));
    assert_eq!(deliver(&fx, d).await.status(), 401);
}

#[tokio::test]
async fn a_body_over_the_limit_is_refused_before_the_signature_is_checked() {
    let fx = setup(Some(SECRET)).await;
    let body = vec![b' '; 25 * 1024 * 1024 + 1];
    let resp = deliver(
        &fx,
        Delivery {
            event: "ping",
            id: Some("d-big"),
            signature: Some(sign(SECRET, &body)),
            body,
            cookie: None,
        },
    )
    .await;
    assert_eq!(resp.status(), 413);
}

#[tokio::test]
async fn a_browser_session_neither_opens_nor_closes_the_route() {
    let fx = setup(Some(SECRET)).await;
    let login = reqwest::Client::new()
        .post(format!("{}/api/v1/auth/login", fx.url))
        .json(&json!({"name": "root", "password": "rootpw12345"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ");
    // A session and no CSRF token: the signature is what counts.
    let mut d = Delivery::signed("ping", "d-session", &json!({"zen": "Half measures."}));
    d.cookie = Some(cookie.clone());
    assert_eq!(deliver(&fx, d).await.status(), 200);
    // And an admin's session is no substitute for it.
    let mut d = Delivery::signed("ping", "d-session-2", &json!({"zen": "Half measures."}));
    d.cookie = Some(cookie);
    d.signature = None;
    assert_eq!(deliver(&fx, d).await.status(), 401);
}

#[tokio::test]
async fn a_replayed_delivery_is_ignored() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    let first = deliver(
        &fx,
        Delivery::signed("member", "d-replay", &member_event("acme/kb")),
    )
    .await;
    assert_eq!(first.status(), 200);
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 2);
    let again = deliver(
        &fx,
        Delivery::signed("member", "d-replay", &member_event("acme/kb")),
    )
    .await;
    assert_eq!(again.status(), 200);
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 2, "the replay did no work");
}

#[tokio::test]
async fn a_delivery_is_forgotten_after_a_day() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    let now = Utc::now().timestamp();
    // Seen an hour ago: still a replay.
    assert!(
        fx.auth
            .remember_delivery_at("d-hour", now - 3600)
            .await
            .unwrap()
    );
    deliver(
        &fx,
        Delivery::signed("member", "d-hour", &member_event("acme/kb")),
    )
    .await;
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 1);
    // Seen a day and a second ago: forgotten, so it is a new delivery.
    assert!(
        fx.auth
            .remember_delivery_at("d-day", now - 86_401)
            .await
            .unwrap()
    );
    let resp = deliver(
        &fx,
        Delivery::signed("member", "d-day", &member_event("acme/kb")),
    )
    .await;
    assert_eq!(resp.status(), 200);
    ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 2);
}

#[tokio::test]
async fn a_member_event_invalidates_the_repo_so_the_next_request_asks_github() {
    let fx = setup(Some(SECRET)).await;
    let people = ask(&fx).await;
    assert_eq!(people[0].github_role, "read");
    ask(&fx).await;
    assert_eq!(
        listings(&fx.forge, "acme/kb"),
        1,
        "the second question is cached"
    );
    collaborators(&fx.forge, "acme/kb", &[(ADA_ID, "ada-gh", "write")]);
    let resp = deliver(
        &fx,
        Delivery::signed("member", "d-member", &member_event("acme/kb")),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let people = ask(&fx).await;
    assert_eq!(listings(&fx.forge, "acme/kb"), 2);
    assert_eq!(people[0].github_role, "write");
    // The other repository was not touched.
    fx.eng.github_people("vault").await.unwrap();
    fx.eng.github_people("vault").await.unwrap();
    assert_eq!(listings(&fx.forge, "acme/vault"), 1);
}

#[tokio::test]
async fn a_role_in_the_payload_is_never_used() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    // The payload says admin everywhere it could; GitHub's listing says read.
    let mut payload = member_event("acme/kb");
    payload["member"]["role_name"] = json!("admin");
    payload["member"]["permissions"] = json!({"admin": true, "push": true, "pull": true});
    payload["permission"] = json!("admin");
    let resp = deliver(&fx, Delivery::signed("member", "d-role", &payload)).await;
    assert_eq!(resp.status(), 200);
    let people = ask(&fx).await;
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].github_role, "read");
    assert_eq!(people[0].account.as_deref(), Some("ada"));
    assert_eq!(
        fx.eng
            .domain_right(
                &Scope::User {
                    account: "ada".into(),
                    admin: false,
                },
                "kb",
            )
            .await
            .unwrap(),
        crystalline_service::scope::DomainRight::Read
    );
}

/// A `repository` `renamed` body: `from` (a name on `owner`) is now
/// `owner/to`, with the id the payload claims.
fn renamed(owner: &str, from: &str, to: &str, id: u64) -> Value {
    json!({
        "action": "renamed",
        "changes": {"repository": {"name": {"from": from}}},
        "repository": {"id": id, "full_name": format!("{owner}/{to}"), "name": to, "owner": {"login": owner}},
        "installation": {"id": 1},
    })
}

#[tokio::test]
async fn a_renamed_repository_updates_the_origin() {
    let fx = setup(Some(SECRET)).await;
    let id = fx.forge.id_of("acme/kb").unwrap();
    fx.forge.rename("acme/kb", "acme/handbook");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "repository",
            "d-rename",
            &renamed("acme", "kb", "handbook", id),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/handbook");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault");
    let saved = std::fs::read_to_string(fx.tmp.path().join("config.yaml")).unwrap();
    assert!(saved.contains("acme/handbook"), "{saved}");
    assert!(!saved.contains("acme/kb"), "{saved}");
    // GitHub was asked, by id and by the old name.
    let hits = fx.forge.hits();
    assert!(hits.iter().any(|h| h.path == format!("/repositories/{id}")));
    assert!(hits.iter().any(|h| h.path == "/repos/acme/kb"));
}

/// A signed body can be sent again with a fresh delivery id. One that says
/// what is no longer true moves nothing: GitHub is asked, and says
/// otherwise.
#[tokio::test]
async fn a_stale_renamed_body_sent_again_with_a_fresh_id_moves_nothing() {
    let fx = setup(Some(SECRET)).await;
    let id = fx.forge.id_of("acme/kb").unwrap();
    let forward = renamed("acme", "kb", "handbook", id);
    fx.forge.rename("acme/kb", "acme/handbook");
    deliver(&fx, Delivery::signed("repository", "d-forward", &forward)).await;
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/handbook");
    // And back again.
    fx.forge.rename("acme/handbook", "acme/kb");
    deliver(
        &fx,
        Delivery::signed(
            "repository",
            "d-back",
            &renamed("acme", "handbook", "kb", id),
        ),
    )
    .await;
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    // The first body once more, under a new id.
    let resp = deliver(
        &fx,
        Delivery::signed("repository", "d-forward-again", &forward),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
}

/// A rename GitHub does not confirm (here: the id names another
/// repository, and the old name was never renamed) moves nothing.
#[tokio::test]
async fn a_rename_github_does_not_confirm_moves_nothing() {
    let fx = setup(Some(SECRET)).await;
    let vault = fx.forge.id_of("acme/vault").unwrap();
    let kb = fx.forge.id_of("acme/kb").unwrap();
    for (id, delivery) in [(vault, "d-other-id"), (kb, "d-not-renamed")] {
        let resp = deliver(
            &fx,
            Delivery::signed(
                "repository",
                delivery,
                &renamed("acme", "kb", "handbook", id),
            ),
        )
        .await;
        assert_eq!(resp.status(), 200);
        assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    }
    // No id at all: nothing to confirm with.
    let mut payload = renamed("acme", "kb", "handbook", kb);
    payload["repository"].as_object_mut().unwrap().remove("id");
    deliver(&fx, Delivery::signed("repository", "d-no-id", &payload)).await;
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
}

/// Ruling R32: a move to another owner is never followed. The domain is
/// closed until an admin reconnects it, whatever GitHub says later.
#[tokio::test]
async fn a_transferred_repository_closes_its_domain() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    let payload = json!({
        "action": "transferred",
        "changes": {"owner": {"from": {"organization": {"login": "ACME"}}}},
        "repository": {"id": 100, "full_name": "newco/kb", "name": "kb", "owner": {"login": "newco"}},
        "installation": {"id": 1},
    });
    let resp = deliver(&fx, Delivery::signed("repository", "d-transfer", &payload)).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    let closed = fx.eng.github_access_closed();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].domain, "kb");
    assert_eq!(
        closed[0].reason,
        "the repository moved to newco/kb on GitHub; disconnect the domain and connect it again"
    );
    // An installation event that rechecks closed domains does not open it.
    let resp = deliver(
        &fx,
        Delivery::signed(
            "installation",
            "d-new-perms",
            &json!({"action": "new_permissions_accepted", "installation": {"id": 1}}),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fx.eng.github_access_closed().len(), 1);
}

pub(crate) fn origin_repo(eng: &Engine, domain: &str) -> String {
    eng.config().domains[domain]
        .origin
        .as_ref()
        .unwrap()
        .repo
        .clone()
}

/// Every file under `dir`, read as text.
fn texts(dir: &Path) -> Vec<String> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect()
}

#[tokio::test]
async fn github_app_authorization_drops_the_token_and_keeps_the_link() {
    let fx = setup(Some(SECRET)).await;
    fx.forge
        .users
        .lock()
        .unwrap()
        .insert("ghu_ada".to_string(), (ADA_ID, "ada-gh".to_string()));
    fx.eng
        .link_github(
            "ada",
            UserTokenGrant {
                access_token: "ghu_ada".to_string(),
                refresh_token: Some("ghr_ada".to_string()),
                expires_at: Some(Utc::now() + chrono::Duration::hours(8)),
                refresh_expires_at: Some(Utc::now() + chrono::Duration::days(180)),
            },
        )
        .await
        .unwrap();
    assert!(
        texts(&fx.tokens_dir())
            .iter()
            .any(|t| t.contains("ghu_ada"))
    );
    let payload = json!({"action": "revoked", "sender": {"id": ADA_ID, "login": "ada-gh"}});
    let resp = deliver(
        &fx,
        Delivery::signed("github_app_authorization", "d-revoke", &payload),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let status = fx.eng.github_link_status("ada").await.unwrap();
    assert!(status.linked, "the link stays");
    assert_eq!(status.github_id, Some(ADA_ID));
    assert!(!status.token_live, "the token is marked gone");
    assert!(
        !texts(&fx.tokens_dir())
            .iter()
            .any(|t| t.contains("ghu_ada")),
        "the token is deleted"
    );
}

/// The machine owner's agent, which reads every domain.
fn owner() -> ReflectionCaller {
    ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false)
}

#[tokio::test]
async fn a_repository_made_public_rescans_the_reflection() {
    let fx = setup(Some(SECRET)).await;
    let page = WriteReflectionParams {
        domain: "kb".into(),
        path: "overview.md".into(),
        markdown: Some(OVERVIEW.into()),
        sources: BTreeMap::from([("Team".to_string(), vec![ROTA.to_string()])]),
        ..Default::default()
    };
    fx.eng.write_reflection(&page, &owner()).await.unwrap();
    let later = || Instant::now() + Duration::from_secs(120);
    fx.eng.reflection_backup_tick(later()).await;
    let head = || {
        fx.kb
            .commit_paths(&fx.kb.branch_commit(BACKUP_BRANCH).unwrap())
    };
    assert!(head().contains(&"reflections/overview.md".to_string()));

    // Everybody reads kb now, and only its collaborators read vault.
    fx.forge
        .public
        .lock()
        .unwrap()
        .insert("acme/kb".to_string());
    let payload = json!({
        "action": "publicized",
        "repository": {"full_name": "acme/kb", "name": "kb", "private": false},
        "installation": {"id": 1},
    });
    let resp = deliver(&fx, Delivery::signed("repository", "d-public", &payload)).await;
    assert_eq!(resp.status(), 200);
    let mut held = Vec::new();
    for _ in 0..100 {
        fx.eng.reflection_backup_tick(later()).await;
        held = fx.eng.reflection_backup_held_back("kb");
        if !held.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(held, vec!["overview.md".to_string()]);
    assert!(
        !head().iter().any(|p| p.contains("overview.md")),
        "{:?}",
        head()
    );
}

#[tokio::test]
async fn a_removed_installation_repository_closes_its_domain() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    assert!(fx.eng.github_access_closed().is_empty());
    fx.forge.uninstall("acme/kb");
    let payload = json!({
        "action": "removed",
        "installation": {"id": 1},
        "repository_selection": "selected",
        "repositories_added": [],
        "repositories_removed": [{"id": 5, "full_name": "acme/kb", "name": "kb"}],
    });
    let resp = deliver(
        &fx,
        Delivery::signed("installation_repositories", "d-removed", &payload),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let closed = fx.eng.github_access_closed();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].domain, "kb");
    assert_eq!(
        closed[0].reason,
        "acme/kb was removed from the GitHub App's installation: add it back there or disconnect the domain"
    );
    assert!(fx.eng.github_people("kb").await.is_none());

    // Added back: the recheck opens it again.
    fx.forge.install("acme/kb", 1);
    let payload = json!({
        "action": "added",
        "installation": {"id": 1},
        "repository_selection": "selected",
        "repositories_added": [{"id": 5, "full_name": "acme/kb", "name": "kb"}],
        "repositories_removed": [],
    });
    let resp = deliver(
        &fx,
        Delivery::signed("installation_repositories", "d-added", &payload),
    )
    .await;
    assert_eq!(resp.status(), 200);
    for _ in 0..100 {
        if fx.eng.github_access_closed().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(fx.eng.github_access_closed().is_empty());
}

#[tokio::test]
async fn a_deleted_repository_closes_its_domain() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.uninstall("acme/kb");
    let payload = json!({
        "action": "deleted",
        "repository": {"full_name": "acme/kb", "name": "kb"},
        "installation": {"id": 1},
    });
    let resp = deliver(&fx, Delivery::signed("repository", "d-deleted", &payload)).await;
    assert_eq!(resp.status(), 200);
    let closed = fx.eng.github_access_closed();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert_eq!(closed[0].domain, "kb");
    assert_eq!(
        closed[0].reason,
        "acme/kb was deleted on GitHub: disconnect the domain or connect it to another repository"
    );
}

#[tokio::test]
async fn an_event_for_an_unconnected_repository_does_nothing() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.eng.github_people("vault").await.unwrap();
    let before = fx.forge.hits().len();
    for (event, id, payload) in [
        ("member", "d-u1", member_event("acme/other")),
        (
            "repository",
            "d-u2",
            json!({"action": "deleted", "repository": {"full_name": "acme/other"}}),
        ),
        (
            "installation_repositories",
            "d-u3",
            json!({"action": "removed", "installation": {"id": 1},
                   "repositories_added": [], "repositories_removed": [{"full_name": "acme/other"}]}),
        ),
    ] {
        let resp = deliver(&fx, Delivery::signed(event, id, &payload)).await;
        assert_eq!(resp.status(), 200, "{event}");
    }
    ask(&fx).await;
    fx.eng.github_people("vault").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fx.forge.hits().len(),
        before,
        "{:?}",
        &fx.forge.hits()[before..]
    );
    assert!(fx.eng.github_access_closed().is_empty());
}

/// An uninstalled (or suspended) installation closes every connected
/// repository the App client knows it covers, not only those the payload
/// lists.
#[tokio::test]
async fn an_uninstalled_installation_closes_every_repository_it_covers() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.eng.github_people("vault").await.unwrap();
    fx.forge.uninstall("acme/kb");
    fx.forge.uninstall("acme/vault");
    let payload = json!({
        "action": "deleted",
        "installation": {"id": 1},
        "repositories": [{"full_name": "acme/kb"}],
    });
    let resp = deliver(
        &fx,
        Delivery::signed("installation", "d-uninstall", &payload),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let closed: Vec<String> = fx
        .eng
        .github_access_closed()
        .into_iter()
        .map(|c| c.domain)
        .collect();
    assert_eq!(closed, vec!["kb".to_string(), "vault".to_string()]);
}

/// When the recheck opens a closed domain again, the Reflection is looked
/// at again too: a page whose repository now has more readers than its
/// source is held back from the backup branch.
#[tokio::test]
async fn a_reopened_domain_rescans_the_reflection() {
    let fx = setup(Some(SECRET)).await;
    let page = WriteReflectionParams {
        domain: "kb".into(),
        path: "overview.md".into(),
        markdown: Some(OVERVIEW.into()),
        sources: BTreeMap::from([("Team".to_string(), vec![ROTA.to_string()])]),
        ..Default::default()
    };
    fx.eng.write_reflection(&page, &owner()).await.unwrap();
    let later = || Instant::now() + Duration::from_secs(120);
    fx.eng.reflection_backup_tick(later()).await;
    assert!(fx.eng.reflection_backup_held_back("kb").is_empty());

    // Closed without a rescan (as at start, before anybody asked), and
    // wider on GitHub meanwhile.
    fx.eng
        .github_governor()
        .close("acme/kb", "removed from the App's installation".into());
    collaborators(
        &fx.forge,
        "acme/kb",
        &[(ADA_ID, "ada-gh", "read"), (2, "bob-gh", "read")],
    );
    let payload = json!({
        "action": "added",
        "installation": {"id": 1},
        "repository_selection": "selected",
        "repositories_added": [{"id": 100, "full_name": "acme/kb", "name": "kb"}],
        "repositories_removed": [],
    });
    let resp = deliver(
        &fx,
        Delivery::signed("installation_repositories", "d-back", &payload),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let mut held = Vec::new();
    for _ in 0..100 {
        fx.eng.reflection_backup_tick(later()).await;
        held = fx.eng.reflection_backup_held_back("kb");
        if !held.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(fx.eng.github_access_closed().is_empty());
    assert_eq!(held, vec!["overview.md".to_string()]);
}

/// Sends a POST to the webhook that declares a body of `declared` bytes and
/// sends only a few of them, and answers the status line, or `None` when
/// the server waits for the rest of the body.
async fn status_without_the_body(
    fx: &Fx,
    headers: &[(&str, &str)],
    declared: usize,
) -> Option<u16> {
    status_after_a_stall(fx, headers, declared, Duration::from_secs(3)).await
}

/// [`status_without_the_body`], waiting up to `wait` for the answer.
async fn status_after_a_stall(
    fx: &Fx,
    headers: &[(&str, &str)],
    declared: usize,
    wait: Duration,
) -> Option<u16> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(fx.addr).await.unwrap();
    let mut head = format!(
        "POST {PATH} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n",
        fx.addr
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n{\"zen\":");
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let mut buf = [0u8; 1024];
    let read = tokio::time::timeout(wait, async {
        loop {
            let n = stream.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            answer.extend_from_slice(&buf[..n]);
            if answer.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
    })
    .await;
    read.ok()?;
    let text = String::from_utf8_lossy(&answer);
    text.split_whitespace().nth(1)?.parse().ok()
}

#[tokio::test]
async fn an_unconfigured_instance_answers_401_without_reading_the_body() {
    let fx = setup(None).await;
    let status = status_without_the_body(
        &fx,
        &[
            ("x-github-event", "ping"),
            ("x-github-delivery", "d-raw"),
            ("x-hub-signature-256", &format!("sha256={}", "a".repeat(64))),
        ],
        20 * 1024 * 1024,
    )
    .await;
    assert_eq!(status, Some(401));
}

#[tokio::test]
async fn a_missing_or_malformed_signature_header_is_401_without_reading_the_body() {
    let fx = setup(Some(SECRET)).await;
    let short = format!("sha256={}", "a".repeat(63));
    let upper = format!("sha256={}", "A".repeat(64));
    let sha1 = format!("sha1={}", "a".repeat(40));
    let long = format!("sha256={}", "a".repeat(65));
    for signature in [
        None,
        Some(short.as_str()),
        Some(upper.as_str()),
        Some(sha1.as_str()),
        Some(long.as_str()),
    ] {
        let mut headers = vec![("x-github-event", "ping"), ("x-github-delivery", "d-raw")];
        if let Some(signature) = signature {
            headers.push(("x-hub-signature-256", signature));
        }
        let status = status_without_the_body(&fx, &headers, 20 * 1024 * 1024).await;
        assert_eq!(status, Some(401), "{signature:?}");
    }
}

#[tokio::test]
async fn a_large_signed_body_is_taken() {
    let fx = setup(Some(SECRET)).await;
    let payload = json!({"zen": "x".repeat(12 * 1024 * 1024)});
    let resp = deliver(&fx, Delivery::signed("ping", "d-large", &payload)).await;
    assert_eq!(resp.status(), 200);
}

/// The same delivery twice at once does the work once.
#[tokio::test]
async fn two_identical_deliveries_at_once_do_the_work_once() {
    let fx = setup(Some(SECRET)).await;
    let id = fx.forge.id_of("acme/kb").unwrap();
    fx.forge.rename("acme/kb", "acme/handbook");
    let body = renamed("acme", "kb", "handbook", id);
    let (a, b) = tokio::join!(
        deliver(&fx, Delivery::signed("repository", "d-twice", &body)),
        deliver(&fx, Delivery::signed("repository", "d-twice", &body)),
    );
    assert_eq!((a.status().as_u16(), b.status().as_u16()), (200, 200));
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/handbook");
    let asked = fx
        .forge
        .hits()
        .iter()
        .filter(|h| h.path == format!("/repositories/{id}"))
        .count();
    assert_eq!(asked, 1, "GitHub was asked once");
}

/// A body that does not arrive within ten seconds is answered 408 and its
/// slot is free again: four stalled senders do not hold the route.
#[tokio::test]
async fn a_body_that_does_not_arrive_is_408_and_frees_its_slot() {
    let fx = setup(Some(SECRET)).await;
    let signature = format!("sha256={}", "a".repeat(64));
    let headers = [
        ("x-github-event", "ping"),
        ("x-github-delivery", "d-slow"),
        ("x-hub-signature-256", signature.as_str()),
    ];
    let stalled = |_| status_after_a_stall(&fx, &headers, 1024 * 1024, Duration::from_secs(20));
    let answers = futures::future::join_all((0..4).map(stalled)).await;
    assert_eq!(answers, vec![Some(408); 4]);
    let resp = deliver(
        &fx,
        Delivery::signed("ping", "d-after", &json!({"zen": "Slow down."})),
    )
    .await;
    assert_eq!(resp.status(), 200);
}

/// A confirmed rename leaves a domain alone whose recorded repository is
/// another one of that name (the name was reused before).
#[tokio::test]
async fn a_rename_skips_a_domain_recorded_for_another_repository() {
    let fx = setup(Some(SECRET)).await;
    let dir = fx.tmp.path().join("origins").join("kb");
    let mut state = crystalline_remote::state::OriginState::load(&dir)
        .unwrap()
        .unwrap();
    let mut ids = state.repository_ids.unwrap();
    ids.repository = 999;
    state.repository_ids = Some(ids);
    state.save(&dir).unwrap();
    let id = fx.forge.id_of("acme/kb").unwrap();
    fx.forge.rename("acme/kb", "acme/handbook");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "repository",
            "d-skip",
            &renamed("acme", "kb", "handbook", id),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
}

/// A `repository` event with `action` for `acme/kb`.
fn kb_repository_event(action: &str) -> Value {
    json!({
        "action": action,
        "repository": {"full_name": "acme/kb", "name": "kb", "archived": action == "archived"},
        "installation": {"id": 1},
    })
}

/// What the archived domain answers a write with.
const KB_ARCHIVED: &str = "'kb' is read-only because its GitHub repository acme/kb is archived; reading still works, and the domain can change again once the repository is unarchived on GitHub";

#[tokio::test]
async fn a_signed_archived_delivery_makes_the_domain_read_only() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.archive("acme/kb");
    let resp = deliver(
        &fx,
        Delivery::signed("repository", "d-archived", &kb_repository_event("archived")),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        fx.eng.github_archived_repo("kb").as_deref(),
        Some("acme/kb")
    );
    assert_eq!(fx.eng.github_archived_repo("vault"), None);
    let page = WriteReflectionParams {
        domain: "kb".into(),
        path: "overview.md".into(),
        markdown: Some(OVERVIEW.into()),
        ..Default::default()
    };
    let refused = fx.eng.write_reflection(&page, &owner()).await.unwrap_err();
    assert_eq!(refused.to_string(), KB_ARCHIVED);

    // Unarchived on GitHub: the delivery asks again, and the answer opens it.
    fx.forge.unarchive("acme/kb");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "repository",
            "d-unarchived",
            &kb_repository_event("unarchived"),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    for _ in 0..250 {
        if fx.eng.github_archived_repo("kb").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(fx.eng.github_archived_repo("kb"), None);
    fx.eng.write_reflection(&page, &owner()).await.unwrap();
}

#[tokio::test]
async fn the_backup_tick_makes_no_push_for_an_archived_domain() {
    let fx = setup(Some(SECRET)).await;
    let later = || Instant::now() + Duration::from_secs(120);
    let page = |path: &str| WriteReflectionParams {
        domain: "kb".into(),
        path: path.into(),
        markdown: Some(OVERVIEW.into()),
        ..Default::default()
    };
    fx.eng
        .write_reflection(&page("overview.md"), &owner())
        .await
        .unwrap();
    fx.eng.reflection_backup_tick(later()).await;
    let head = || fx.kb.branch_commit(BACKUP_BRANCH);
    let before = head();
    assert!(before.is_some(), "the first backup went out");
    // A change waits to be backed up when the repository is archived.
    fx.eng
        .write_reflection(&page("second.md"), &owner())
        .await
        .unwrap();
    fx.forge.archive("acme/kb");
    let resp = deliver(
        &fx,
        Delivery::signed("repository", "d-archived", &kb_repository_event("archived")),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let commits = fx.forge.commits().len();
    let outcomes = fx.eng.reflection_backup_tick(later()).await;
    assert!(
        outcomes.iter().all(|(domain, _)| domain != "kb"),
        "{outcomes:?}"
    );
    assert_eq!(head(), before, "nothing was pushed for kb");
    assert_eq!(fx.forge.commits().len(), commits);
    // Unarchived: the change goes out on the next tick.
    fx.forge.unarchive("acme/kb");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "repository",
            "d-unarchived",
            &kb_repository_event("unarchived"),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    for _ in 0..250 {
        if fx.eng.github_archived_repo("kb").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    fx.eng.reflection_backup_tick(later()).await;
    assert_ne!(head(), before, "the change goes out once unarchived");
}

/// An `organization` `renamed` body: the account `from` is now `to`, with
/// the id the payload claims.
fn org_renamed(from: &str, to: &str, id: u64) -> Value {
    json!({
        "action": "renamed",
        "changes": {"login": {"from": from}},
        "organization": {"login": to, "id": id},
        "installation": {"id": 1},
    })
}

/// An `organization` `deleted` body for `login`, with the id it claims.
fn org_deleted(login: &str, id: u64) -> Value {
    json!({
        "action": "deleted",
        "organization": {"login": login, "id": id},
        "installation": {"id": 1},
    })
}

/// The repository `domain`'s origin state names.
fn state_repo(fx: &Fx, domain: &str) -> String {
    crystalline_remote::state::OriginState::load(&fx.tmp.path().join("origins").join(domain))
        .unwrap()
        .unwrap()
        .repo
}

/// How many `GET /repositories/{id}` the forge answered.
fn by_id_hits(fx: &Fx) -> usize {
    fx.forge
        .hits()
        .iter()
        .filter(|h| h.path.starts_with("/repositories/"))
        .count()
}

fn invalidate(fx: &Fx, repo: &str) {
    fx.eng
        .invalidate_github_access(crystalline_service::engine::Invalidate::Repo(
            repo.to_string(),
        ));
}

/// Asks the governor about `kb` and `vault`, so both hold a fresh snapshot
/// and no lookup runs for the next five minutes: a lookup after the forge
/// renamed the owner would follow the rename on its own (the passive path).
async fn settle_both(fx: &Fx) {
    ask(fx).await;
    fx.eng.github_people("vault").await.unwrap();
}

#[tokio::test]
async fn an_organization_rename_github_confirms_moves_the_origin() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    let (kb, vault) = (
        fx.forge.id_of("acme/kb").unwrap(),
        fx.forge.id_of("acme/vault").unwrap(),
    );
    fx.forge.rename_owner("acme", "acme-labs");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "organization",
            "d-org-rename",
            &org_renamed("acme", "acme-labs", owner_id("acme")),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme-labs/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme-labs/vault");
    let saved = std::fs::read_to_string(fx.tmp.path().join("config.yaml")).unwrap();
    assert!(saved.contains("acme-labs/kb"), "{saved}");
    assert!(saved.contains("acme-labs/vault"), "{saved}");
    assert!(!saved.contains("acme/"), "{saved}");
    for domain in ["kb", "vault"] {
        let want = format!("acme-labs/{domain}");
        assert!(
            eventually(|| state_repo(&fx, domain) == want).await,
            "{domain}: {}",
            state_repo(&fx, domain)
        );
    }
    // GitHub was asked about each recorded repository by its id.
    let hits = fx.forge.hits();
    for id in [kb, vault] {
        assert!(hits.iter().any(|h| h.path == format!("/repositories/{id}")));
    }
    // And the domain stays open under its new name.
    assert!(fx.eng.github_people("kb").await.is_some());
    assert!(fx.eng.github_access_closed().is_empty());
}

/// Review Focus 3: a rename GitHub does not confirm, or one the payload
/// cannot prove, moves nothing.
#[tokio::test]
async fn an_organization_rename_github_does_not_confirm_moves_nothing() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    let acme = owner_id("acme");
    // The organization was never renamed: GitHub answers the old name.
    let resp = deliver(
        &fx,
        Delivery::signed(
            "organization",
            "d-org-not-renamed",
            &org_renamed("acme", "acme-labs", acme),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(by_id_hits(&fx) > 0, "GitHub was asked");
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault");

    // Renamed now, but the body names another organization id, or carries
    // no installation or no id: nothing is asked and nothing moves.
    fx.forge.rename_owner("acme", "acme-labs");
    let before = by_id_hits(&fx);
    let mut no_installation = org_renamed("acme", "acme-labs", acme);
    no_installation
        .as_object_mut()
        .unwrap()
        .remove("installation");
    let mut no_id = org_renamed("acme", "acme-labs", acme);
    no_id["organization"].as_object_mut().unwrap().remove("id");
    for (delivery, payload) in [
        ("d-org-other-id", org_renamed("acme", "acme-labs", 4242)),
        ("d-org-no-installation", no_installation),
        ("d-org-no-id", no_id),
    ] {
        let resp = deliver(&fx, Delivery::signed("organization", delivery, &payload)).await;
        assert_eq!(resp.status(), 200, "{delivery}");
        assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb", "{delivery}");
        assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault", "{delivery}");
    }
    assert_eq!(by_id_hits(&fx), before);
    let saved = std::fs::read_to_string(fx.tmp.path().join("config.yaml")).unwrap();
    assert!(!saved.contains("acme-labs"), "{saved}");
}

/// Review Focus 3: a domain with no recorded ids proves nothing, so it
/// keeps its origin while a domain with matching ids moves.
#[tokio::test]
async fn an_organization_rename_leaves_a_domain_without_recorded_ids_alone() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    let dir = fx.tmp.path().join("origins").join("kb");
    let mut state = crystalline_remote::state::OriginState::load(&dir)
        .unwrap()
        .unwrap();
    state.repository_ids = None;
    state.save(&dir).unwrap();
    let kb = fx.forge.id_of("acme/kb").unwrap();
    fx.forge.rename_owner("acme", "acme-labs");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "organization",
            "d-org-unrecorded",
            &org_renamed("acme", "acme-labs", owner_id("acme")),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme-labs/vault");
    assert!(
        !fx.forge
            .hits()
            .iter()
            .any(|h| h.path == format!("/repositories/{kb}")),
        "nothing recorded, nothing to ask about"
    );
}

/// Review Focus 3: a signed body sent again under a fresh id after the
/// organization was renamed back moves nothing.
#[tokio::test]
async fn a_stale_organization_renamed_body_sent_again_moves_nothing() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    let acme = owner_id("acme");
    let forward = org_renamed("acme", "acme-labs", acme);
    fx.forge.rename_owner("acme", "acme-labs");
    deliver(
        &fx,
        Delivery::signed("organization", "d-org-forward", &forward),
    )
    .await;
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme-labs/kb");
    // And back again.
    fx.forge.rename_owner("acme-labs", "acme");
    deliver(
        &fx,
        Delivery::signed(
            "organization",
            "d-org-back",
            &org_renamed("acme-labs", "acme", acme),
        ),
    )
    .await;
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault");
    // The first body once more, under a new id.
    let resp = deliver(
        &fx,
        Delivery::signed("organization", "d-org-forward-again", &forward),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault");
}

#[tokio::test]
async fn a_read_only_instance_logs_an_organization_rename_and_moves_nothing() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    let config = fx.tmp.path().join("config.yaml");
    let before = std::fs::read_to_string(&config).unwrap();
    fx.forge.rename_owner("acme", "acme-labs");
    let eng = restart_engine(&fx, true).await;
    let (logs, _guard) = crate::support::capture_logs();
    let outcome = eng
        .apply_github_event(
            "organization",
            &org_renamed("acme", "acme-labs", owner_id("acme")),
        )
        .await
        .unwrap();
    assert!(!outcome.readers_changed);
    assert_eq!(origin_repo(&eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&eng, "vault"), "acme/vault");
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state_repo(&fx, "kb"), "acme/kb");
    assert_eq!(state_repo(&fx, "vault"), "acme/vault");
    assert!(
        logs.any_contains("this instance is read-only"),
        "{:?}",
        logs.lines()
    );
}

/// An organization rename that moves a domain carries its repository's
/// archived mark to the new name, so the domain stays read-only.
#[tokio::test]
async fn an_organization_rename_carries_the_archived_mark() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    fx.forge.archive("acme/kb");
    let resp = deliver(
        &fx,
        Delivery::signed("repository", "d-archived", &kb_repository_event("archived")),
    )
    .await;
    assert_eq!(resp.status(), 200);
    // The lookups the delivery started have answered.
    settle_both(&fx).await;
    assert_eq!(
        fx.eng.github_archived_repo("kb").as_deref(),
        Some("acme/kb")
    );
    fx.forge.rename_owner("acme", "acme-labs");
    let outcome = fx
        .eng
        .apply_github_event(
            "organization",
            &org_renamed("acme", "acme-labs", owner_id("acme")),
        )
        .await
        .unwrap();
    assert!(!outcome.readers_changed);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme-labs/kb");
    assert_eq!(
        fx.eng.github_archived_repo("kb").as_deref(),
        Some("acme-labs/kb")
    );
    assert_eq!(fx.eng.github_archived_repo("vault"), None);
}

#[tokio::test]
async fn a_deleted_organization_closes_its_domains() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.eng.github_people("vault").await.unwrap();
    // A body naming another organization of that login closes nothing.
    let resp = deliver(
        &fx,
        Delivery::signed("organization", "d-org-other", &org_deleted("acme", 4242)),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(fx.eng.github_access_closed().is_empty());

    const REASON: &str = "the GitHub organization acme was deleted: disconnect the domain or connect it to another repository";
    let delete = org_deleted("acme", owner_id("acme"));
    for delivery in ["d-org-deleted", "d-org-deleted-again"] {
        let resp = deliver(&fx, Delivery::signed("organization", delivery, &delete)).await;
        assert_eq!(resp.status(), 200);
        let mut closed = fx.eng.github_access_closed();
        closed.sort_by(|a, b| a.domain.cmp(&b.domain));
        assert_eq!(closed.len(), 2, "{delivery}: {closed:?}");
        assert_eq!(closed[0].domain, "kb");
        assert_eq!(closed[1].domain, "vault");
        assert!(closed.iter().all(|c| c.reason == REASON), "{closed:?}");
        assert!(fx.eng.github_people("kb").await.is_none());
        // GitHub still shows the App both repositories: the recheck after
        // the grace (asked at once here by naming them) opens them again,
        // so a replayed delete does not keep them shut.
        invalidate(&fx, "acme/kb");
        invalidate(&fx, "acme/vault");
        assert!(
            eventually(|| fx.eng.github_access_closed().is_empty()).await,
            "{delivery}: {:?}",
            fx.eng.github_access_closed()
        );
        assert!(fx.eng.github_people("kb").await.is_some());
    }
}

/// The passive path: with the webhook not delivered, the next lookup sees
/// the same repository and owner ids under the new login and follows it.
#[tokio::test]
async fn an_organization_rename_without_a_webhook_moves_the_origin_on_a_lookup() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.rename_owner("acme", "acme-labs");
    invalidate(&fx, "acme/kb");
    assert!(fx.eng.github_people("kb").await.is_some());
    assert!(closed_reason(&fx.eng, "kb").is_none());
    assert!(eventually(|| origin_repo(&fx.eng, "kb") == "acme-labs/kb").await);
    let saved = std::fs::read_to_string(fx.tmp.path().join("config.yaml")).unwrap();
    assert!(saved.contains("acme-labs/kb"), "{saved}");
}

/// The passive path never follows a transfer: another owner id closes the
/// domain.
#[tokio::test]
async fn a_transfer_to_another_organization_without_a_webhook_still_closes_the_domain() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.forge.transfer("acme/kb", "newco/kb");
    invalidate(&fx, "acme/kb");
    assert!(fx.eng.github_people("kb").await.is_none());
    assert_eq!(closed_reason(&fx.eng, "kb").as_deref(), Some(MOVED));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
}

#[tokio::test]
async fn an_organization_event_for_an_owner_no_domain_is_connected_to_does_nothing() {
    let fx = setup(Some(SECRET)).await;
    ask(&fx).await;
    fx.eng.github_people("vault").await.unwrap();
    let before = fx.forge.hits().len();
    let mut no_id = org_renamed("otherco", "otherco-labs", owner_id("otherco"));
    no_id["organization"].as_object_mut().unwrap().remove("id");
    for (id, payload) in [
        (
            "d-o1",
            org_renamed("otherco", "otherco-labs", owner_id("otherco")),
        ),
        ("d-o2", org_deleted("otherco", owner_id("otherco"))),
        ("d-o3", no_id),
    ] {
        let resp = deliver(&fx, Delivery::signed("organization", id, &payload)).await;
        assert_eq!(resp.status(), 200, "{id}");
    }
    ask(&fx).await;
    fx.eng.github_people("vault").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fx.forge.hits().len(),
        before,
        "{:?}",
        &fx.forge.hits()[before..]
    );
    assert!(fx.eng.github_access_closed().is_empty());
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault");
}

/// GitHub answers the recorded repository id, but under another owner id
/// (the repository was transferred to an account now called `acme-labs`):
/// that is no rename of the recorded organization, so nothing moves.
#[tokio::test]
async fn an_organization_rename_github_answers_under_another_owner_id_moves_nothing() {
    let fx = setup(Some(SECRET)).await;
    settle_both(&fx).await;
    let kb = fx.forge.id_of("acme/kb").unwrap();
    fx.forge.transfer("acme/kb", "acme-labs/kb");
    assert_ne!(fx.forge.owner_id_of("acme-labs"), owner_id("acme"));
    let resp = deliver(
        &fx,
        Delivery::signed(
            "organization",
            "d-org-other-owner",
            &org_renamed("acme", "acme-labs", owner_id("acme")),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(
        fx.forge
            .hits()
            .iter()
            .any(|h| h.path == format!("/repositories/{kb}")),
        "GitHub was asked"
    );
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme/vault");
}

/// Two domains on one repository: the one GitHub confirmed (recorded ids)
/// moves, the one with no recorded ids stays on the old name.
#[tokio::test]
async fn an_organization_rename_moves_only_the_confirmed_domain_of_a_repository() {
    let fx = setup(Some(SECRET)).await;
    connect(&fx.eng, "acme/kb", "kb2", &fx.tmp.path().join("kb2")).await;
    settle_both(&fx).await;
    fx.eng.github_people("kb2").await;
    let dir = fx.tmp.path().join("origins").join("kb2");
    let mut state = crystalline_remote::state::OriginState::load(&dir)
        .unwrap()
        .unwrap();
    state.repository_ids = None;
    state.save(&dir).unwrap();
    fx.forge.rename_owner("acme", "acme-labs");
    let resp = deliver(
        &fx,
        Delivery::signed(
            "organization",
            "d-org-two-domains",
            &org_renamed("acme", "acme-labs", owner_id("acme")),
        ),
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(origin_repo(&fx.eng, "kb"), "acme-labs/kb");
    assert_eq!(origin_repo(&fx.eng, "kb2"), "acme/kb");
    assert_eq!(origin_repo(&fx.eng, "vault"), "acme-labs/vault");
}
