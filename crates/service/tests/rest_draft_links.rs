//! The draft share-link surface, end to end over the production HTTP router:
//! minting a link on one's own draft, handing it to somebody, what they can do
//! with it, and what the rest of the instance still refuses to tell them.
//!
//! Served rather than driven through the engine, because who somebody is is
//! resolved at the door here: the session names the account, and the scope,
//! the actor, the drafts in range and the grants that widen them all follow
//! from that one resolution. A test that called the engine directly would be
//! asserting about a caller the door never produced.

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ReviewMode, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, Scope};
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# team\n\n## Scope\n\n- Everything the team knows\n\n## When to Use\n\n- Route here for team questions\n";

/// A whole engram document, so a save driven with it reaches the resolution
/// this test is about rather than stopping at the parse gate in front of it.
const CAROLS_EDIT: &str = "---\ntype: engram\ntitle: Fresh\npermalink: fresh\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Fresh\n\ncarol was here\n";

const PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Plan\n\nWhat the team agreed.\n";

/// A served instance with one file domain, `team`, in review mode.
struct Fixture {
    addr: std::net::SocketAddr,
    engine: Arc<Engine>,
    /// Held for the test's duration: every successful write marks its domain
    /// pending under the state directory, which this redirects into a scratch
    /// home. See `support::ScratchStateDir`.
    _state: support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

async fn serve() -> Fixture {
    let state = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    std::fs::write(dir.join("plan.md"), PLAN).unwrap();

    let mut entry = DomainEntry::file(dir);
    entry.review = Some(ReviewMode::Overlay);
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            mcp: Some(true),
            oauth: Some(false),
            ..AuthConfig::default()
        }),
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        }),
        ..GlobalConfig::default()
    };
    cfg.domains.insert("team".to_string(), entry);
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role) in [
        ("alice", Role::Editor),
        ("bob", Role::Editor),
        ("carol", Role::Editor),
        ("vera", Role::Viewer),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    let router = http_router(
        engine.clone(),
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
    Fixture {
        addr,
        engine,
        _state: state,
        _tmp: tmp,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

/// One signed-in browser: its cookie and its CSRF token.
struct Session {
    cookie: String,
    csrf: String,
}

async fn login(addr: std::net::SocketAddr, name: &str) -> Session {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": "pw12345678"}))
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
    Session {
        cookie,
        csrf: body["csrf"].as_str().unwrap().to_string(),
    }
}

impl Session {
    fn request(
        &self,
        addr: std::net::SocketAddr,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        client()
            .request(method, format!("http://{addr}{path}"))
            .header("cookie", format!("fluid_session={}", self.cookie))
            .header("x-csrf-token", &self.csrf)
    }
}

impl Fixture {
    /// One account's draft of a fresh page, written through the engine as the
    /// account the door would have resolved. The page exists in nobody else's
    /// view at all, which is the state every test here starts from.
    async fn draft(&self, account: &str, title: &str, body: &str) -> String {
        let params = crystalline_service::params::WriteParams {
            domain: "team".to_string(),
            title: title.to_string(),
            content: body.to_string(),
            folder: None,
            engram_type: None,
            tags: vec!["team".to_string()],
            status: None,
            metadata: None,
            overwrite: false,
        };
        let receipt = self
            .engine
            .write_engram_as(
                &params,
                None,
                &Scope::User {
                    account: account.to_string(),
                    admin: false,
                },
            )
            .await
            .expect("the draft lands in that account's overlay");
        assert_eq!(receipt["draft"], serde_json::json!(true), "{receipt}");
        receipt["path"].as_str().unwrap().to_string()
    }

    /// Mint a link on `owner`'s draft of `path`, as `owner`.
    async fn mint(&self, session: &Session, path: &str) -> serde_json::Value {
        let resp = session
            .request(
                self.addr,
                reqwest::Method::POST,
                "/api/v1/domains/team/draft-links",
            )
            .json(&serde_json::json!({"path": path}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "minting a link on one's own draft");
        resp.json().await.unwrap()
    }
}

/// The whole arc in one test: an author mints a link on their own draft, hands
/// it to somebody, and that person - and only that person - opens it.
///
/// The three properties that make a share-link safe to hand out at all are
/// here, in the order somebody meets them: the draft is invisible to the
/// stranger before the link, the first account to present the link binds it,
/// and a second account presenting the same link is told exactly what an
/// invented one is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_author_mints_and_a_stranger_binds() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let carol = login(f.addr, "carol").await;

    // Before the link, the page is not bob's to read at all.
    let unseen = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(unseen.status(), 404, "an unshared draft is nobody else's");

    let minted = f.mint(&alice, &path).await;
    let token = minted["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dl_"), "{minted}");

    let accepted: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["owner"], serde_json::json!("alice"), "{accepted}");
    assert_eq!(accepted["path"], serde_json::json!(path), "{accepted}");
    assert_eq!(accepted["editable"], serde_json::json!(true), "{accepted}");
    assert!(
        accepted["content"]
            .as_str()
            .unwrap()
            .contains("A page only alice has"),
        "the draft itself comes with the grant: {accepted}"
    );

    // Carol has the same link - forwarded, guessed, found - and it is dead.
    let forwarded = carol
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        forwarded.status(),
        404,
        "a link is for one account, and it was bob who opened it"
    );

    // And alice can see who holds it, and take it back.
    let listed: serde_json::Value = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            &format!("/api/v1/domains/team/draft-links?path={path}"),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed[0]["grantee"], serde_json::json!("bob"), "{listed}");
    assert!(
        listed[0].get("token").is_none(),
        "and the link itself is never readable again: {listed}"
    );

    let id = minted["id"].as_i64().unwrap();
    let revoked = alice
        .request(
            f.addr,
            reqwest::Method::DELETE,
            &format!("/api/v1/draft-links/{id}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 204);
    let after = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 404, "a revoked link opens nothing");
}

