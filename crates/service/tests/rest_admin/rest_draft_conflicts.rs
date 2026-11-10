//! The draft conflict routes, end to end over the production HTTP router: a
//! person who may write a reviewing domain lists, reads and settles her own
//! drafts' conflicts there, with no share role, and nobody else's.
//!
//! The team is a mock forge, so a peer's change arrives through an ordinary
//! pull. The instance shares as one instance credential
//! (`github.share_identity: instance`), where only an admin holds the share
//! role: that is the mode in which an editor has drafts and cannot reach the
//! `/sync` routes, and so the mode these routes exist for.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crate::support::{MockProvider, sha256_hex};
use crystalline_core::config::{
    AuthConfig, GitHubConfig, GlobalConfig, ResponseFormat, ReviewMode, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_remote::state::OriginState;
use crystalline_service::daemon::http_router;
use crystalline_service::params::WriteParams;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, ReviewModeConfirm, Scope};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const PASSWORD: &str = "pw12345678";

fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\nteam: github\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

fn plan() -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        "the first line of the plan\n\nthe middle of the plan\n\nthe last line of the plan",
    )
}

/// The plan after bob changed its first line.
fn plan_later() -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        "bob's first line\n\nthe middle of the plan\n\nthe last line of the plan",
    )
}

fn other() -> Vec<u8> {
    engram("Other", "other", "the team's other page")
}

/// The id a draft conflict of `actor` at `path` has.
fn id(actor: &str, path: &str) -> String {
    format!(
        "d{}",
        &sha256_hex(format!("{actor}\0{path}").as_bytes())[..7]
    )
}

fn scope(actor: &str) -> Scope {
    Scope::User {
        account: actor.to_string(),
        admin: false,
    }
}

/// A reviewing team domain `team` over a mock forge, and the engine that set
/// it up. Serve it with [`Team::serve`].
struct Team {
    tmp: tempfile::TempDir,
    mock: Arc<MockProvider>,
    engine: Arc<Engine>,
    /// Every successful write marks its domain pending under the state
    /// directory, which this redirects into a scratch home.
    _state: crate::support::ScratchStateDir,
}

/// A served engine and the accounts that sign in to it.
struct Served {
    addr: std::net::SocketAddr,
    auth: Arc<AuthStore>,
}

fn config_with(edit: impl FnOnce(&mut GlobalConfig), root: &std::path::Path) -> GlobalConfig {
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig::default()),
        github: Some(GitHubConfig {
            enabled: Some(true),
            // Said out loud, so the share role stays the admin's alone.
            share_identity: Some("instance".to_string()),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        }),
        ..GlobalConfig::default()
    };
    edit(&mut cfg);
    cfg
}

