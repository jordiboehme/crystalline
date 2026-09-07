//! What a private domain looks like from the outside, over the REST API.
//!
//! One property, asserted from as many angles as the surface offers: a domain
//! a caller may not see is answered exactly as a domain nobody registered.
//! Not "forbidden", not an empty result of a different shape - the same status
//! and the same words, because the existence of a private domain is the secret
//! it keeps. Beside it, the two refusals that are NOT hidden: a member whose
//! level is too low is told their level, and only an admin may change whether
//! a domain is private at all.
//!
//! Driven through the production router (`daemon::http_router`) over a live
//! loopback listener, like `rest_mcp_tokens.rs`, so the guard, the mount point
//! and the engine's private-domain resolver are all the real ones.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, OriginConfig, ResponseFormat,
    ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_remote::state::OriginState;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;

use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use serde_json::json;
use tokio::sync::Mutex;

/// One engram's markdown, with `body` as everything below the heading.
fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\n\
         status: stable\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// A domain's MANIFEST, which every domain needs at its root.
fn manifest(name: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\n\
         status: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n\
         - Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
    )
}

/// A client for one identity: the served address plus the session cookie and
/// CSRF token it sends.
#[derive(Clone)]
struct SessionClient {
    addr: std::net::SocketAddr,
    session: Option<(String, String)>,
}

impl SessionClient {
    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = Self::client().request(method, format!("http://{}{path}", self.addr));
        if let Some((cookie, csrf)) = &self.session {
            req = req
                .header("cookie", format!("fluid_session={cookie}"))
                .header("x-csrf-token", csrf);
        }
        req
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::GET, path)
            .send()
            .await
            .unwrap()
    }

    /// The body of a GET, as text, with the status asserted first so a failure
    /// reports what came back rather than a parse error.
    async fn get_text(&self, path: &str, expect: u16) -> String {
        let resp = self.get(path).await;
        let status = resp.status();
        let body = resp.text().await.unwrap();
        assert_eq!(status, expect, "GET {path}: {body}");
        body
    }

    async fn get_json(&self, path: &str) -> serde_json::Value {
        serde_json::from_str(&self.get_text(path, 200).await).unwrap()
    }

    async fn post_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.request(reqwest::Method::POST, path)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn put_json(&self, path: &str, body: serde_json::Value) -> reqwest::Response {
        self.request(reqwest::Method::PUT, path)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn delete(&self, path: &str) -> reqwest::Response {
        self.request(reqwest::Method::DELETE, path)
            .send()
            .await
            .unwrap()
    }
}

/// Two file domains - `open`, shared, and `lab`, which the tests make private -
/// and the four accounts the private-domain policy has words for.
struct RestCtx {
    addr: std::net::SocketAddr,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

impl RestCtx {
    /// `open` holds `alpha`; `lab` holds `secret`, which cites `open:alpha`
    /// (so the inbound list has something to hide) and links at a target
    /// nobody wrote (so the consolidation sweep has a finding to hide).
    ///
    /// The cast: `owner` (instance editor, the account the domain is made
    /// private under), `mem` (instance EDITOR, so a membership refusal cannot
    /// be mistaken for an instance-role one), `out` (instance editor, no
    /// membership), `mgr` (instance editor, manager membership) and `boss`
    /// (instance admin).
    async fn two_domains() -> RestCtx {
        RestCtx::build(false, false).await
    }

    /// The same domains on an instance serving the anonymous viewer tier, so
    /// the one identity that carries no account at all can be put to the same
    /// questions the accounts are.
    async fn anonymous_instance() -> RestCtx {
        RestCtx::build(false, true).await
    }

    /// The same two domains, both carrying a GitHub origin, on an instance
    /// with GitHub on and `share_identity = personal` - which is what puts an
    /// instance EDITOR through the share gate, so the sync summary's own
    /// filtering is what the assertions see rather than the admin gate above
    /// it. Nothing here connects: the status read reports local state and says
    /// the connection is absent.
    async fn two_team_domains() -> RestCtx {
        RestCtx::build(true, false).await
    }

