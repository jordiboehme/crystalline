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
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
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
    /// The accounts database the router reads, so a test can change what an
    /// account may see without going through an admin screen this fixture has
    /// no admin for.
    auth: Arc<AuthStore>,
    /// The instance root, so a test can look at the overlay tree itself: some
    /// of what these routes must NOT do is only visible on disk, as a
    /// deletion marker that was never written.
    root: std::path::PathBuf,
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
    // Two files the team reviewed, so a test can ask what a join may do to
    // somebody else's attachments as well as to its own.
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/shared.png"), b"the team's picture").unwrap();
    std::fs::write(dir.join("assets/other.png"), b"another of the team's").unwrap();

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
        auth,
        root,
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

    /// One account's own read of an engram, as the door resolved them.
    async fn reads(&self, session: &Session, permalink: &str) -> serde_json::Value {
        let resp = session
            .request(
                self.addr,
                reqwest::Method::GET,
                &format!("/api/v1/domains/team/engrams/{permalink}"),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "reading '{permalink}'");
        resp.json().await.unwrap()
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

    // The granted path itself IS readable, and only it: that is the one
    // widening a link makes, and it is asserted in full by
    // `a_granted_read_answers_the_draft_and_names_whose`. What matters here is
    // that it is the exception rather than the rule - the search and the
    // listing above went on saying nothing.
    let read: serde_json::Value = bob
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
    assert_eq!(
        read["draft_owner"],
        serde_json::json!("alice"),
        "the granted path answers her draft, named as hers: {read}"
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
    let his: serde_json::Value = bob
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
    assert_eq!(
        his["draft_owner"],
        serde_json::json!("alice"),
        "and what he reads at that path is still HER draft - he is holding \
         none of his own, which is the whole point of a joined write: {his}"
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

/// A grant is visibility, and visibility reaches the account's agent too.
///
/// The user sees the granted draft on the screen the link lands on; the agent
/// - which authenticates as the same account and has no screen at all - sees
/// it by reading the path the link was for. Anything else would mean a person
/// could be handed a colleague's draft and their own agent could not be shown
/// what they were looking at.
///
/// The widening is exactly one path and nothing else: the reads around it
/// answer what they always answered, which is what the search test beside this
/// one asserts from the other side. Two things the read says out loud, because
/// a granted draft must never be mistaken for the page the team holds: it is
/// marked as a draft, and it names whose.
///
/// And it does not carry the rest of the owner's overlay with it. A link in
/// the granted draft that points at another of the owner's drafts resolves to
/// nothing for the grantee, because that draft was not shared: the grant
/// widens one path, not a neighbourhood.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_granted_read_answers_the_draft_and_names_whose() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    // Two drafts of alice's, and only one of them is shared. The shared one
    // points at the other, which is what makes the no-transitivity assertion
    // possible at all.
    f.draft("alice", "Secret", "A second page only alice has.")
        .await;
    let path = f
        .draft(
            "alice",
            "Fresh",
            "A page only alice has.\n\n- relates_to [[Secret]]",
        )
        .await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let carol = login(f.addr, "carol").await;

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

    let read: serde_json::Value = bob
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
        read["content"]
            .as_str()
            .unwrap()
            .contains("A page only alice has"),
        "the read at the granted path answers the draft: {read}"
    );
    assert_eq!(read["draft"], serde_json::json!(true), "{read}");
    assert_eq!(
        read["draft_owner"],
        serde_json::json!("alice"),
        "and says whose, so it is never mistaken for the team's page: {read}"
    );
    assert_eq!(
        read["relations"][0]["resolved"],
        serde_json::json!(false),
        "the link onto her OTHER draft resolves to nothing: that one was not \
         shared, and a grant widens one path rather than a neighbourhood: {read}"
    );

    let unshared = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/secret",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        unshared.status(),
        404,
        "and her other draft is still nobody else's"
    );

    let stranger = carol
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        stranger.status(),
        404,
        "while somebody holding no link sees exactly what they saw before"
    );
}