async fn team() -> Team {
    let state = crate::support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let files: BTreeMap<String, Vec<u8>> = [
        ("MANIFEST.md", manifest()),
        ("plan.md", plan()),
        ("other.md", other()),
    ]
    .into_iter()
    .map(|(path, bytes)| (path.to_string(), bytes))
    .collect();
    let commit = mock.add_commit(files);
    mock.set_branch("main", &commit);

    let cfg = config_with(|_| {}, tmp.path());
    let config_path = tmp.path().join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
        .with_origin_provider(mock.clone())
        .with_origins_dir(tmp.path().join("origins"))
        .with_token_store_dir(tmp.path().join("tokens"))
        .with_state_dir(tmp.path().join("state"));
    let root = tmp.path().join("team-knowledge");
    engine
        .origin_add(
            "acme/team",
            Some("team"),
            None,
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
    engine
        .set_review_mode(
            "team",
            Some(ReviewMode::Overlay),
            ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    Team {
        tmp,
        mock,
        engine: Arc::new(engine),
        _state: state,
    }
}

impl Team {
    fn root(&self) -> std::path::PathBuf {
        self.tmp.path().join("team-knowledge")
    }

    /// Another engine over this one's index and folders, with its config
    /// edited, read-only or not.
    fn engine_over(&self, read_only: bool, edit: impl FnOnce(&mut GlobalConfig)) -> Arc<Engine> {
        let config_path = self.tmp.path().join("config.yaml");
        let mut cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
        edit(&mut cfg);
        Arc::new(
            Engine::new(self.engine.store(), cfg, None, Some(config_path))
                .with_read_only(read_only)
                .with_origin_provider(self.mock.clone())
                .with_origins_dir(self.tmp.path().join("origins"))
                .with_token_store_dir(self.tmp.path().join("tokens"))
                .with_state_dir(self.tmp.path().join("state")),
        )
    }

    /// `actor` writes the page `title` whole, as a draft.
    async fn write(&self, actor: &str, title: &str, body: &str, admin: bool) {
        let params = WriteParams {
            path: None,
            base64: None,
            domain: "team".to_string(),
            title: title.to_string(),
            content: Some(body.to_string()),
            folder: None,
            engram_type: None,
            tags: vec!["test".to_string()],
            status: None,
            metadata: None,
            overwrite: true,
            share_link: None,
            model: None,
            expected_checksum: None,
        };
        let receipt = self
            .engine
            .write_engram_as(
                &params,
                None,
                &Scope::User {
                    account: actor.to_string(),
                    admin,
                },
            )
            .await
            .unwrap();
        assert_eq!(receipt["draft"], json!(true), "{receipt}");
    }

    /// The team commits these changes and this machine pulls them.
    async fn push(&self, changes: &[(&str, Vec<u8>)]) {
        let head = self.mock.branch_commit("main").unwrap();
        let mut files: BTreeMap<String, Vec<u8>> = self
            .mock
            .commit_paths(&head)
            .into_iter()
            .map(|path| {
                let bytes = self.mock.commit_file(&head, &path).unwrap();
                (path, bytes)
            })
            .collect();
        for (path, bytes) in changes {
            files.insert((*path).to_string(), bytes.clone());
        }
        let commit = self.mock.add_commit(files);
        self.mock.set_branch("main", &commit);
        self.engine
            .origin_update(Some("team"), &Scope::Unrestricted)
            .await
            .unwrap();
    }

    /// Alice drafts the plan whole and bob changes its first line: one
    /// conflict of hers, `EditEdit`.
    async fn alice_in_conflict(&self) -> String {
        self.write("alice", "Plan", "alice rewrote the plan", false)
            .await;
        self.push(&[("plan.md", plan_later())]).await;
        let found = self.engine.draft_conflicts("team", "alice").await.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, id("alice", "plan.md"));
        found[0].id.clone()
    }

    /// The exact paths `actor` holds a draft row at.
    async fn rows(&self, actor: &str) -> Vec<String> {
        let store = self.engine.store();
        let store = store.lock().await;
        let domain = store.domain_id("team").await.unwrap().unwrap();
        let mut rows: Vec<String> = store
            .overlay_entries(domain, actor)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.path)
            .collect();
        rows.sort();
        rows
    }

    async fn serve(&self) -> Served {
        serve(self.engine.clone(), self.tmp.path()).await
    }
}