    async fn build(team: bool, anonymous: bool) -> RestCtx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig {
            auth: Some(AuthConfig {
                trusted_header: None,
                anonymous: Some(anonymous),
                mcp: None,
                max_users: None,
                oidc: None,
            }),
            github: team.then(|| GitHubConfig {
                enabled: Some(true),
                share_identity: Some("personal".to_string()),
                ..GitHubConfig::default()
            }),
            ..GlobalConfig::default()
        };
        let mut layout = vec![
            (
                "open",
                vec![("Alpha", "alpha", "A rule about alpha.".to_string())],
            ),
            (
                "lab",
                vec![(
                    "Secret",
                    "secret",
                    "- cites [[open:Alpha]]\n- relates_to [[Nowhere At All]]\n\nAnd a \
                     bare [[Alpha]] besides.\n"
                        .to_string(),
                )],
            ),
        ];
        if !team {
            // A third shared domain, empty, so a cross-domain move has
            // somewhere to land that everybody may write. Left out of the team
            // fixture, whose assertions count the team domains a caller sees.
            layout.push(("spare", Vec::new()));
        }
        for (name, engrams) in layout {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("MANIFEST.md"), manifest(name)).unwrap();
            for (title, permalink, body) in engrams {
                std::fs::write(
                    dir.join(format!("{permalink}.md")),
                    engram(title, permalink, &body),
                )
                .unwrap();
            }
            let entry = if team {
                DomainEntry {
                    origin: Some(OriginConfig {
                        repo: format!("acme/{name}"),
                        path: None,
                        branch: None,
                        poll_secs: None,
                    }),
                    ..DomainEntry::file(dir)
                }
            } else {
                DomainEntry::file(dir)
            };
            cfg.domains.insert(name.to_string(), entry);
        }
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        if team {
            // The state a first pull would have left behind. Without it a
            // status read fails on the missing file instead of reporting what
            // it knows, and nothing here downloads a repository.
            for name in ["open", "lab"] {
                OriginState::new(format!("acme/{name}"), "main")
                    .save(&root.join("origins").join(name))
                    .unwrap();
            }
        }
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                // Pinned inside the temp directory: nothing in this suite may
                // reach the developer's own state directory or credentials.
                .with_origins_dir(root.join("origins"))
                .with_token_store_dir(root.join("tokens")),
        );
        engine.sync(None).await.unwrap();
        let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
        for (name, role) in [
            ("owner", Role::Editor),
            ("mem", Role::Editor),
            ("out", Role::Editor),
            ("mgr", Role::Editor),
            ("boss", Role::Admin),
        ] {
            auth.add_user(name, name, None, role, "s3cret")
                .await
                .unwrap();
        }
        let addr = serve(engine, auth.clone());
        RestCtx {
            addr,
            auth,
            _tmp: tmp,
        }
    }

    /// Close a domain, under `owner`. Straight into the accounts store rather
    /// than through the route that does it: the route is admin-only and is
    /// itself under test below, and a fixture that had to satisfy the gate it
    /// is setting up for would be testing itself.
    async fn make_private(&self, domain: &str, owner: &str) {
        self.auth
            .set_domain_visibility(domain, true, owner)
            .await
            .unwrap();
    }

    async fn add_member(&self, domain: &str, account: &str, level: MemberLevel) {
        self.auth
            .upsert_domain_member(domain, account, level, "owner")
            .await
            .unwrap();
    }

    /// The anonymous viewer: no cookie, no CSRF token, no account.
    fn as_anonymous(&self) -> SessionClient {
        SessionClient {
            addr: self.addr,
            session: None,
        }
    }

    async fn as_user(&self, name: &str) -> SessionClient {
        SessionClient {
            addr: self.addr,
            session: Some(login(self.addr, name, "s3cret").await),
        }
    }
}

fn serve(engine: Arc<Engine>, auth: Arc<AuthStore>) -> std::net::SocketAddr {
    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth, None).unwrap();
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
    addr
}

async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = SessionClient::client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login must succeed");
    let cookie = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| v.split(';').next()?.strip_prefix("fluid_session="))
        .expect("login sets the session cookie")
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    let csrf = body["csrf"].as_str().expect("login returns a csrf token");
    (cookie, csrf.to_string())
}

/// The name of a domain nobody may see is absent from every listing, and every
/// route that addresses it answers the 404 an unregistered name answers - the
/// same status AND the same words, which is the part that makes it a secret
/// rather than a hint.
#[tokio::test]
async fn a_private_domain_is_absent_for_strangers_and_served_to_members() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mem", MemberLevel::Viewer).await;

    let out = ctx.as_user("out").await;
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(listed.contains("open"), "{listed}");
    assert!(!listed.contains("lab"), "a stranger sees no lab: {listed}");

    // Every domain-addressed read, and the unregistered name each is compared
    // against, so the two answers are asserted equal rather than merely both
    // being 404s.
    for (real, ghost) in [
        (
            "/api/v1/domains/lab/engrams",
            "/api/v1/domains/ghost/engrams",
        ),
        ("/api/v1/domains/lab/tree", "/api/v1/domains/ghost/tree"),
        (
            "/api/v1/domains/lab/manifest",
            "/api/v1/domains/ghost/manifest",
        ),
        (
            "/api/v1/domains/lab/attachments",
            "/api/v1/domains/ghost/attachments",
        ),
        (
            "/api/v1/domains/lab/engrams/secret",
            "/api/v1/domains/ghost/engrams/secret",
        ),
        (
            "/api/v1/domains/lab/files/assets/x.png",
            "/api/v1/domains/ghost/files/assets/x.png",
        ),
    ] {
        let hidden = out.get_text(real, 404).await;
        let missing = out.get_text(ghost, 404).await;
        assert_eq!(
            hidden.replace("lab", "ghost"),
            missing,
            "{real} must answer exactly what {ghost} answers"
        );
        assert!(
            !hidden.contains("forbidden") && !hidden.contains("membership"),
            "and must not hint at what it is hiding: {hidden}"
        );
    }

    // Search never names it either, and the domain filter is not a way in.
    let searched = out.get_json("/api/v1/search?q=secret").await.to_string();
    assert!(!searched.contains("Secret"), "{searched}");
    let filtered = out
        .get_json("/api/v1/search?q=secret&domains=lab")
        .await
        .to_string();
    assert!(!filtered.contains("Secret"), "{filtered}");

    // The member and the admin both see it.
    for name in ["mem", "boss", "owner"] {
        let client = ctx.as_user(name).await;
        let listed = client.get_json("/api/v1/domains").await.to_string();
        assert!(listed.contains("lab"), "{name} must see lab: {listed}");
        let read = client
            .get_json("/api/v1/domains/lab/engrams/secret")
            .await
            .to_string();
        assert!(read.contains("Secret"), "{name} reads it: {read}");
    }
}