/// A grant whose draft has gone does not lock its grantee out of the path.
///
/// The refusal that teaches "join, or draft your own" is only true while there
/// IS a draft to join. Once its author has taken theirs away, a grantee still
/// being told to join it could neither join - the link answers that the draft
/// is gone - nor write, and a dead row would have taken a path away from
/// somebody it was never about.
///
/// Driven over the base page rather than over a draft-only one, because that
/// is the shape where it bites: a draft-only path stops resolving for the
/// grantee at all once its author drops it, so the refusal is never reached.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_whose_draft_is_gone_stops_refusing_the_grantees_own_write() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;

    // Alice redrafts the page the team holds, and shares that draft.
    let base = f.reads(&alice, "plan").await;
    let redrafted = base["content"]
        .as_str()
        .unwrap()
        .replace("What the team agreed", "What alice would rather");
    let saved = alice
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/plan",
        )
        .header(
            "if-match",
            format!("\"{}\"", base["checksum"].as_str().unwrap()),
        )
        .json(&serde_json::json!({"content": redrafted}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200, "{:?}", saved.text().await);
    let token = f.mint(&alice, "plan.md").await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let accepted = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200);

    // Then she takes her draft away - her deletion of the page stands where
    // her redraft did - while the domain goes on reviewing changes, so nothing
    // has ended the grant row.
    let hers = f.reads(&alice, "plan").await;
    let deleted = alice
        .request(
            f.addr,
            reqwest::Method::DELETE,
            "/api/v1/domains/team/engrams/plan",
        )
        .header(
            "if-match",
            format!("\"{}\"", hers["checksum"].as_str().unwrap()),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204, "{:?}", deleted.text().await);

    // Bob, who was never asked about any of that, redrafts the team's page in
    // his own overlay exactly as anybody else may.
    let his = f.reads(&bob, "plan").await;
    let written = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/plan",
        )
        .header(
            "if-match",
            format!("\"{}\"", his["checksum"].as_str().unwrap()),
        )
        .json(&serde_json::json!({
            "content": his["content"].as_str().unwrap().replace("agreed", "is weighing"),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        written.status(),
        200,
        "a link to a draft that is gone refuses nothing: {:?}",
        written.text().await
    );
}

/// A link is the author's word about one draft and never about a domain, so it
/// must not outlive the grantee's access to the domain that draft is in.
///
/// The case that would otherwise be a hole: a grant redeemed while the domain
/// was open to everybody, and then the domain is made private with the grantee
/// no member of it. Every other read they make answers as though the domain
/// were not there, and this one has to as well - a widening that survived the
/// authorization behind it would be one page of a private domain still open to
/// somebody who was removed from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_does_not_outlive_the_grantees_access_to_the_domain() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
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
    let read = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 200, "and reads the draft while he may");

    // The domain becomes alice's private one, and bob is nobody in it.
    f.auth
        .set_domain_visibility("team", true, "alice")
        .await
        .unwrap();

    let after = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        after.status(),
        404,
        "the grant widens nothing in a domain he can no longer see"
    );
    let reopened = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        reopened.status(),
        404,
        "and the link itself opens nothing either"
    );
}

/// The draft alice shares in the attachment tests: it references exactly one of
/// the team's two files, which is what makes "referenced" and "not referenced"
/// two different questions about one join.
const ILLUSTRATED: &str = "A page only alice has.\n\n![Shot](assets/shared.png)\n";

impl Fixture {
    /// Alice drafts an illustrated page, shares it, and bob joins: the state
    /// every attachment test below starts from. Answers bob's join key.
    async fn joined_to_an_illustrated_draft(&self, alice: &Session, bob: &Session) -> String {
        let path = self.draft("alice", "Fresh", ILLUSTRATED).await;
        let token = self.mint(alice, &path).await["token"]
            .as_str()
            .unwrap()
            .to_string();
        let joined: serde_json::Value = bob
            .request(self.addr, reqwest::Method::POST, "/api/v1/draft-links/join")
            .json(&serde_json::json!({"token": token}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        joined["join_key"].as_str().unwrap().to_string()
    }

    /// Whether alice's files overlay holds a deletion marker at `path`.
    fn owner_tombstoned(&self, path: &str) -> bool {
        self.root
            .join("state/overlays/team/alice/files")
            .join(format!("{path}.tombstone"))
            .exists()
    }
}

/// A join carries the page and the files that page points at, and stops there.
///
/// Adding an illustration to the draft you were invited into is the whole
/// reason a join reaches the files overlay at all, so a file that stands
/// nowhere yet is a file the join may make.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_upload_of_a_new_file_lands_in_the_owners_overlay() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let key = f.joined_to_an_illustrated_draft(&alice, &bob).await;

    let uploaded = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/files/assets/sketch.png",
        )
        .header("x-crystalline-join", &key)
        .header("content-type", "image/png")
        .body(b"bob's addition".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(
        uploaded.status(),
        200,
        "a path nothing stands at is the join's to make: {:?}",
        uploaded.text().await
    );
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
        "and it lands in the owner's overlay, to be folded with her draft: {hers}"
    );
}