/// Sharing is the author's to do, and nobody else's.
///
/// The 404 is deliberately the same answer whether nobody is drafting at that
/// path or somebody else is: whose drafts exist is exactly what a domain in
/// review mode does not say, and a mint route that answered differently would
/// be a way to ask.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_author_mint_is_404() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let bob = login(f.addr, "bob").await;

    let refused = bob
        .request(
            f.addr,
            reqwest::Method::POST,
            "/api/v1/domains/team/draft-links",
        )
        .json(&serde_json::json!({"path": path}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 404, "bob is not drafting there");

    let nothing = bob
        .request(
            f.addr,
            reqwest::Method::POST,
            "/api/v1/domains/team/draft-links",
        )
        .json(&serde_json::json!({"path": "nobody-drafts-this.md"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        nothing.status(),
        404,
        "and a path nobody is drafting answers exactly the same"
    );
}

/// A viewer opens the draft and cannot type in it, and is told why by the
/// server rather than by a greyed-out button.
///
/// Reading somebody's wording is exactly what a viewer account is for, so the
/// link binds whatever the role; editing is the domain's ordinary write gate,
/// which a viewer does not pass. Both halves are here because the interesting
/// case is that they differ.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_role_grantee_opens_read_only_with_a_reason() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let alice = login(f.addr, "alice").await;
    let vera = login(f.addr, "vera").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let accepted: serde_json::Value = vera
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["editable"], serde_json::json!(false), "{accepted}");
    let reason = accepted["reason"].as_str().unwrap();
    assert!(
        reason.contains("viewer") && reason.contains("editor access"),
        "the reason names what she holds and what would change it: {reason}"
    );
    assert!(
        accepted["content"]
            .as_str()
            .unwrap()
            .contains("A page only alice has"),
        "and she has the draft to read: {accepted}"
    );

    let refused = vera
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/join")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        403,
        "and cannot join what she may not edit"
    );
}

/// The grant widens one path and nothing else.
///
/// The property the whole mode rests on, asserted from the one direction that
/// could quietly lose it: a grantee's search, listing and ordinary read are
/// exactly what they were before the link. Links are the only cross-overlay
/// visibility there is, and they are reached through the link surface alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grantees_search_still_excludes_the_owners_draft() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f
        .draft("alice", "Fresh", "A distinctive marmalade sentence.")
        .await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let accepted = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200, "bob holds the grant");

    let found: serde_json::Value = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/search?q=marmalade&mode=text",
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        found["hits"].as_array().map(|h| h.len()),
        Some(0),
        "a granted draft is not in the grantee's search: {found}"
    );

    let listed: serde_json::Value = bob
        .request(f.addr, reqwest::Method::GET, "/api/v1/domains/team/engrams")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = listed.to_string();
    assert!(
        !text.contains("fresh"),
        "nor in his listing of the domain: {text}"
    );

    let read = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        read.status(),
        404,
        "nor readable at its own address by the ordinary route"
    );

    // Alice's own view is untouched by any of it.
    let hers = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(hers.status(), 200, "while it is still her draft to read");
}