/// The membership level decides a write, and it decides it on top of the
/// instance role rather than instead of it: a stranger is told nothing, a
/// member below editor is told their level, and neither is confused with the
/// other.
#[tokio::test]
async fn membership_level_gates_writes_not_instance_role() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    // An instance EDITOR, so a refusal here can only come from the membership.
    ctx.add_member("lab", "mem", MemberLevel::Viewer).await;

    let body = json!({"title": "Fresh", "content": "# Fresh\n"});
    let mem = ctx.as_user("mem").await;
    let refused = mem
        .post_json("/api/v1/domains/lab/engrams", body.clone())
        .await;
    assert_eq!(refused.status(), 403);
    let detail = refused.text().await.unwrap();
    assert!(
        detail.contains("viewer") && detail.contains("editor access is required"),
        "the refusal names the level held and the level needed: {detail}"
    );

    let out = ctx.as_user("out").await;
    let hidden = out
        .post_json("/api/v1/domains/lab/engrams", body.clone())
        .await;
    assert_eq!(
        hidden.status(),
        404,
        "a stranger's write is refused by the domain not existing for them"
    );

    // The other half of the rule: a domain invitation widens what an account
    // may reach, never what its instance role lets it do. `mem` writes the
    // shared domain because it is an instance editor there.
    let allowed = mem.post_json("/api/v1/domains/open/engrams", body).await;
    assert_eq!(allowed.status(), 201, "{:?}", allowed.text().await);

    // And an editor member does write the private domain.
    ctx.add_member("lab", "mem", MemberLevel::Editor).await;
    let now = mem
        .post_json(
            "/api/v1/domains/lab/engrams",
            json!({"title": "Second", "content": "# Second\n"}),
        )
        .await;
    assert_eq!(now.status(), 201, "{:?}", now.text().await);
}

/// A move writes at both ends, so it is gated at both: out of a domain and
/// into one. The destination rides in the body rather than the path, which
/// changes nothing - naming a domain is not a way to learn that it exists.
#[tokio::test]
async fn a_move_is_gated_at_both_ends() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mem", MemberLevel::Viewer).await;

    // A stranger may write `open` and may not see `lab`: the destination is
    // what refuses, in the words of a domain that does not exist.
    let out = ctx.as_user("out").await;
    let into_hidden = out
        .post_json(
            "/api/v1/domains/open/move",
            json!({"permalink": "alpha", "destination": "alpha", "destination_domain": "lab"}),
        )
        .await;
    assert_eq!(into_hidden.status(), 404);
    let detail = into_hidden.text().await.unwrap();
    assert!(!detail.contains("membership"), "{detail}");

    // A viewer member may see the destination, so it is refused by level.
    let mem = ctx.as_user("mem").await;
    let into_readonly = mem
        .post_json(
            "/api/v1/domains/open/move",
            json!({"permalink": "alpha", "destination": "alpha", "destination_domain": "lab"}),
        )
        .await;
    assert_eq!(into_readonly.status(), 403);
    let detail = into_readonly.text().await.unwrap();
    assert!(detail.contains("viewer"), "{detail}");

    // And the source end: out of a domain the caller may not see at all.
    let out_of_hidden = out
        .post_json(
            "/api/v1/domains/lab/move",
            json!({"permalink": "secret", "destination": "secret", "destination_domain": "open"}),
        )
        .await;
    assert_eq!(out_of_hidden.status(), 404);

    // Nothing moved.
    let boss = ctx.as_user("boss").await;
    boss.get_text("/api/v1/domains/open/engrams/alpha", 200)
        .await;
    boss.get_text("/api/v1/domains/lab/engrams/secret", 200)
        .await;
}