/// Replacing the picture the shared draft actually shows is the other half of
/// what a join is for: the page and its own illustrations are one piece of
/// work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_overwrite_of_a_referenced_attachment_lands() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let key = f.joined_to_an_illustrated_draft(&alice, &bob).await;

    let replaced = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/files/assets/shared.png",
        )
        .header("x-crystalline-join", &key)
        .header("content-type", "image/png")
        .body(b"a clearer version".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(
        replaced.status(),
        200,
        "the draft points at this file, so it is part of what was shared: {:?}",
        replaced.text().await
    );
    let bytes = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/files/assets/shared.png",
        )
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(
        bytes.as_ref(),
        b"a clearer version",
        "and alice's own view of it is the replacement, held as her draft"
    );
}

/// A join is not the run of somebody else's overlay.
///
/// The file the shared draft does not point at is another piece of work
/// entirely - it may be staged for a different draft of alice's, or it may be
/// the team's - and a join into one page must not be a way to overwrite it
/// under her name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_overwrite_of_an_unreferenced_path_refuses() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let key = f.joined_to_an_illustrated_draft(&alice, &bob).await;

    let refused = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/files/assets/other.png",
        )
        .header("x-crystalline-join", &key)
        .header("content-type", "image/png")
        .body(b"bob rewrites the team's file".to_vec())
        .send()
        .await
        .unwrap();
    let problem: serde_json::Value = refused.json().await.unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("assets/other.png") && detail.contains("ask its author"),
        "the refusal names the file and the two ways forward: {problem}"
    );
    let bytes = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/files/assets/other.png",
        )
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(
        bytes.as_ref(),
        b"another of the team's",
        "and the file is as the team left it"
    );
}

/// And a join is certainly not a way to delete the team's files under somebody
/// else's name.
///
/// The marker is what would be folded, so the assertion is about the tree
/// rather than about the status: a deletion that was refused and staged anyway
/// would be a deletion nobody refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_delete_of_an_unreferenced_file_refuses_and_leaves_no_tombstone() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let key = f.joined_to_an_illustrated_draft(&alice, &bob).await;

    let refused = bob
        .request(
            f.addr,
            reqwest::Method::DELETE,
            "/api/v1/domains/team/files/assets/other.png",
        )
        .header("x-crystalline-join", &key)
        .send()
        .await
        .unwrap();
    let problem: serde_json::Value = refused.json().await.unwrap();
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("assets/other.png"),
        "refused, in words naming the file: {problem}"
    );
    assert!(
        !f.owner_tombstoned("assets/other.png"),
        "and nothing was staged under her name for the fold to carry out"
    );
    let bytes = alice
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/files/assets/other.png",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(bytes.status(), 200, "the file is still there for her");
}

/// A grant ends when the draft ends, and "ends" has to mean ended rather than
/// dormant.
///
/// The freshness check makes a link to a discarded draft look dead, which is
/// the right answer while nothing stands at that path. But a link that was only
/// dormant springs back onto whatever its author drafts there NEXT - a
/// different text, written after they took the first one back - and neither
/// side would be told. So a discard ends the row, and a redraft at the same
/// path revives nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_discarded_draft_ends_its_grant_and_a_redraft_revives_nothing() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;

    let path = f
        .draft("alice", "Fresh", "The first thing alice wrote.")
        .await;
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
    assert_eq!(accepted.status(), 200, "bob holds the link");

    // Alice takes her draft back. It was hers alone, so nothing of it remains.
    let hers = f.reads(&alice, "fresh").await;
    let discarded = alice
        .request(
            f.addr,
            reqwest::Method::DELETE,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header(
            "if-match",
            format!("\"{}\"", hers["checksum"].as_str().unwrap()),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(discarded.status(), 204, "{:?}", discarded.text().await);

    // And then writes a different page at the same path.
    f.draft("alice", "Fresh", "A second thing, written later.")
        .await;

    let dead = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        dead.status(),
        404,
        "the link ended with the draft it was for: {:?}",
        dead.text().await
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
        "and her second page is hers alone, exactly as her first was"
    );
}