/// Seeing a draft and editing it are two states, and a write proves it.
///
/// Without a join the save is refused in words that name both ways forward;
/// with one it lands in the OWNER's draft and the receipt says so. The second
/// half is the only write in the system that crosses between two overlays, so
/// it is asserted on the rows rather than on the reply alone: alice's draft
/// carries bob's sentence, and bob is holding nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_at_a_granted_path_needs_a_join_and_then_lands_in_the_owners_draft() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let accepted: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let checksum = accepted["checksum"].as_str().unwrap().to_string();
    let edited = accepted["content"]
        .as_str()
        .unwrap()
        .replace("only alice has", "bob typed into");

    let refused = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("if-match", format!("\"{checksum}\""))
        .json(&serde_json::json!({"content": edited}))
        .send()
        .await
        .unwrap();
    let problem: serde_json::Value = refused.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("Join the draft") && detail.contains("your own overlay"),
        "the refusal names both ways forward: {problem}"
    );

    let joined: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/join")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = joined["join_key"].as_str().unwrap().to_string();

    let saved = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("if-match", format!("\"{checksum}\""))
        .header("x-crystalline-join", &key)
        .json(&serde_json::json!({"content": edited}))
        .send()
        .await
        .unwrap();
    let status = saved.status();
    assert_eq!(status, 200, "a joined save lands: {:?}", saved.text().await);
    let receipt: serde_json::Value = saved.json().await.unwrap();
    assert_eq!(
        receipt["joined"],
        serde_json::json!("landed in alice's draft"),
        "and says whose work it changed: {receipt}"
    );

    // The rows, which are the authority: her draft carries his sentence, and
    // he is holding nothing of his own.
    let hers: serde_json::Value = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        hers["content"].as_str().unwrap().contains("bob typed into"),
        "her draft is where it landed: {hers}"
    );
    let his = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        his.status(),
        404,
        "and he is still not holding a draft of his own"
    );

    // Leaving puts the write back where it was.
    let left = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/leave")
        .json(&serde_json::json!({"key": key}))
        .send()
        .await
        .unwrap();
    assert_eq!(left.status(), 204);
    let after = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("x-crystalline-join", &key)
        .header("if-match", "\"whatever\"")
        .json(&serde_json::json!({"content": edited}))
        .send()
        .await
        .unwrap();
    let problem: serde_json::Value = after.json().await.unwrap();
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Join the draft"),
        "a key that has been left is not a join: {problem}"
    );
}

/// A join key is a session's, so presenting somebody else's opens nothing.
///
/// The security property the whole arrangement rests on: if a leaked key
/// worked for whoever held it, a grant would be a way into an author's draft
/// for anybody the key reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_accounts_join_key_is_not_a_join() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let carol = login(f.addr, "carol").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let joined: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/join")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = joined["join_key"].as_str().unwrap().to_string();

    // Carol has bob's key and alice's draft is still not hers.
    let refused = carol
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("x-crystalline-join", &key)
        .header("if-match", "\"whatever\"")
        .json(&serde_json::json!({"content": CAROLS_EDIT}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        404,
        "the key is not hers, so the page is one she cannot see: {:?}",
        refused.text().await
    );
    let hers: serde_json::Value = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !hers["content"].as_str().unwrap().contains("carol was here"),
        "and her draft is as she left it: {hers}"
    );
}

/// Files follow the join: an upload made inside somebody's draft lands in
/// THEIR files overlay, to be folded or discarded with the draft that
/// references it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_upload_lands_in_the_owners_files() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let joined: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/join")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = joined["join_key"].as_str().unwrap().to_string();

    let uploaded = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/files/assets/sketch.png",
        )
        .header("x-crystalline-join", &key)
        .header("content-type", "image/png")
        .body(b"not really a png".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(uploaded.status(), 200, "{:?}", uploaded.text().await);

    // Alice holds it; bob holds nothing.
    let hers: serde_json::Value = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/attachments",
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        hers.to_string().contains("assets/sketch.png"),
        "the file is in the owner's overlay: {hers}"
    );
    let his: serde_json::Value = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/attachments",
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !his.to_string().contains("assets/sketch.png"),
        "and not in the uploader's: {his}"
    );
}

/// A grant lasts exactly as long as the thing it grants.
///
/// Folding the draft puts its text in the folder where everybody can read it
/// anyway, and discarding it leaves nothing to read; either way the link now
/// names a draft that is not there, and the session that was inside it is
/// inside nothing. Both end at the same moment and in the same call, which is
/// why one test covers them: leaving review mode ends every draft in the
/// domain at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folding_the_draft_ends_the_link_and_the_join() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let joined: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/join")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key = joined["join_key"].as_str().unwrap().to_string();

    // The machine owner takes the domain out of review mode, folding alice's
    // one draft into the folder the team shares.
    f.engine
        .set_review_mode(
            "team",
            None,
            crystalline_service::ReviewModeConfirm::Confirmed {
                folds: vec![("alice".to_string(), crystalline_service::FoldChoice::Fold)],
            },
            &Scope::Unrestricted,
        )
        .await
        .expect("leaving review mode folds the one draft there is");

    let dead = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(dead.status(), 404, "the link ended with the draft");

    // And the key is not a join any more: the page is in the folder now, so a
    // save of it is an ordinary save that lands where every other one does.
    let saved = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("x-crystalline-join", &key)
        .header("if-match", "\"whatever\"")
        .json(&serde_json::json!({"content": CAROLS_EDIT}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        saved.status(),
        412,
        "a stale token on an ordinary save, which is what this now is: {:?}",
        saved.text().await
    );
}