/// What points at an engram is a list of other domains, so it is the one read
/// whose ROWS are mostly about somewhere else. A referrer inside a hidden
/// domain is absent from the page, from the total and from the per-relation
/// summary - and the count the engram's own read reports agrees with it,
/// though the two are filtered by different machinery.
#[tokio::test]
async fn an_inbound_referrer_in_a_hidden_domain_is_not_reported() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;

    let boss = ctx.as_user("boss").await;
    let seen = boss.get_json("/api/v1/domains/open/inbound/alpha").await;
    assert_eq!(seen["total"], 1, "{seen}");
    assert!(seen.to_string().contains("Secret"), "{seen}");

    let out = ctx.as_user("out").await;
    let hidden = out.get_json("/api/v1/domains/open/inbound/alpha").await;
    assert_eq!(
        hidden["total"], 0,
        "the total counts what it showed: {hidden}"
    );
    assert!(hidden["hits"].as_array().unwrap().is_empty(), "{hidden}");
    assert!(
        hidden["types"].as_array().unwrap().is_empty(),
        "the summary would otherwise count a reference the reader may not see: {hidden}"
    );
    assert!(!hidden.to_string().contains("lab"), "{hidden}");

    // The engram's own read filters its inbound block in the engine, in Rust,
    // where this route filters in SQL. Two mechanisms, one property: they must
    // report the same number.
    let read = out.get_json("/api/v1/domains/open/engrams/alpha").await;
    let count = read["inbound"]["count"].as_u64().unwrap_or(0);
    assert_eq!(count, 0, "{read}");
    let seen_read = boss.get_json("/api/v1/domains/open/engrams/alpha").await;
    assert_eq!(
        seen_read["inbound"]["count"].as_u64().unwrap_or(0),
        seen["total"].as_u64().unwrap(),
        "the two inbound counts agree for a caller who sees everything"
    );
}

/// The consolidation queue names a domain, an engram and its evidence on every
/// row, so an unscoped sweep is a list of private domain names with their
/// contents attached.
#[tokio::test]
async fn the_evolve_queue_names_no_hidden_domain() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;

    let boss = ctx.as_user("boss").await;
    let swept = boss.get_json("/api/v1/evolve").await.to_string();
    assert!(swept.contains("lab"), "an admin sweeps everything: {swept}");

    let out = ctx.as_user("out").await;
    let scoped = out.get_json("/api/v1/evolve").await.to_string();
    assert!(!scoped.contains("lab"), "{scoped}");
    assert!(
        scoped.contains("open"),
        "and the domains it may see are still swept: {scoped}"
    );

    // Naming the domain is not a way in either: it is refused as an
    // unregistered name is.
    let named = out.get("/api/v1/evolve?domains=lab").await;
    assert_eq!(named.status(), 404);
    let ghost = out.get("/api/v1/evolve?domains=ghost").await;
    assert_eq!(ghost.status(), 404);
}

/// The two directions of the visibility verb are two different decisions.
///
/// CLOSING a shared domain is the instance's: it hands the domain to whoever
/// called, so a shared domain would otherwise be taken by whoever asked first.
/// OPENING a private one is its owner's, or an admin's: the owner already sees
/// everything in it, and opening what they closed takes nothing from anybody.
/// A manager may do neither - that is where a manager's authority ends.
#[tokio::test]
async fn the_owner_re_shares_and_only_an_admin_closes_a_domain() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mgr", MemberLevel::Manager).await;

    let mgr = ctx.as_user("mgr").await;
    let refused = mgr
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(
        refused.status(),
        403,
        "a manager invites and changes levels, and never changes visibility"
    );

    let out = ctx.as_user("out").await;
    let stranger = out
        .put_json("/api/v1/domains/lab/visibility", json!({"private": true}))
        .await;
    assert_eq!(
        stranger.status(),
        403,
        "a stranger closing a domain is refused by the role gate, which runs \
         first and says nothing about which domains exist"
    );
    let stranger = out
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(
        stranger.status(),
        404,
        "and opening one they cannot see is the answer an unregistered name \
         gets: this direction has to resolve the owner, so it must not confirm \
         the domain exists"
    );

    // The owner opens what the owner closed.
    let owner = ctx.as_user("owner").await;
    let opened = owner
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(opened.status(), 204, "{:?}", opened.text().await);
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(
        listed.contains("lab"),
        "the domain is shared again for everyone: {listed}"
    );
    assert!(
        ctx.auth.memberships_of("mgr").await.unwrap().is_empty(),
        "opening a domain forgot who was invited into it"
    );

    // And cannot close it again: a shared domain has no owner, so there is
    // nobody but the instance to ask.
    let refused = owner
        .put_json("/api/v1/domains/lab/visibility", json!({"private": true}))
        .await;
    assert_eq!(
        refused.status(),
        403,
        "closing a shared domain would hand it to the caller, so it stays the \
         instance's decision"
    );

    let boss = ctx.as_user("boss").await;
    let closed = boss
        .put_json("/api/v1/domains/lab/visibility", json!({"private": true}))
        .await;
    assert_eq!(closed.status(), 204);
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(!listed.contains("lab"), "{listed}");
    assert_eq!(
        ctx.auth
            .domain_visibility("lab")
            .await
            .unwrap()
            .unwrap()
            .owner,
        "boss",
        "closing a domain names the caller as its owner"
    );
}

/// A name nobody registered is refused before an acl row is minted for it: the
/// visibility records are keyed by domain name and cannot know which domains
/// exist, so a typo would otherwise close a domain that does not exist yet.
#[tokio::test]
async fn privatizing_an_unknown_domain_mints_nothing() {
    let ctx = RestCtx::two_domains().await;
    let boss = ctx.as_user("boss").await;

    let refused = boss
        .put_json("/api/v1/domains/ghost/visibility", json!({"private": true}))
        .await;
    assert_eq!(refused.status(), 404);
    assert!(
        ctx.auth.private_domains().await.unwrap().is_empty(),
        "no acl row was written for a domain nobody registered"
    );
}