/// Removing a domain ends every link in it, for the reason the fold does: the
/// drafts it was reviewing are over, whichever way they ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_a_domain_ends_the_links_in_it() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
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
    assert_eq!(accepted.status(), 200);

    // The machine owner unregisters the domain, ending every draft in it.
    f.engine
        .unregister_domain("team", &Scope::Unrestricted, false, &["alice".to_string()])
        .await
        .expect("the domain goes, and alice's one draft with it");

    assert!(
        f.auth
            .overlay_grants_held("bob", "team")
            .await
            .unwrap()
            .is_empty(),
        "and bob holds no live link into a domain that is not there"
    );
}

/// A grant is not a reason to hide somebody's own unfolded work from them.
///
/// Bob is redrafting the team's page and holds a link to alice's redraft of the
/// same one. The precedence is the mode's own: his own draft first, then what a
/// grant widens, then the page the team holds - so every ordinary read answers
/// HIS text, and alice's is where it was put, on the screen the link lands on.
/// A write while joined still lands in her overlay, because that is what the
/// join decided and the read order says nothing about it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grantees_own_draft_at_the_granted_path_wins_for_reads() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;

    // Both redraft the team's page, each in their own overlay.
    let base = f.reads(&alice, "plan").await;
    let checksum = base["checksum"].as_str().unwrap().to_string();
    for (who, phrase) in [
        (&alice, "what alice would rather"),
        (&bob, "bobs own marmalade"),
    ] {
        let redrafted = base["content"]
            .as_str()
            .unwrap()
            .replace("What the team agreed", phrase);
        let saved = who
            .request(
                f.addr,
                reqwest::Method::PUT,
                "/api/v1/domains/team/engrams/plan",
            )
            .header("if-match", format!("\"{checksum}\""))
            .json(&serde_json::json!({"content": redrafted}))
            .send()
            .await
            .unwrap();
        assert_eq!(saved.status(), 200, "{:?}", saved.text().await);
    }

    let token = f.mint(&alice, "plan.md").await["token"]
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
    assert!(
        accepted["content"]
            .as_str()
            .unwrap()
            .contains("what alice would rather"),
        "the link hands over HER draft, which is what it is for: {accepted}"
    );

    // And every ordinary read of his answers his own work.
    let his = f.reads(&bob, "plan").await;
    assert!(
        his["content"]
            .as_str()
            .unwrap()
            .contains("bobs own marmalade"),
        "his own draft is what he reads at that path: {his}"
    );
    assert!(
        his.get("draft_owner").is_none(),
        "and it is not marked as anybody else's: {his}"
    );

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
        found["hits"].as_array().map(|hits| hits.len()),
        Some(1),
        "and his search finds it, as it did before any link existed: {found}"
    );
}

/// Somebody who may not read the domain cannot burn the link on their way to
/// being refused.
///
/// Redeeming binds a link to its first presenter for good. If the domain screen
/// ran after that bind, a stranger who found the link - and who is told nothing,
/// since the refusal is the same 404 an invented token gets - would have spent
/// it, and the person it was meant for could never open it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_of_no_such_domain_does_not_burn_the_link() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;
    let carol = login(f.addr, "carol").await;
    let path = f.draft("alice", "Fresh", "A page only alice has.").await;
    let token = f.mint(&alice, &path).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    // The domain becomes alice's private one. Carol, who is nobody in it,
    // finds the link and presents it.
    f.auth
        .set_domain_visibility("team", true, "alice")
        .await
        .unwrap();
    let refused = carol
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 404, "she is told what a stranger is told");

    // Bob is made a member, and the link is still his to open.
    f.auth
        .upsert_domain_member("team", "bob", MemberLevel::Editor, "alice")
        .await
        .unwrap();
    let accepted: serde_json::Value = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        accepted["owner"],
        serde_json::json!("alice"),
        "nobody spent it on the way past: {accepted}"
    );
}

