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
        RestCtx::build(false).await
    }

    /// The same two domains, both carrying a GitHub origin, on an instance
    /// with GitHub on and `share_identity = personal` - which is what puts an
    /// instance EDITOR through the share gate, so the sync summary's own
    /// filtering is what the assertions see rather than the admin gate above
    /// it. Nothing here connects: the status read reports local state and says
    /// the connection is absent.
    async fn two_team_domains() -> RestCtx {
        RestCtx::build(true).await
    }

    async fn build(team: bool) -> RestCtx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig {
            auth: Some(AuthConfig {
                trusted_header: None,
                anonymous: Some(false),
                mcp: None,
                max_users: None,
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
                    "- cites [[open:Alpha]]\n- relates_to [[Nowhere At All]]".to_string(),
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

/// Only an admin decides whether a domain is private - not a manager, and not
/// the domain's own owner - because making a domain private transfers it to
/// the caller, and a verb that hands over a domain cannot be one a domain's
/// own administration may reach.
#[tokio::test]
async fn only_an_admin_changes_a_domains_visibility() {
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

    let owner = ctx.as_user("owner").await;
    let refused = owner
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(
        refused.status(),
        403,
        "and neither does the owner: the instance owns this decision"
    );

    let out = ctx.as_user("out").await;
    let stranger = out
        .put_json("/api/v1/domains/lab/visibility", json!({"private": true}))
        .await;
    assert_eq!(
        stranger.status(),
        403,
        "a stranger is refused by the role gate, which runs first and says \
         nothing about which domains exist"
    );

    let boss = ctx.as_user("boss").await;
    let opened = boss
        .put_json("/api/v1/domains/lab/visibility", json!({"private": false}))
        .await;
    assert_eq!(opened.status(), 204, "{:?}", opened.text().await);
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(
        listed.contains("lab"),
        "the domain is shared again for everyone: {listed}"
    );

    // Closing it again names the caller as its owner and forgets the old
    // membership list.
    let closed = boss
        .put_json("/api/v1/domains/lab/visibility", json!({"private": true}))
        .await;
    assert_eq!(closed.status(), 204);
    let listed = out.get_json("/api/v1/domains").await.to_string();
    assert!(!listed.contains("lab"), "{listed}");
    assert!(
        ctx.auth.memberships_of("mgr").await.unwrap().is_empty(),
        "opening a domain forgot who was invited into it"
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
/// into the prefixed form. The private domain holds one of those references
/// here, so the receipt is where it could surface - and it must not, in any
/// form. `links_rewritten` is a count and nothing in the answer names a domain
/// but the two ends of the move.
///
/// (The reference in this fixture is written prefixed, which the rewrite skips,
/// so no file inside the private domain is touched by this particular move.
/// What is pinned is the receipt's shape, which is the same whichever branch
/// the rewrite takes.)
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
    assert!(
        !receipt.contains("lab") && !receipt.contains("Secret"),
        "the referrer inside the private domain is not named: {receipt}"
    );
}