/// The instance-wide share summary enumerates every team domain rather than
/// addressing one, so it is the route where a private domain's NAME leaks
/// without any of its content being read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sync_summary_lists_only_visible_team_domains() {
    let ctx = RestCtx::two_team_domains().await;
    ctx.make_private("lab", "owner").await;

    let boss = ctx.as_user("boss").await;
    let all = boss.get_json("/api/v1/sync").await;
    let names: Vec<&str> = all["domains"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["domain"].as_str())
        .collect();
    assert!(names.contains(&"lab") && names.contains(&"open"), "{all}");

    // An instance editor reaches this route because the instance shares
    // personally; what it may see is what it is a member of.
    let out = ctx.as_user("out").await;
    let scoped = out.get_json("/api/v1/sync").await;
    let names: Vec<&str> = scoped["domains"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["domain"].as_str())
        .collect();
    assert_eq!(names, vec!["open"], "{scoped}");
    assert!(!scoped.to_string().contains("lab"), "{scoped}");

    // And the per-domain sync surface answers for it exactly as it answers
    // for a domain nobody registered.
    let hidden = out.get_text("/api/v1/domains/lab/sync", 404).await;
    let missing = out.get_text("/api/v1/domains/ghost/sync", 404).await;
    assert_eq!(hidden.replace("lab", "ghost"), missing);
    boss.get_text("/api/v1/domains/lab/sync", 200).await;
}

/// A cross-domain move reaches past the two domains it names: it gathers every
/// inbound reference to the engram, across domains, and rewrites the bare ones
/// into the prefixed form - but only inside domains the mover may see. The
/// private domain holds a bare reference to the moving engram, and
/// `links_rewritten` being 0 here is what says the rewrite did NOT run inside
/// it: a stranger's move leaves a hidden domain's files untouched (its members
/// see the dangling link as an unresolved-link finding), and the count names
/// visible rewrites only.
///
/// What must NOT happen either is the disclosure: the receipt carries a count
/// and nothing else about the referrer - no domain name, no title, no path.
#[tokio::test]
async fn a_cross_domain_move_receipt_names_no_hidden_domain() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;

    let out = ctx.as_user("out").await;
    let moved = out
        .post_json(
            "/api/v1/domains/open/move",
            json!({"permalink": "alpha", "destination": "alpha", "destination_domain": "spare"}),
        )
        .await;
    assert_eq!(moved.status(), 200, "{:?}", moved.text().await);
    let receipt = moved.text().await.unwrap();
    let receipt_json: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(
        receipt_json["links_rewritten"], 0,
        "the rewrite must not run inside the private domain: {receipt}"
    );
    assert!(
        !receipt.contains("lab") && !receipt.contains("Secret"),
        "the referrer inside the private domain is not named: {receipt}"
    );
}

/// The domain a route gates is the one in its path - and an identifier in the
/// body must not be able to name another one. The absolute
/// `crystalline://<domain>/<permalink>` form overrides the domain hint
/// wherever it is accepted, so a write that resolved it would act on a domain
/// nobody gated: a move that carries an engram OUT of a private domain, a
/// supersede pair written into a private engram's file, an `evolve_ack`
/// stamped into one, and - success against not-found - an existence oracle
/// for any permalink in it.
///
/// Refused for every caller, the admin included: the rule is that a write
/// resolves inside the domain its request named, not that some callers may
/// cross. An admin who wants the private domain addresses it by path, where
/// the gate serves them.
#[tokio::test]
async fn an_absolute_identifier_cannot_reach_another_domain() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    // A member who may read the private domain and not write it: the case
    // where the caller can see the name and still must not act on it.
    ctx.add_member("lab", "mem", MemberLevel::Viewer).await;

    let hidden = "crystalline://lab/secret";
    for name in ["out", "mem", "boss"] {
        let client = ctx.as_user(name).await;

        // A move OUT of the private domain, addressed through a domain the
        // caller may write.
        let moved = client
            .post_json(
                "/api/v1/domains/open/move",
                json!({"permalink": hidden, "destination": "stolen"}),
            )
            .await;
        assert_eq!(moved.status(), 404, "move as {name}");

        // A supersede pair wired into the private engram's file.
        let retired = client
            .post_json(
                "/api/v1/domains/open/retire",
                json!({"permalink": "alpha", "status": "superseded", "successor": hidden}),
            )
            .await;
        assert_eq!(retired.status(), 404, "retire as {name}");

        // Both acknowledgment verbs, which edit the engram they name.
        let acked = client
            .post_json(
                "/api/v1/domains/open/evolve/ack",
                json!({"permalink": hidden, "rule": "V006"}),
            )
            .await;
        assert_eq!(acked.status(), 404, "ack as {name}");
        let unacked = client
            .request(reqwest::Method::DELETE, "/api/v1/domains/open/evolve/ack")
            .json(&json!({"permalink": hidden, "rule": "V006"}))
            .send()
            .await
            .unwrap();
        assert_eq!(unacked.status(), 404, "unack as {name}");
    }

    // Nothing moved, nothing was written into either engram, and the refusals
    // above were not an oracle: a permalink nobody wrote answers the same way
    // one that exists does.
    let boss = ctx.as_user("boss").await;
    let secret = boss.get_json("/api/v1/domains/lab/engrams/secret").await;
    assert!(
        !secret["content"].as_str().unwrap().contains("supersedes"),
        "{secret}"
    );
    assert!(secret["frontmatter"]["evolve_ack"].is_null(), "{secret}");
    let alpha = boss.get_json("/api/v1/domains/open/engrams/alpha").await;
    assert_eq!(alpha["frontmatter"]["status"], "stable", "{alpha}");
    let ghost = boss
        .post_json(
            "/api/v1/domains/open/move",
            json!({"permalink": "crystalline://lab/nobody-wrote-this", "destination": "x"}),
        )
        .await;
    assert_eq!(ghost.status(), 404);

    // The same-domain absolute form still writes: what is refused is crossing
    // domains, not the absolute form itself.
    let same_domain = boss
        .post_json(
            "/api/v1/domains/open/retire",
            json!({"permalink": "crystalline://open/alpha", "status": "deprecated"}),
        )
        .await;
    assert_eq!(same_domain.status(), 200, "{:?}", same_domain.text().await);
}