async fn serve(engine: Arc<Engine>, dir: &std::path::Path) -> Served {
    // One accounts database per served engine, beside the team's folders.
    static SERVED: AtomicUsize = AtomicUsize::new(0);
    let auth_dir = dir.join(format!(
        "auth-{}",
        SERVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&auth_dir).unwrap();
    let auth = Arc::new(
        AuthStore::open(&auth_dir.join("web-auth.db"))
            .await
            .unwrap(),
    );
    for (name, role) in [
        ("alice", Role::Editor),
        ("bob", Role::Editor),
        ("vera", Role::Viewer),
        ("root", Role::Admin),
    ] {
        auth.add_user(name, name, None, role, PASSWORD)
            .await
            .unwrap();
    }
    let router = http_router(
        engine,
        Arc::new(AtomicUsize::new(0)),
        &[],
        auth.clone(),
        None,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Served { addr, auth }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// One signed-in browser: its cookie and its CSRF token.
struct Session {
    addr: std::net::SocketAddr,
    cookie: String,
    csrf: String,
}

impl Served {
    async fn login(&self, name: &str) -> Session {
        let resp = client()
            .post(format!("http://{}/api/v1/auth/login", self.addr))
            .json(&json!({"name": name, "password": PASSWORD}))
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
        Session {
            addr: self.addr,
            cookie,
            csrf: body["csrf"].as_str().unwrap().to_string(),
        }
    }
}

impl Session {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        client()
            .request(method, format!("http://{}/api/v1{path}", self.addr))
            .header("cookie", format!("fluid_session={}", self.cookie))
            .header("x-csrf-token", &self.csrf)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let resp = self
            .request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    async fn resolve(&self, id: &str, body: Value) -> (u16, Value) {
        let resp = self
            .request(
                reqwest::Method::POST,
                &format!("/domains/team/drafts/conflicts/{id}/resolve"),
            )
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }
}

const LIST: &str = "/domains/team/drafts/conflicts";

fn sides(id: &str) -> String {
    format!("/domains/team/drafts/conflicts/{id}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_editor_with_no_share_role_can_settle_her_own_conflict() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let served = t.serve().await;
    let alice = served.login("alice").await;

    let (status, list) = alice.get(LIST).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        list,
        json!({
            "conflicts": [{
                "id": hers,
                "path": "plan.md",
                "kind": "EditEdit",
                "draft": true,
                "moved_from": null,
                "file": false,
                "other": null,
            }],
            "merged": [],
            // Every choice is open to her here.
            "settle_note": null,
        })
    );

    let (status, both) = alice.get(&sides(&hers)).await;
    assert_eq!(status, 200, "{both}");
    assert_eq!(both["id"], json!(hers));
    assert_eq!(both["path"], "plan.md");
    assert_eq!(both["base"], json!(String::from_utf8(plan()).unwrap()));
    assert_eq!(
        both["upstream"],
        json!(String::from_utf8(plan_later()).unwrap())
    );
    assert!(
        both["local"]
            .as_str()
            .unwrap()
            .contains("alice rewrote the plan"),
        "{both}"
    );

    // The share surface stays closed to her, as before.
    let (status, _) = alice.get("/domains/team/sync").await;
    assert_eq!(status, 403, "an editor has no share role on this instance");
    let (status, _) = alice
        .get(&format!("/domains/team/sync/conflicts/{hers}"))
        .await;
    assert_eq!(status, 403);

    let (status, receipt) = alice.resolve(&hers, json!({"resolution": "mine"})).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["resolved"], "plan.md", "{receipt}");
    assert_eq!(receipt["draft"], json!(true), "{receipt}");
    assert_eq!(receipt["remaining"], json!(0), "{receipt}");
    assert!(receipt.get("kept_file").is_none(), "{receipt}");

    let (status, list) = alice.get(LIST).await;
    assert_eq!(status, 200);
    assert_eq!(
        list,
        json!({"conflicts": [], "merged": [], "settle_note": null})
    );
    assert_eq!(t.rows("alice").await, vec!["plan.md".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_accounts_conflict_is_not_found() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let served = t.serve().await;
    let alice = served.login("alice").await;
    let bob = served.login("bob").await;
    let root = served.login("root").await;

    let (status, _) = alice.get(&sides(&hers)).await;
    assert_eq!(status, 200, "her own id is found");

    let (status, list) = bob.get(LIST).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        list,
        json!({"conflicts": [], "merged": [], "settle_note": null})
    );

    // Not her id, not even for an admin: a draft is its author's alone.
    for who in [&bob, &root] {
        let (status, body) = who.get(&sides(&hers)).await;
        assert_eq!(status, 404, "{body}");
        assert!(!body.to_string().contains("alice"), "{body}");
        let (status, body) = who.resolve(&hers, json!({"resolution": "theirs"})).await;
        assert_eq!(status, 404, "{body}");
        assert!(!body.to_string().contains("alice"), "{body}");
    }
    // Her own resolve settles only the id she names.
    let (status, body) = alice
        .resolve("d0000000", json!({"resolution": "theirs"}))
        .await;
    assert_eq!(status, 404, "{body}");
    // An id nobody holds reads the same.
    let (status, unknown) = bob.get(&sides("d0000000")).await;
    assert_eq!(status, 404);
    assert_eq!(
        unknown["detail"],
        json!("no conflict d0000000 among your drafts in 'team'")
    );
    let (_, foreign) = bob.get(&sides(&hers)).await;
    assert_eq!(
        unknown["detail"]
            .as_str()
            .unwrap()
            .replace("d0000000", "ID"),
        foreign["detail"].as_str().unwrap().replace(&hers, "ID"),
    );

    assert_eq!(
        t.engine
            .draft_conflicts("team", "alice")
            .await
            .unwrap()
            .len(),
        1,
        "her conflict stands"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_cannot_resolve() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let served = t.serve().await;
    let vera = served.login("vera").await;

    let (status, list) = vera.get(LIST).await;
    assert_eq!(status, 200, "a reader lists her own, which is none: {list}");
    assert_eq!(
        list,
        json!({"conflicts": [], "merged": [], "settle_note": null})
    );

    for id in [hers.as_str(), "d0000000"] {
        let (status, body) = vera.resolve(id, json!({"resolution": "theirs"})).await;
        assert_eq!(status, 403, "{id}: {body}");
    }
    assert_eq!(
        t.engine
            .draft_conflicts("team", "alice")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_lists_and_refuses_the_resolve() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let read_only = t.engine_over(true, |_| {});
    let served = serve(read_only, t.tmp.path()).await;
    let alice = served.login("alice").await;

    let (status, list) = alice.get(LIST).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["conflicts"][0]["id"], json!(hers));
    // Said before she presses: nothing can be settled on this server.
    assert_eq!(
        list["settle_note"],
        "This server is read-only, so these drafts cannot be settled here."
    );
    let (status, both) = alice.get(&sides(&hers)).await;
    assert_eq!(status, 200, "{both}");

    let (status, body) = alice.resolve(&hers, json!({"resolution": "mine"})).await;
    assert_eq!(status, 403, "{body}");
    // The sentence every REST write is refused with on a read-only
    // instance, not only the engine's own backstop.
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("read-only; changes are disabled here"),
        "{body}"
    );
    assert_eq!(
        t.engine
            .draft_conflicts("team", "alice")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_needs_the_csrf_token() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let served = t.serve().await;
    let alice = served.login("alice").await;

    let resp = client()
        .post(format!(
            "http://{}/api/v1/domains/team/drafts/conflicts/{hers}/resolve",
            served.addr
        ))
        .header("cookie", format!("fluid_session={}", alice.cookie))
        .json(&json!({"resolution": "mine"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "no token, no write");
    assert_eq!(
        t.engine
            .draft_conflicts("team", "alice")
            .await
            .unwrap()
            .len(),
        1,
        "nothing was settled"
    );

    let (status, receipt) = alice.resolve(&hers, json!({"resolution": "mine"})).await;
    assert_eq!(status, 200, "with the token it lands: {receipt}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_reviewing_domain_lists_nothing() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    // Her rows stay, and the engine still reads them as a conflict: only the
    // review key is off.
    let off = t.engine_over(false, |cfg| {
        cfg.domains.get_mut("team").unwrap().review = None;
    });
    assert!(!off.drafts_stay_open("team"));
    assert_eq!(
        off.draft_conflicts("team", "alice").await.unwrap().len(),
        1,
        "the rows are there to be listed"
    );
    let served = serve(off, t.tmp.path()).await;
    let alice = served.login("alice").await;

    let (status, list) = alice.get(LIST).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        list,
        json!({"conflicts": [], "merged": [], "settle_note": null})
    );
    // In the words an id that is not hers gets, so the two read the same.
    let not_hers = json!(format!("no conflict {hers} among your drafts in 'team'"));
    let (status, body) = alice.get(&sides(&hers)).await;
    assert_eq!(status, 404);
    assert_eq!(body["detail"], not_hers, "{body}");
    let (status, body) = alice.resolve(&hers, json!({"resolution": "mine"})).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["detail"], not_hers, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_old_route_does_not_reach_a_draft_conflict() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    // The admin holds the share role and a draft of the other page, and the
    // folder holds an edit nobody drafted at that same path. The team then
    // changes the page, so the pull records the folder's own conflict there.
    t.write("root", "Other", "root's draft of the other page", true)
        .await;
    std::fs::write(
        t.root().join("other.md"),
        engram("Other", "other", "edited straight on disk"),
    )
    .unwrap();
    let upstream = engram("Other", "other", "changed upstream");
    t.push(&[("other.md", upstream.clone())]).await;
    let state = OriginState::load(&t.tmp.path().join("origins").join("team"))
        .unwrap()
        .unwrap();
    assert_eq!(state.conflicts.len(), 1, "the pull recorded the folder's");
    let folders = state.conflicts[0].id.clone();
    assert_eq!(state.conflicts[0].path, "other.md");
    assert_eq!(t.rows("root").await, vec!["other.md".to_string()]);

    let served = t.serve().await;
    let root = served.login("root").await;

    // A draft id is no folder conflict.
    let (status, body) = root
        .get(&format!("/domains/team/sync/conflicts/{hers}"))
        .await;
    assert_eq!(status, 404, "{body}");
    let resp = root
        .request(
            reqwest::Method::POST,
            &format!("/domains/team/sync/conflicts/{hers}/resolve"),
        )
        .json(&json!({"resolution": "theirs"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        t.engine
            .draft_conflicts("team", "alice")
            .await
            .unwrap()
            .len(),
        1
    );

    // The folder's id settles the folder, and the admin's draft at that path
    // is not touched.
    let resp = root
        .request(
            reqwest::Method::POST,
            &format!("/domains/team/sync/conflicts/{folders}/resolve"),
        )
        .json(&json!({"resolution": "theirs"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let receipt: Value = resp.json().await.unwrap();
    assert!(receipt["draft"].is_null(), "the folder's: {receipt}");
    assert_eq!(receipt["remaining"], json!(0), "{receipt}");
    assert_eq!(
        std::fs::read(t.root().join("other.md")).unwrap(),
        upstream,
        "the folder took the team's text"
    );
    assert_eq!(
        t.rows("root").await,
        vec!["other.md".to_string()],
        "the admin's draft stands"
    );

    // And the new route does not reach the folder's conflict.
    let (status, body) = root.get(&sides(&folders)).await;
    assert_eq!(status, 404, "{body}");
}

/// The resolve answers the engine's receipt as it is, and a merge lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_merged_resolve_lands_in_her_draft() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let served = t.serve().await;
    let alice = served.login("alice").await;

    let (status, body) = alice.resolve(&hers, json!({"resolution": "merged"})).await;
    assert_eq!(status, 422, "merged needs content: {body}");
    let (status, body) = alice
        .resolve(&hers, json!({"resolution": "sideways"}))
        .await;
    assert_eq!(status, 422, "{body}");

    let merged = String::from_utf8(engram(
        "Plan",
        "plan",
        "bob's first line\n\nalice rewrote the plan",
    ))
    .unwrap();
    let (status, receipt) = alice
        .resolve(&hers, json!({"resolution": "merged", "content": merged}))
        .await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["draft"], json!(true), "{receipt}");
    let read = t
        .engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: "plan".to_string(),
                domain: Some("team".to_string()),
                share_link: None,
                include_images: None,
            },
            &scope("alice"),
        )
        .await
        .unwrap();
    let text = read["content"].as_str().unwrap();
    assert!(
        text.contains("bob's first line") && text.contains("alice rewrote the plan"),
        "{text}"
    );
}

/// A resolve that names the team side it was shown is refused with 409
/// when a pull has brought newer team text since, and nothing changes; with
/// the side read again it settles.
#[tokio::test]
async fn a_resolve_against_team_text_she_was_not_shown_is_a_409() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let served = t.serve().await;
    let alice = served.login("alice").await;
    let (status, shown) = alice.get(&sides(&hers)).await;
    assert_eq!(status, 200, "{shown}");
    let t1 = shown["upstream_sha256"].as_str().unwrap().to_string();

    let newer = engram(
        "Plan",
        "plan",
        "bob's first line\n\nthe middle of the plan\n\nthe last line of the plan\n\n\
         a colleague's new paragraph",
    );
    t.push(&[("plan.md", newer)]).await;
    let (status, body) = alice
        .resolve(
            &hers,
            json!({"resolution": "mine", "expected_upstream_sha256": t1}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        body["detail"],
        "The team changed this page since you looked at it. Look at it again before you settle it."
    );
    let (_, list) = alice.get("/domains/team/drafts/conflicts").await;
    assert_eq!(list["conflicts"][0]["id"], json!(hers), "it stands: {list}");

    let (_, again) = alice.get(&sides(&hers)).await;
    let t2 = again["upstream_sha256"].as_str().unwrap().to_string();
    assert_ne!(t1, t2);
    let (status, receipt) = alice
        .resolve(
            &hers,
            json!({"resolution": "mine", "expected_upstream_sha256": t2}),
        )
        .await;
    assert_eq!(status, 200, "{receipt}");
}

/// An account that lost its write right still lists
/// its conflicts, and is told before it presses that none can be settled.
#[tokio::test]
async fn a_session_without_the_write_right_is_told_it_cannot_settle() {
    let t = team().await;
    t.alice_in_conflict().await;
    let served = t.serve().await;
    served.auth.set_role("alice", Role::Viewer).await.unwrap();
    let alice = served.login("alice").await;

    let (status, list) = alice.get(LIST).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["conflicts"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(
        list["settle_note"],
        "You cannot write in this domain, so these drafts cannot be settled here. Ask its \
         owner for write access."
    );
}

/// The sentence a list of her conflicts that cannot be read is answered with,
/// on status and on every draft conflict route alike.
const UNREADABLE: &str = "Your conflicts in this domain cannot be read, so none are listed, and a share of this domain refuses until they can. Try again, and if it stays, ask whoever runs this server.";

/// A team state file that cannot be parsed is refused in one plain sentence on
/// all three routes, to every reader, and names no place on the server's disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_state_file_names_no_server_path() {
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let state_dir = t.tmp.path().join("origins").join("team");
    std::fs::write(state_dir.join("state.json"), b"{ not json").unwrap();
    let served = t.serve().await;
    let alice = served.login("alice").await;
    let vera = served.login("vera").await;
    let secret = t.tmp.path().to_str().unwrap().to_string();

    let mut answers = vec![
        ("alice list", alice.get(LIST).await),
        ("alice sides", alice.get(&sides(&hers)).await),
        ("vera sides", vera.get(&sides("d0000000")).await),
        (
            "alice resolve",
            alice.resolve(&hers, json!({"resolution": "mine"})).await,
        ),
    ];
    for (what, (status, body)) in answers.drain(..) {
        assert_eq!(status, 422, "{what}: {body}");
        assert_eq!(body["detail"], json!(UNREADABLE), "{what}: {body}");
        assert!(!body.to_string().contains(&secret), "{what}: {body}");
        assert!(!body.to_string().contains("state.json"), "{what}: {body}");
    }
}

/// What a resolve whose write failed answers: nothing changed, and what to do.
const NOT_SETTLED: &str = "The conflict could not be settled, so nothing was changed. Try again, or ask whoever runs this server to look at its log.";

/// A resolve that read her conflicts and then could not write is told so in
/// its own words, as a server fault, and with no place on the server's disk.
/// It is not told that her conflicts cannot be read: they were.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_write_says_nothing_was_changed() {
    use std::os::unix::fs::PermissionsExt;
    let t = team().await;
    let hers = t.alice_in_conflict().await;
    let record = crystalline_service::draft_start::record_file(
        &t.tmp.path().join("state"),
        "team",
        "alice",
        "plan.md",
    )
    .unwrap();
    let folder = record.parent().unwrap().to_path_buf();
    assert!(record.exists(), "{}", record.display());
    let served = t.serve().await;
    let alice = served.login("alice").await;
    let secret = t.tmp.path().to_str().unwrap().to_string();

    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (status, body) = alice.resolve(&hers, json!({"resolution": "mine"})).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(status, 500, "{body}");
    assert_eq!(body["detail"], json!(NOT_SETTLED), "{body}");
    assert!(!body.to_string().contains(&secret), "{body}");
    // Her conflicts still read, and the one she pressed stands.
    let (status, list) = alice.get(LIST).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["conflicts"][0]["id"], json!(hers));
}

// --- The folder's own conflict at a file beside the pages --------------------

const DECK: &str = "assets/deck.png";
const FOLDER_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00the deck edited in the folder";
const NEWER_DECK: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's newer deck";

/// The team holds a deck, the folder's copy is edited out of band, and the
/// team then changes it: the pull records the folder's conflict at the file,
/// and its id is returned.
async fn a_folder_file_conflict(t: &Team) -> String {
    t.push(&[(DECK, b"\x89PNG\r\n\x1a\n\x00the team's deck".to_vec())])
        .await;
    std::fs::write(t.root().join(DECK), FOLDER_DECK).unwrap();
    t.push(&[(DECK, NEWER_DECK.to_vec())]).await;
    let state = OriginState::load(&t.tmp.path().join("origins").join("team"))
        .unwrap()
        .unwrap();
    assert_eq!(state.conflicts.len(), 1, "the pull recorded the folder's");
    assert_eq!(state.conflicts[0].path, DECK);
    state.conflicts[0].id.clone()
}

/// The folder's conflict at a file settles by its id, each way, and the
/// domain shares again afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_folder_route_settles_a_file_each_way_and_the_domain_shares_again() {
    for (keep, on_disk, kept_copy) in [("mine", FOLDER_DECK, false), ("theirs", NEWER_DECK, true)] {
        let t = team().await;
        t.write("root", "Other", "root's draft of the other page", true)
            .await;
        let folders = a_folder_file_conflict(&t).await;
        let pending = t
            .engine
            .origin_share(
                "team",
                None,
                None,
                None,
                None,
                crystalline_service::engine::ShareActor::Account("root".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(pending["outcome"], "conflicts_pending", "{keep}: {pending}");

        let served = t.serve().await;
        let root = served.login("root").await;
        let resp = root
            .request(
                reqwest::Method::POST,
                &format!("/domains/team/sync/conflicts/{folders}/resolve"),
            )
            .json(&json!({"resolution": keep}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{keep}");
        let receipt: Value = resp.json().await.unwrap();
        assert_eq!(receipt["resolved"], DECK, "{keep}: {receipt}");
        assert_eq!(receipt["remaining"], json!(0), "{keep}: {receipt}");
        assert_eq!(receipt["file"], json!(true), "{keep}: {receipt}");
        assert_eq!(receipt["kept"], keep, "{keep}: {receipt}");
        assert_eq!(receipt["kept_copy"], json!(kept_copy), "{keep}: {receipt}");
        // An account is never told a place on this server's disk.
        assert!(receipt["kept_file"].is_null(), "{keep}: {receipt}");
        assert_eq!(
            std::fs::read(t.root().join(DECK)).unwrap(),
            on_disk,
            "{keep}"
        );

        let shared = t
            .engine
            .origin_share(
                "team",
                None,
                None,
                None,
                None,
                crystalline_service::engine::ShareActor::Account("root".to_string()),
            )
            .await
            .unwrap();
        assert_eq!(shared["outcome"], "proposed", "{keep}: {shared}");
    }
}

/// A file has no text to merge, and the route says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_folder_route_refuses_to_merge_a_file() {
    let t = team().await;
    let folders = a_folder_file_conflict(&t).await;
    let served = t.serve().await;
    let root = served.login("root").await;
    let resp = root
        .request(
            reqwest::Method::POST,
            &format!("/domains/team/sync/conflicts/{folders}/resolve"),
        )
        .json(&json!({"resolution": "merged", "content": "some text"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["detail"], "A file cannot be merged here. Keep one of the two.",
        "{body}"
    );
    assert_eq!(std::fs::read(t.root().join(DECK)).unwrap(), FOLDER_DECK);
}