/// Renaming a draft ends the links on it, exactly as discarding one does.
///
/// A rename is the author deciding the page belongs somewhere else. The link
/// was minted on a path, so after the move it names somewhere its author is no
/// longer working - and a row left live would spring back onto whatever they
/// drafted at the old path next, which is the same revival a discard used to
/// allow. The author re-shares the page under its new path; nothing follows the
/// rename by itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renamed_draft_ends_its_grant_and_a_redraft_at_the_old_path_revives_nothing() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;

    let path = f
        .draft("alice", "Fresh", "The first thing alice wrote.")
        .await;
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
    assert_eq!(accepted.status(), 200, "bob holds the link");

    // Alice decides the page belongs somewhere else.
    let moved = alice
        .request(f.addr, reqwest::Method::POST, "/api/v1/domains/team/move")
        .json(&serde_json::json!({"permalink": "fresh", "destination": "notes/fresh"}))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), 200, "{:?}", moved.text().await);

    let dead = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        dead.status(),
        404,
        "the link ended with the draft leaving that path: {:?}",
        dead.text().await
    );
    // The page kept its own address through the move - a document travels
    // verbatim - so this is the same engram, now standing somewhere the link
    // was never minted on, and nothing followed it there.
    let elsewhere = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        elsewhere.status(),
        404,
        "where alice put the page is hers alone until she shares it again"
    );

    // And a later draft at the OLD path revives nothing.
    //
    // The moved draft is taken away first, and only because it still answers
    // to the address the second page will want - one engram answers to one
    // address, wherever it stands. That discard is at the NEW path and ends
    // nothing at the old one, so what the old token meets below is whatever
    // the rename left behind it.
    let moved_read = f.reads(&alice, "fresh").await;
    assert_eq!(
        moved_read["path"],
        serde_json::json!("notes/fresh.md"),
        "and it is the moved one she is reading: {moved_read}"
    );
    let discarded = alice
        .request(
            f.addr,
            reqwest::Method::DELETE,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header(
            "if-match",
            format!("\"{}\"", moved_read["checksum"].as_str().unwrap()),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(discarded.status(), 204, "{:?}", discarded.text().await);
    f.draft("alice", "Fresh", "A second thing, written later.")
        .await;
    let revived = bob
        .request(f.addr, reqwest::Method::POST, "/api/v1/draft-links/accept")
        .json(&serde_json::json!({"token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(revived.status(), 404, "{:?}", revived.text().await);
    let read = bob
        .request(
            f.addr,
            reqwest::Method::GET,
            "/api/v1/domains/team/engrams/fresh",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), 404, "her second page is hers alone too");
}

/// And it ends the sessions that were inside it.
///
/// A join is to one draft at one path. Once its author has moved that draft
/// away, a session still holding the join would be writing into an overlay
/// entry that is not there - so the join ends with the grant, and the writing
/// goes back to being the writer's own, which is what the refusal always
/// offered as the second way forward.
///
/// Two things are asserted and they are the two halves of "gone": the registry
/// holds nothing, which is what the bar at the top of the screen reads on its
/// next look, and a write still presenting the key reaches nothing of alice's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_ends_a_live_join_at_the_old_path() {
    let _serialized = support::maintenance_guard().await;
    let f = serve().await;
    let alice = login(f.addr, "alice").await;
    let bob = login(f.addr, "bob").await;

    let path = f
        .draft("alice", "Fresh", "The thing alice is drafting.")
        .await;
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
    let checksum = joined["checksum"].as_str().unwrap().to_string();
    assert!(
        f.engine.joins().get(&key, "bob").is_some(),
        "the join is open before the rename"
    );

    // Alice moves the draft bob is working in.
    let moved = alice
        .request(f.addr, reqwest::Method::POST, "/api/v1/domains/team/move")
        .json(&serde_json::json!({"permalink": "fresh", "destination": "notes/fresh"}))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), 200, "{:?}", moved.text().await);

    // The registry is what the bar reads on its next look, and it holds
    // nothing: the join ended with the draft it was a join to.
    assert!(
        f.engine.joins().get(&key, "bob").is_none(),
        "the join ended with the draft"
    );

    // Bob writes at the old path, still presenting the key he was given. It
    // reaches nothing of hers: there is no join for it to route through any
    // more, and no draft of his own at that path either.
    let written = bob
        .request(
            f.addr,
            reqwest::Method::PUT,
            "/api/v1/domains/team/engrams/fresh",
        )
        .header("if-match", format!("\"{checksum}\""))
        .header("x-crystalline-join", &key)
        .json(&serde_json::json!({"content": "bob types into a draft he has left"}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        written.status(),
        200,
        "a key that has been ended routes nothing: {:?}",
        written.text().await
    );
    let hers = f.reads(&alice, "fresh").await;
    assert_eq!(
        hers["path"],
        serde_json::json!("notes/fresh.md"),
        "her draft is where she moved it: {hers}"
    );
    assert!(
        hers["content"]
            .as_str()
            .unwrap()
            .contains("The thing alice is drafting"),
        "and it says what she left it saying: {hers}"
    );
}