/// The anonymous viewer tier, end to end: an identity with no account behind
/// it sees what is shared and no private domain, and is refused an addressed
/// read of one in the words a name nobody registered gets.
#[tokio::test]
async fn the_anonymous_viewer_sees_no_private_domain() {
    let ctx = RestCtx::anonymous_instance().await;
    ctx.make_private("lab", "owner").await;

    let nobody = ctx.as_anonymous();
    let listed = nobody.get_json("/api/v1/domains").await.to_string();
    assert!(listed.contains("open"), "{listed}");
    assert!(!listed.contains("lab"), "{listed}");

    let hidden = nobody
        .get_text("/api/v1/domains/lab/engrams/secret", 404)
        .await;
    let missing = nobody
        .get_text("/api/v1/domains/ghost/engrams/secret", 404)
        .await;
    assert_eq!(hidden.replace("lab", "ghost"), missing);

    // And a write is refused without ever confirming the domain: an anonymous
    // identity never writes, whatever `auth.anonymous` allows it to read.
    let refused = nobody
        .post_json(
            "/api/v1/domains/lab/engrams",
            json!({"title": "Nope", "content": "x"}),
        )
        .await;
    assert_eq!(refused.status(), 404);
}

/// The branch of the sweep shim that must never widen: with every domain
/// private and the caller a member of none of them, an empty domain filter
/// cannot be handed to a verb that reads one as "sweep everything".
#[tokio::test]
async fn a_sweep_with_nothing_visible_refuses_rather_than_widening() {
    let ctx = RestCtx::two_domains().await;
    for domain in ["open", "lab", "spare"] {
        ctx.make_private(domain, "owner").await;
    }

    let out = ctx.as_user("out").await;
    let refused = out.get("/api/v1/evolve").await;
    let status = refused.status();
    let body = refused.text().await.unwrap();
    assert_eq!(status, 404, "{body}");
    for domain in ["open", "lab", "spare"] {
        assert!(!body.contains(domain), "and it names none of them: {body}");
    }

    // The owner still sweeps its own.
    let owner = ctx.as_user("owner").await;
    let swept = owner.get_json("/api/v1/evolve").await.to_string();
    assert!(swept.contains("lab"), "{swept}");
}

/// The manager pin, from the plan: a manager invites and re-levels, and the
/// same manager cannot change what the domain's visibility is.
#[tokio::test]
async fn manager_invites_but_cannot_change_visibility() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mgr", MemberLevel::Manager).await;

    let mgr = ctx.as_user("mgr").await;
    let invited = mgr
        .put_json(
            "/api/v1/domains/lab/members/out",
            json!({"level": "editor"}),
        )
        .await;
    assert_eq!(invited.status(), 204, "{:?}", invited.text().await);
    assert_eq!(
        ctx.auth.memberships_of("out").await.unwrap(),
        vec![("lab".to_string(), MemberLevel::Editor)]
    );

    // The same person, one route over.
    let refused = mgr
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(refused.status(), 403);
    let refused = mgr
        .put_json("/api/v1/domains/lab/owner", json!({"owner": "mgr"}))
        .await;
    assert_eq!(
        refused.status(),
        403,
        "and cannot hand the domain to itself either"
    );

    let owner = ctx.as_user("owner").await;
    let opened = owner
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(opened.status(), 204, "{:?}", opened.text().await);
}

/// Who may read the member list: everybody who can see the domain, and nobody
/// else. A stranger is answered exactly as for a domain nobody registered.
#[tokio::test]
async fn the_member_list_is_served_to_members_and_hidden_from_strangers() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mem", MemberLevel::Viewer).await;

    // A viewer-level member sees who else is here: this is what the domain
    // card draws, and being invited is what earns it.
    let mem = ctx.as_user("mem").await;
    let listed = mem.get_json("/api/v1/domains/lab/members").await;
    assert_eq!(listed["owner"], json!("owner"));
    assert_eq!(listed["visibility"], json!("private"));
    assert_eq!(listed["members"][0]["principal"], json!("mem"));
    assert_eq!(listed["members"][0]["level"], json!("viewer"));
    assert_eq!(listed["members"].as_array().unwrap().len(), 1);

    // A stranger gets the answer an unregistered name gets, word for word.
    let out = ctx.as_user("out").await;
    let hidden = out.get_text("/api/v1/domains/lab/members", 404).await;
    let missing = out.get_text("/api/v1/domains/ghost/members", 404).await;
    assert_eq!(hidden.replace("lab", "ghost"), missing);

    // A shared domain answers honestly rather than refusing: no owner, no
    // members, because membership decides nothing while a domain is shared.
    let shared = out.get_json("/api/v1/domains/open/members").await;
    assert_eq!(shared["owner"], json!(null));
    assert_eq!(shared["visibility"], json!("shared"));
    assert!(shared["members"].as_array().unwrap().is_empty());
}

/// The anonymous tier reads the member listing of what it can already read,
/// and finds no private domain there.
///
/// The listing is the one route on this surface that does not demand an
/// account, and this is why: an instance serving `auth.anonymous` serves that
/// caller the domain list, search and every engram in a shared domain, so a
/// member listing that alone answered 401 would break a card whose every other
/// call succeeds. Nothing is given away by serving it - a shared domain has no
/// owner and no members, and a private one is absent.
#[tokio::test]
async fn the_anonymous_viewer_reads_a_shared_member_listing_and_no_private_one() {
    let ctx = RestCtx::anonymous_instance().await;
    ctx.make_private("lab", "owner").await;

    let nobody = ctx.as_anonymous();
    let shared = nobody.get_json("/api/v1/domains/open/members").await;
    assert_eq!(shared["visibility"], json!("shared"));
    assert_eq!(shared["owner"], json!(null));
    assert!(shared["members"].as_array().unwrap().is_empty());

    let hidden = nobody.get_text("/api/v1/domains/lab/members", 404).await;
    let missing = nobody.get_text("/api/v1/domains/ghost/members", 404).await;
    assert_eq!(hidden.replace("lab", "ghost"), missing);

    // And it administers nothing: every mutation tells it to log in, which is
    // exactly what would change the answer.
    let refused = nobody
        .put_json(
            "/api/v1/domains/lab/members/mem",
            json!({"level": "editor"}),
        )
        .await;
    assert_eq!(refused.status(), 401);
    assert_eq!(
        nobody
            .delete("/api/v1/domains/lab/members/mem")
            .await
            .status(),
        401
    );
    let refused = nobody
        .put_json("/api/v1/domains/lab/owner", json!({"owner": "mem"}))
        .await;
    assert_eq!(refused.status(), 401);
}

/// A member may always remove itself. Leaving is not an administrative act,
/// and the domain closes behind them.
#[tokio::test]
async fn a_member_leaves_a_domain_without_asking_a_manager() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mem", MemberLevel::Viewer).await;
    ctx.add_member("lab", "out", MemberLevel::Viewer).await;

    let mem = ctx.as_user("mem").await;
    // Not a manager, so evicting somebody else is refused...
    let refused = mem.delete("/api/v1/domains/lab/members/out").await;
    assert_eq!(refused.status(), 403);
    // ...and leaving is not.
    let left = mem.delete("/api/v1/domains/lab/members/MEM").await;
    assert_eq!(
        left.status(),
        204,
        "the login name is folded, so the path segment's case cannot turn a \
         departure into an eviction: {:?}",
        left.text().await
    );
    assert!(ctx.auth.memberships_of("mem").await.unwrap().is_empty());
    // And the domain is gone from behind them.
    let gone = mem.get("/api/v1/domains/lab/members").await;
    assert_eq!(gone.status(), 404);

    // The owner is not a membership row and is refused with the route that
    // does change who it is.
    let owner = ctx.as_user("owner").await;
    let refused = owner.delete("/api/v1/domains/lab/members/owner").await;
    assert_eq!(refused.status(), 409);
    assert!(
        refused.text().await.unwrap().contains("owner"),
        "the refusal names the route that hands the domain on"
    );

    // A name that is not a member of this domain is a plain 404.
    let missing = owner.delete("/api/v1/domains/lab/members/mem").await;
    assert_eq!(missing.status(), 404);
}

/// Handing a domain on: the admin does it, and the old owner keeps nothing.
#[tokio::test]
async fn an_admin_transfers_a_domain_and_the_old_owner_becomes_a_stranger() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mem", MemberLevel::Editor).await;

    let boss = ctx.as_user("boss").await;
    let handed = boss
        .put_json("/api/v1/domains/lab/owner", json!({"owner": "mem"}))
        .await;
    assert_eq!(handed.status(), 204, "{:?}", handed.text().await);
    assert_eq!(
        ctx.auth
            .domain_visibility("lab")
            .await
            .unwrap()
            .unwrap()
            .owner,
        "mem"
    );
    assert!(
        ctx.auth.memberships_of("mem").await.unwrap().is_empty(),
        "the new owner's editor row is gone: it could only say less"
    );

    // The old owner is a stranger now, and the domain is answered as one
    // nobody registered.
    let owner = ctx.as_user("owner").await;
    assert_eq!(owner.get("/api/v1/domains/lab/members").await.status(), 404);
    let listed = owner.get_json("/api/v1/domains").await.to_string();
    assert!(!listed.contains("lab"), "{listed}");

    // Unless the new owner invites them back.
    let mem = ctx.as_user("mem").await;
    let invited = mem
        .put_json(
            "/api/v1/domains/lab/members/owner",
            json!({"level": "viewer"}),
        )
        .await;
    assert_eq!(invited.status(), 204, "{:?}", invited.text().await);
    let back = owner.get_json("/api/v1/domains/lab/members").await;
    assert_eq!(back["owner"], json!("mem"));
    assert_eq!(back["members"][0]["principal"], json!("owner"));
}

/// A domain whose owner's account was removed has no owner at all, and the
/// listing says so rather than showing an empty name. Only an admin can reach
/// it, which is the fail-closed answer the resolver already gives.
#[tokio::test]
async fn a_domain_whose_owner_was_removed_reports_no_owner() {
    let ctx = RestCtx::two_domains().await;
    ctx.make_private("lab", "owner").await;
    ctx.add_member("lab", "mem", MemberLevel::Editor).await;
    ctx.auth.remove_user("owner").await.unwrap();

    let boss = ctx.as_user("boss").await;
    let listed = boss.get_json("/api/v1/domains/lab/members").await;
    assert_eq!(
        listed["owner"],
        json!(null),
        "no owner, rather than an account whose name is empty: {listed}"
    );
    assert_eq!(listed["visibility"], json!("private"));
    assert_eq!(listed["members"][0]["principal"], json!("mem"));

    // And an admin can hand it to somebody, which is how it gets an owner
    // again.
    let handed = boss
        .put_json("/api/v1/domains/lab/owner", json!({"owner": "mem"}))
        .await;
    assert_eq!(handed.status(), 204, "{:?}", handed.text().await);
}

/// Membership means nothing on a shared domain, so every mutation is refused
/// there - and named as a conflict with the domain's current state rather than
/// as a permission problem, since making it private is what comes first.
#[tokio::test]
async fn membership_is_refused_on_a_shared_domain() {
    let ctx = RestCtx::two_domains().await;
    let boss = ctx.as_user("boss").await;

    let refused = boss
        .put_json(
            "/api/v1/domains/open/members/mem",
            json!({"level": "editor"}),
        )
        .await;
    assert_eq!(refused.status(), 409, "{:?}", refused.text().await);
    let refused = boss.delete("/api/v1/domains/open/members/mem").await;
    assert_eq!(refused.status(), 409);
    let refused = boss
        .put_json("/api/v1/domains/open/owner", json!({"owner": "mem"}))
        .await;
    assert_eq!(refused.status(), 409);

    // And a principal nobody has an account for is an unprocessable body, not
    // a row left waiting for somebody to claim the name.
    ctx.make_private("lab", "owner").await;
    let refused = boss
        .put_json(
            "/api/v1/domains/lab/members/ghost",
            json!({"level": "editor"}),
        )
        .await;
    assert_eq!(refused.status(), 422, "{:?}", refused.text().await);
    assert!(ctx.auth.domain_members("lab").await.unwrap().is_empty());
}

/// A domain can be registered private in one step, owned by whoever created
/// it. That is the personal-private-domain case: an account makes itself a
/// domain nobody else can see, without a second call that would leave it
/// shared in between.
#[tokio::test]
async fn a_domain_can_be_created_private_and_belongs_to_its_creator() {
    let ctx = RestCtx::two_domains().await;
    let boss = ctx.as_user("boss").await;

    let created = boss
        .post_json(
            "/api/v1/domains",
            json!({"mode": "virtual", "name": "vault", "private": true}),
        )
        .await;
    assert_eq!(created.status(), 201, "{:?}", created.text().await);
    assert_eq!(
        ctx.auth
            .domain_visibility("vault")
            .await
            .unwrap()
            .unwrap()
            .owner,
        "boss",
        "the creator owns it"
    );

    let out = ctx.as_user("out").await;
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(!listed.contains("vault"), "{listed}");
    assert_eq!(out.get("/api/v1/domains/vault/members").await.status(), 404);

    // The default is unchanged: a domain created without the flag is shared.
    let created = boss
        .post_json(
            "/api/v1/domains",
            json!({"mode": "virtual", "name": "attic"}),
        )
        .await;
    assert_eq!(created.status(), 201, "{:?}", created.text().await);
    assert!(ctx.auth.domain_visibility("attic").await.unwrap().is_none());
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(listed.contains("attic"), "{listed}");
}
