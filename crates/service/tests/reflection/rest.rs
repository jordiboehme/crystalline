//! The REST contract Fluid builds on.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::AuthConfig;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, MemberLevel, ReflectionLevel, Role};
use serde_json::{Value, json};

use crate::fixture::{Fx, engram, fixture_with, manifest};

pub struct Served {
    pub addr: std::net::SocketAddr,
    pub fx: Fx,
    pub auth: Arc<AuthStore>,
}

/// `eng` private to keeper, `ann` a member with knowledge `none` and
/// Reflection `contributor`, `vera` a viewer member, `ed` an editor member;
/// `nora` an instance account with knowledge `none` and Reflection
/// `contributor`. With `anonymous`, `eng` stays shared (public) and has no
/// members, and the anonymous viewer is on. Reflection on, one engram, one
/// curated reflection.
pub async fn serve() -> Served {
    serve_with(false).await
}

pub async fn serve_with(anonymous: bool) -> Served {
    serve_opts(Opts {
        anonymous,
        private: !anonymous,
        origin: false,
    })
    .await
}

/// How [`serve_opts`] sets the instance up.
#[derive(Clone, Copy)]
pub struct Opts {
    /// The anonymous viewer is on.
    pub anonymous: bool,
    /// `eng` is private to keeper, with the members [`serve`] names.
    pub private: bool,
    /// `eng` is connected to a GitHub repository and its MANIFEST names no
    /// `reflection_home`, so its Reflection is homeless (and no page is
    /// written: a homeless Reflection takes none).
    pub origin: bool,
}

pub async fn serve_opts(opts: Opts) -> Served {
    let Opts {
        anonymous,
        private,
        origin,
    } = opts;
    let fx = fixture_with(
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        move |cfg, _| {
            cfg.auth = Some(AuthConfig {
                anonymous: Some(anonymous),
                ..Default::default()
            });
            if origin {
                cfg.domains.get_mut("eng").unwrap().origin =
                    Some(crystalline_core::config::OriginConfig {
                        repo: "acme/kb".into(),
                        path: None,
                        branch: None,
                        poll_secs: None,
                    });
            }
        },
    )
    .await;
    let mut p = crate::docs::write(
        "guides/deploying.md",
        "# Deploying\n\n## Night deploys\n\nDeploys run at night.\n",
    );
    p.sources = std::collections::BTreeMap::from([
        ("_top".to_string(), Vec::new()),
        (
            "Night deploys".to_string(),
            vec!["runbooks/deploy".to_string()],
        ),
    ]);
    if !origin {
        fx.engine
            .write_reflection(&p, &crystalline_service::ReflectionCaller::owner())
            .await
            .unwrap();
    }
    serve_fx(fx, private).await
}

/// Serve `fx`'s engine with [`serve`]'s accounts: `keeper`, `ann`, `vera`,
/// `ed` and `nora`, and with `private` the members of a private `eng`.
pub async fn serve_fx(fx: Fx, private: bool) -> Served {
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for (name, role) in [
        ("keeper", Role::Editor),
        ("ann", Role::Viewer),
        ("vera", Role::Viewer),
        ("ed", Role::Editor),
        ("nora", Role::None),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_reflection_level("nora", ReflectionLevel::Contributor)
        .await
        .unwrap();
    if private {
        auth.set_domain_visibility("eng", true, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member(
            "eng",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Contributor),
            "keeper",
        )
        .await
        .unwrap();
        auth.upsert_domain_member("eng", "vera", MemberLevel::Viewer, None, "keeper")
            .await
            .unwrap();
        auth.upsert_domain_member("eng", "ed", MemberLevel::Editor, None, "keeper")
            .await
            .unwrap();
    }
    let router = http_router(
        fx.engine.clone(),
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
    Served { addr, fx, auth }
}

pub struct Session {
    addr: std::net::SocketAddr,
    cookie: String,
    csrf: String,
}

impl Session {
    pub async fn login(addr: std::net::SocketAddr, name: &str) -> Session {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let resp = client
            .post(format!("http://{addr}/api/v1/auth/login"))
            .json(&json!({ "name": name, "password": "pw12345678" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let cookie = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| v.split(';').next()?.strip_prefix("fluid_session="))
            .unwrap()
            .to_string();
        let body: Value = resp.json().await.unwrap();
        Session {
            addr,
            cookie,
            csrf: body["csrf"].as_str().unwrap().to_string(),
        }
    }

    pub async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut req = client
            .request(method, format!("http://{}{path}", self.addr))
            .header("cookie", format!("fluid_session={}", self.cookie))
            .header("x-csrf-token", &self.csrf);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    pub async fn get(&self, path: &str) -> (u16, Value) {
        self.send(reqwest::Method::GET, path, None).await
    }
}

#[tokio::test]
async fn a_contributor_sees_no_sources_and_no_engram_route() {
    let s = serve().await;
    let ann = Session::login(s.addr, "ann").await;
    let (status, domains) = ann.get("/api/v1/domains").await;
    assert_eq!(status, 200);
    assert!(
        !domains.to_string().contains("\"eng\""),
        "hidden as engrams: {domains}"
    );
    let (status, listing) = ann.get("/api/v1/reflection/domains").await;
    assert_eq!(status, 200);
    assert_eq!(listing["domains"][0]["domain"], "eng");
    assert_eq!(listing["domains"][0]["right"], "edit");
    assert_eq!(listing["domains"][0]["engrams_visible"], false);
    let (status, _) = ann.get("/api/v1/domains/eng/engrams/runbooks/deploy").await;
    assert_eq!(status, 404, "no engram route answers a contributor");
    let (status, doc) = ann
        .get("/api/v1/domains/eng/reflections/guides/deploying.md")
        .await;
    assert_eq!(status, 200);
    assert!(
        doc["sections"][0].get("sources").is_none(),
        "absent, not empty: {doc}"
    );
    assert!(!doc.to_string().contains("runbooks/deploy"));
    // The markdown comes without the block, and `line` is counted on it.
    let markdown = doc["markdown"].as_str().unwrap();
    assert!(!markdown.contains("reflection:"), "{markdown}");
    let line = doc["sections"][0]["line"].as_u64().unwrap() as usize;
    assert_eq!(
        markdown.lines().nth(line - 1),
        Some("## Night deploys"),
        "{doc}"
    );
    // Search never answers a hit out of the block: `runbooks` is only there.
    let (status, hits) = ann
        .get("/api/v1/domains/eng/reflection/search?q=runbooks")
        .await;
    assert_eq!(status, 200);
    assert_eq!(hits["hits"].as_array().unwrap().len(), 0, "{hits}");
    // Knowledge `none` searches the Reflection it reads.
    let (status, hits) = ann
        .get("/api/v1/domains/eng/reflection/search?q=night")
        .await;
    assert_eq!(status, 200, "{hits}");
    assert_eq!(hits["hits"][0]["path"], "guides/deploying.md", "{hits}");
    // The answered checksum is the stored file's, so an edit of the stripped
    // text with it is not stale, and the stored block survives it.
    let (status, saved) = ann
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/guides/deploying.md",
            Some(json!({ "markdown": markdown.replace("Deploys run at night.", "Deploys run at night, after the batch."), "expected_checksum": doc["checksum"] })),
        )
        .await;
    assert_eq!(status, 200, "a contributor edits: {saved}");
    let stored =
        std::fs::read_to_string(s.fx.root.join("reflections/guides/deploying.md")).unwrap();
    assert!(
        stored.contains("engram: runbooks/deploy") && stored.contains("after the batch"),
        "{stored}"
    );
    let (_, queue) = ann.get("/api/v1/domains/eng/reflection/queue").await;
    assert!(queue["total"].as_u64().unwrap() >= 1);
    assert!(!queue.to_string().contains("runbooks/deploy"), "{queue}");
}

/// A page embedding an engram attachment makes an `engram_attachment` item
/// whose `targets` are engram file paths. Only a reader of engrams gets them.
#[tokio::test]
async fn a_contributor_gets_no_engram_attachment_targets_in_the_queue() {
    let fx = fixture_with(
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            ("assets/shot.png", "an engram attachment"),
            (
                "reflections/b.md",
                "# B\n\n## Shot\n<!-- r:aaaa0003 -->\n\n![e](../assets/shot.png)\n",
            ),
        ],
        |_, _| {},
    )
    .await;
    let s = serve_fx(fx, true).await;
    let queue_path = "/api/v1/domains/eng/reflection/queue";
    let has_item = |queue: &Value| {
        queue["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["what"] == "engram_attachment")
    };
    // The page's own text (a feedback diff) names the link; the item must not.
    let no_targets = |queue: &Value| {
        queue["queue"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("targets").is_none())
    };
    // ed reads engrams: the targets stay.
    let ed = Session::login(s.addr, "ed").await;
    let (status, queue) = ed.get(queue_path).await;
    assert_eq!(status, 200, "{queue}");
    let item = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["what"] == "engram_attachment")
        .unwrap_or_else(|| panic!("no engram_attachment item: {queue}"));
    assert_eq!(item["targets"], json!(["../assets/shot.png"]), "{queue}");
    // ann has knowledge none: the item stays, its targets do not.
    let ann = Session::login(s.addr, "ann").await;
    let (status, queue) = ann.get(queue_path).await;
    assert_eq!(status, 200, "{queue}");
    assert!(has_item(&queue), "the item is still asked for: {queue}");
    assert!(no_targets(&queue), "{queue}");
    // The review route carries the same queue.
    let (status, review) = ann.get("/api/v1/domains/eng/reflection/review").await;
    assert_eq!(status, 200, "{review}");
    assert!(has_item(&review["queue"]), "{review}");
    assert!(no_targets(&review["queue"]), "{review}");
}

#[tokio::test]
async fn a_create_refuses_an_existing_page() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let abs = s.fx.root.join("reflections/guides/deploying.md");
    let before = std::fs::read(&abs).unwrap();
    let (status, err) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/guides/deploying.md",
            Some(json!({ "markdown": "# Mine\n", "create": true })),
        )
        .await;
    assert_eq!(status, 409, "{err}");
    assert_eq!(
        err["detail"],
        "a reflection already lives at 'guides/deploying.md'"
    );
    assert_eq!(
        std::fs::read(&abs).unwrap(),
        before,
        "the page is untouched"
    );
    let (status, made) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/guides/new.md",
            Some(json!({ "markdown": "# New\n", "create": true })),
        )
        .await;
    assert_eq!(status, 200, "{made}");
    assert_eq!(made["created"], true);
}

#[tokio::test]
async fn the_rights_per_route() {
    let s = serve().await;
    let vera = Session::login(s.addr, "vera").await;
    let (status, _) = vera.get("/api/v1/domains/eng/reflection/tree").await;
    assert_eq!(status, 200, "a viewer reads");
    let (status, err) = vera
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/new.md",
            Some(json!({ "markdown": "# New\n" })),
        )
        .await;
    assert_eq!(status, 403, "{err}");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("is read, and edit is needed"),
        "{err}"
    );
    let (status, thread) = vera
        .send(reqwest::Method::POST, "/api/v1/domains/eng/reflection/threads", Some(json!({ "path": "guides/deploying.md", "section": "Night deploys", "quote": "at night", "text": "Still?" })))
        .await;
    assert_eq!(status, 201, "a viewer comments: {thread}");
    let id = thread["id"].as_str().unwrap();
    let (status, err) = vera
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/domains/eng/reflection/threads/{id}/resolve"),
            Some(json!({ "path": "guides/deploying.md", "outcome": "" })),
        )
        .await;
    assert_eq!(status, 422, "a resolve needs its outcome: {err}");
    let (status, resolved) = vera
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/domains/eng/reflection/threads/{id}/resolve"),
            Some(json!({ "path": "guides/deploying.md", "outcome": "Yes." })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(resolved["status"], "resolved");
    let (_, listed) = vera
        .get("/api/v1/domains/eng/reflection/threads?status=all")
        .await;
    assert_eq!(listed["threads"].as_array().unwrap().len(), 1);

    let stranger_status = {
        s.auth
            .add_user("out", "out", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let out = Session::login(s.addr, "out").await;
        out.get("/api/v1/domains/eng/reflection").await.0
    };
    assert_eq!(
        stranger_status, 404,
        "a stranger learns nothing about a private domain"
    );

    // A member editor whose write list leaves `eng` out reads its
    // Reflection, and a write answers 403 naming the list and its fix.
    s.auth
        .set_write_list("ed", &["agents-*".to_string()])
        .await
        .unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let (_, status) = ed.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(status["right"], "read", "{status}");
    let (status, hits) = ed
        .get("/api/v1/domains/eng/reflection/search?q=night")
        .await;
    assert_eq!(status, 200, "an off-list reader searches: {hits}");
    assert_eq!(hits["hits"][0]["path"], "guides/deploying.md", "{hits}");
    let (status, err) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/new.md",
            Some(json!({ "markdown": "# New\n" })),
        )
        .await;
    assert_eq!(status, 403, "{err}");
    assert_eq!(
        err["detail"],
        "'eng' is not on the write list of 'ed'; an instance admin changes it with crystalline users writes"
    );
}

#[tokio::test]
async fn status_tree_doc_move_delete_search_and_review() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let (_, status) = ed.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(status["state"], "on");
    assert_eq!(
        status["member"], true,
        "ed holds a membership row on the private eng"
    );
    assert_eq!(status["home"], "local");
    assert_eq!(status["right"], "curate");
    let (_, tree) = ed.get("/api/v1/domains/eng/reflection/tree").await;
    assert_eq!(tree["nodes"][0]["kind"], "folder");
    assert_eq!(
        tree["nodes"][0]["children"][0]["path"],
        "guides/deploying.md"
    );
    let (_, doc) = ed
        .get("/api/v1/domains/eng/reflections/guides/deploying.md")
        .await;
    assert_eq!(
        doc["sections"][0]["sources"][0]["permalink"],
        "runbooks/deploy"
    );
    let markdown = doc["markdown"].as_str().unwrap();
    assert!(
        markdown.contains("engram: runbooks/deploy"),
        "an editor gets the whole file"
    );
    let line = doc["sections"][0]["line"].as_u64().unwrap() as usize;
    assert_eq!(
        markdown.lines().nth(line - 1),
        Some("## Night deploys"),
        "{doc}"
    );
    let checksum = doc["checksum"].as_str().unwrap().to_string();
    let (status, stale) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/guides/deploying.md",
            Some(json!({ "markdown": "# X\n", "expected_checksum": "0000" })),
        )
        .await;
    assert_eq!(status, 409, "{stale}");
    let (status, _) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/guides/deploying.md",
            Some(json!({ "markdown": doc["markdown"], "expected_checksum": checksum })),
        )
        .await;
    assert_eq!(status, 200);
    let (status, moved) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move",
            Some(json!({ "from": "guides/deploying.md", "to": "ops/deploying.md" })),
        )
        .await;
    assert_eq!(status, 200, "{moved}");
    let (_, hits) = ed
        .get("/api/v1/domains/eng/reflection/search?q=night")
        .await;
    assert_eq!(hits["hits"][0]["path"], "ops/deploying.md");
    let (status, review) = ed.get("/api/v1/domains/eng/reflection/review").await;
    assert_eq!(status, 200);
    assert!(
        review["waiting_for_people"].is_array()
            && review["waiting_for_the_agent"].is_array()
            && review["queue"]["queue"].is_array()
    );
    let (status, _) = ed
        .send(
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/reflections/ops/deploying.md",
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, err) = ed
        .send(
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/reflections/PACT.md",
            None,
        )
        .await;
    assert_eq!(status, 422, "{err}");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("instead of deleting it"),
        "refused as the pact, not for some other reason: {err}"
    );
}

#[tokio::test]
async fn a_non_home_instance_answers_with_the_home_url() {
    let s = serve().await;
    let manifest_text = manifest("reflection: on\nreflection_home: https://team.example.com\n");
    std::fs::write(s.fx.root.join("MANIFEST.md"), manifest_text).unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let (status, body) = ed.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(status, 200);
    assert_eq!(body["state"], "elsewhere");
    assert_eq!(body["home_url"], "https://team.example.com");
    let (status, err) = ed.get("/api/v1/domains/eng/reflection/tree").await;
    assert_eq!(status, 409);
    assert_eq!(err["home_url"], "https://team.example.com");
    assert_eq!(
        err["detail"],
        "this domain's Reflection lives at https://team.example.com"
    );
}

#[tokio::test]
async fn a_contributor_can_leave_a_domain() {
    let s = serve().await;
    let ann = Session::login(s.addr, "ann").await;
    // Leaving reveals nobody else: the listing and an eviction stay the 404
    // of a domain it cannot read as engrams.
    let (status, _) = ann.get("/api/v1/domains/eng/members").await;
    assert_eq!(status, 404, "a contributor does not list the members");
    let (status, _) = ann
        .send(
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/members/vera",
            None,
        )
        .await;
    assert_eq!(status, 404, "nor learns whether another account is one");
    let (status, _) = ann
        .send(
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/members/nobody",
            None,
        )
        .await;
    assert_eq!(status, 404);
    let (status, body) = ann
        .send(
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/members/ann",
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");
    let (status, _) = ann.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(status, 404, "gone for good");
}

#[tokio::test]
async fn the_member_routes_take_both_levels() {
    let s = serve().await;
    let keeper = Session::login(s.addr, "keeper").await;
    s.auth
        .add_user("new", "new", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    let (status, err) = keeper
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/members/new",
            Some(json!({ "level": "none" })),
        )
        .await;
    assert_eq!(
        status, 422,
        "knowledge none needs a Reflection level: {err}"
    );
    let (status, _) = keeper
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/members/new",
            Some(json!({ "level": "none", "reflection": "contributor" })),
        )
        .await;
    assert_eq!(status, 204);
    let (_, members) = keeper.get("/api/v1/domains/eng/members").await;
    let new = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["principal"] == "new")
        .unwrap();
    assert_eq!(
        (new["level"].as_str(), new["reflection"].as_str()),
        (Some("none"), Some("contributor"))
    );
    let vera = members["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["principal"] == "vera")
        .unwrap();
    assert_eq!(vera["reflection"], "default");
}

/// PACT.md is written by curators; a contributor and a reader propose a
/// change through a thread on it.
#[tokio::test]
async fn pact_is_written_by_curators_and_discussed_by_everyone() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let (status, made) = ed
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/PACT.md",
            Some(json!({ "markdown": "# How we write\n\n## Audience\n\nNew people.\n" })),
        )
        .await;
    assert_eq!(
        status, 200,
        "a knowledge editor curates and writes the pact: {made}"
    );
    let ann = Session::login(s.addr, "ann").await;
    let (status, err) = ann
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/PACT.md",
            Some(json!({ "markdown": "# Mine\n" })),
        )
        .await;
    assert_eq!(status, 403, "{err}");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("is edit, and curate is needed"),
        "{err}"
    );
    let vera = Session::login(s.addr, "vera").await;
    for who in [&ann, &vera] {
        let (status, thread) = who
            .send(reqwest::Method::POST, "/api/v1/domains/eng/reflection/threads", Some(json!({ "path": "PACT.md", "section": "Audience", "text": "Support staff too?" })))
            .await;
        assert_eq!(status, 201, "a proposal through a comment: {thread}");
    }
}

/// Knowledge `none` on the instance: no shared domain as engrams, every
/// shared Reflection at the instance Reflection level.
#[tokio::test]
async fn knowledge_none_on_the_instance_finds_its_shared_domain_through_the_reflection() {
    let s = serve_with(true).await;
    let nora = Session::login(s.addr, "nora").await;
    let (status, me) = nora.get("/api/v1/auth/me").await;
    assert_eq!(status, 200, "the instance gate lets it in: {me}");
    assert!(
        me.to_string().contains("\"none\"") && me.to_string().contains("\"contributor\""),
        "{me}"
    );
    let (_, domains) = nora.get("/api/v1/domains").await;
    assert!(
        !domains.to_string().contains("\"eng\""),
        "hidden as engrams: {domains}"
    );
    let (status, _) = nora
        .get("/api/v1/domains/eng/engrams/runbooks/deploy")
        .await;
    assert_eq!(status, 404);
    let (status, listing) = nora.get("/api/v1/reflection/domains").await;
    assert_eq!(status, 200);
    assert_eq!(listing["domains"][0]["domain"], "eng");
    assert_eq!(listing["domains"][0]["right"], "edit");
    assert_eq!(listing["domains"][0]["engrams_visible"], false);
    let (_, status) = nora.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(
        status["member"], false,
        "nothing to leave on a shared domain"
    );
    let (status, doc) = nora
        .get("/api/v1/domains/eng/reflections/guides/deploying.md")
        .await;
    assert_eq!(status, 200);
    assert!(
        !doc.to_string().contains("runbooks/deploy"),
        "no source engram: {doc}"
    );
}

/// Every instance-wide read the API serves, as an account at knowledge
/// `none`: none of them names the shared domain. The guard admits such an
/// account on every path, so this is what catches a read that forgets the
/// `hidden_for` screen.
#[tokio::test]
async fn no_instance_wide_read_names_a_shared_domain_to_knowledge_none() {
    let s = serve_with(true).await;
    let nora = Session::login(s.addr, "nora").await;
    for op in crate::support::MOUNTED_OPERATIONS {
        let Some(path) = op.strip_prefix("GET /api/v1") else {
            continue;
        };
        // Per-domain routes are covered above; the Reflection listing names
        // eng on purpose; the event stream never ends a body.
        // `/openapi.json` describes routes, and its examples may spell a
        // domain; it names no registered domain of this instance.
        if path.contains('{')
            || path.starts_with("/reflection")
            || path.starts_with("/auth")
            || path == "/events"
            || path == "/openapi.json"
        {
            continue;
        }
        let (_, body) = nora.get(&format!("/api/v1{path}")).await;
        assert!(
            !body.to_string().contains("\"eng\""),
            "GET {path} names eng to knowledge none: {body}"
        );
    }
}

/// The anonymous visitor of a public domain reads its Reflection, sources
/// included (it reads the engrams too), and cannot comment.
#[tokio::test]
async fn an_anonymous_visitor_reads_a_public_reflection_and_cannot_comment() {
    let s = serve_with(true).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = |path: &str| format!("http://{}{path}", s.addr);
    let doc: Value = client
        .get(url("/api/v1/domains/eng/reflections/guides/deploying.md"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        doc["sections"][0]["sources"][0]["permalink"], "runbooks/deploy",
        "{doc}"
    );
    let listing: Value = client
        .get(url("/api/v1/reflection/domains"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listing["domains"][0]["right"], "read", "{listing}");
    let resp = client
        .post(url("/api/v1/domains/eng/reflection/threads"))
        .json(
            &json!({ "path": "guides/deploying.md", "section": "Night deploys", "text": "Hello?" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "a comment needs somebody to sign it"
    );
    let err: Value = resp.json().await.unwrap();
    assert_eq!(err["detail"], ANONYMOUS_COMMENT, "{err}");
    // The other thread writes say the same; the page writes say why too.
    for (method, path, body, detail) in [
        (
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/threads/t-00000000/answer",
            Some(json!({ "path": "guides/deploying.md", "text": "Hello?" })),
            ANONYMOUS_COMMENT,
        ),
        (
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflections/guides/deploying.md",
            Some(json!({ "markdown": "# Mine\n" })),
            ANONYMOUS_EDIT,
        ),
        (
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/reflections/guides/deploying.md",
            None,
            ANONYMOUS_EDIT,
        ),
        (
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move",
            Some(json!({ "from": "guides/deploying.md", "to": "x/deploying.md" })),
            ANONYMOUS_EDIT,
        ),
    ] {
        let (status, err) = anonymous(s.addr, method.clone(), path, body).await;
        assert_eq!(status, 401, "{method} {path}: {err}");
        assert_eq!(err["detail"], detail, "{method} {path}");
    }
    assert!(
        s.fx.root.join("reflections/guides/deploying.md").exists(),
        "the page is untouched"
    );
    assert!(
        !s.fx
            .root
            .join("reflections/guides/deploying.threads.md")
            .exists(),
        "no thread was written"
    );
}

/// The contract's sentences for the anonymous viewer's writes.
const ANONYMOUS_COMMENT: &str =
    "anonymous visitors read this Reflection and cannot comment; sign in to comment";
const ANONYMOUS_EDIT: &str =
    "anonymous visitors read this Reflection and cannot change it; sign in to edit";

/// A request with no session at all: the anonymous viewer, where it is on.
pub async fn anonymous(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut req = client.request(method, format!("http://{addr}{path}"));
    if let Some(body) = body {
        req = req.json(&body);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// Every per-domain reflection operation the router mounts, as a method and
/// a concrete path on `domain`, with a body that passes extraction.
fn reflection_requests(domain: &str) -> Vec<(reqwest::Method, String, Option<Value>)> {
    let body = json!({
        "markdown": "# A\n", "path": "guides/deploying.md", "section": "Night deploys",
        "text": "Why?", "outcome": "Yes.", "from": "guides/deploying.md", "to": "x/deploying.md",
        "basis": "b1",
    });
    let ops: Vec<_> = crate::support::MOUNTED_OPERATIONS
        .iter()
        .filter(|op| op.contains("{domain}/reflection"))
        .map(|op| {
            let (method, template) = op.split_once(' ').unwrap();
            let mut path = template
                .replace("{domain}", domain)
                .replace("{path}", "guides/deploying.md")
                .replace("{id}", "t-00000000")
                .replace("{action}", "answer");
            if path.ends_with("/reflection/search") {
                path.push_str("?q=night");
            }
            let method: reqwest::Method = method.parse().unwrap();
            let body = (method == reqwest::Method::POST || method == reqwest::Method::PUT)
                .then(|| body.clone());
            (method, path, body)
        })
        .collect();
    assert_eq!(
        ops.len(),
        19,
        "every per-domain reflection operation: {ops:?}"
    );
    ops
}

/// Every reflection route answers `send`'s caller on `eng` exactly as on a
/// name nobody registered: 404 with the same words.
async fn every_route_hides_eng<F, Fut>(send: F)
where
    F: Fn(reqwest::Method, String, Option<Value>) -> Fut,
    Fut: std::future::Future<Output = (u16, Value)>,
{
    let ghosts = reflection_requests("ghost");
    for ((method, path, body), (_, ghost_path, ghost_body)) in
        reflection_requests("eng").into_iter().zip(ghosts)
    {
        let (status, err) = send(method.clone(), path.clone(), body).await;
        let (ghost_status, ghost_err) = send(method.clone(), ghost_path, ghost_body).await;
        assert_eq!(ghost_status, 404, "{method} on ghost: {ghost_err}");
        assert_eq!(status, 404, "{method} {path}: {err}");
        assert_eq!(
            err["detail"].as_str().unwrap(),
            ghost_err["detail"]
                .as_str()
                .unwrap()
                .replace("ghost", "eng"),
            "{method} {path} answers like an unregistered name"
        );
    }
}

/// A signed-in stranger learns nothing about a private domain on any
/// reflection route, reads and writes alike.
#[tokio::test]
async fn a_stranger_meets_an_unregistered_name_on_every_route() {
    let s = serve().await;
    s.auth
        .add_user("out", "out", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let out = Session::login(s.addr, "out").await;
    every_route_hides_eng(|method, path, body| {
        let out = &out;
        async move { out.send(method, &path, body).await }
    })
    .await;
}

/// The anonymous viewer of an instance with a private domain learns nothing
/// about it either: a write is the same 404, not a 401 that would say the
/// name is worth signing in for.
#[tokio::test]
async fn the_anonymous_viewer_meets_an_unregistered_name_on_a_private_domain() {
    let s = serve_opts(Opts {
        anonymous: true,
        private: true,
        origin: false,
    })
    .await;
    let addr = s.addr;
    every_route_hides_eng(|method, path, body| async move {
        anonymous(addr, method, &path, body).await
    })
    .await;
}

/// Engrams waiting in reflections/ hold the Reflection back: 409, naming
/// them to a reader of engrams and to nobody else.
#[tokio::test]
async fn a_pending_reflection_answers_409_and_names_engrams_only_to_their_readers() {
    let s = serve().await;
    // Indexed as an engram while the switch is off, then switched on by
    // hand, the way a pulled MANIFEST would switch it.
    std::fs::write(s.fx.root.join("MANIFEST.md"), manifest("")).unwrap();
    std::fs::write(
        s.fx.root.join("reflections/old.md"),
        engram("Old", "reflections/old", "- [fact] old"),
    )
    .unwrap();
    s.fx.engine.sync(None).await.unwrap();
    std::fs::write(s.fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let (_, status) = ed.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(status["state"], "pending", "{status}");
    let (status, err) = ed.get("/api/v1/domains/eng/reflection/tree").await;
    assert_eq!(status, 409, "{err}");
    assert_eq!(
        err["detail"],
        "Reflection for 'eng' waits until these engrams leave reflections/: reflections/old.md",
        "{err}"
    );
    let ann = Session::login(s.addr, "ann").await;
    let (status, err) = ann.get("/api/v1/domains/eng/reflection/tree").await;
    assert_eq!(status, 409, "{err}");
    assert_eq!(
        err["detail"], "Reflection for 'eng' waits until the engrams in reflections/ are moved out",
        "no engram named to a contributor: {err}"
    );
}

/// A GitHub-connected domain whose Reflection has no home answers 409 with
/// the way out.
#[tokio::test]
async fn a_homeless_reflection_answers_409() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: true,
        origin: true,
    })
    .await;
    let ed = Session::login(s.addr, "ed").await;
    let (_, status) = ed.get("/api/v1/domains/eng/reflection").await;
    assert_eq!(status["state"], "homeless", "{status}");
    let (status, err) = ed.get("/api/v1/domains/eng/reflection/tree").await;
    assert_eq!(status, 409, "{err}");
    assert_eq!(
        err["detail"],
        "this domain is connected to GitHub and its Reflection has no home yet: switch Reflection on in the domain policies at the team server"
    );
    assert!(err.get("home_url").is_none(), "{err}");
}

/// A switch-off on a domain connected to GitHub says in the PATCH answer
/// what it means for sharing; a PATCH that switches nothing off says nothing.
#[tokio::test]
async fn a_switch_off_on_a_team_domain_answers_with_the_sharing_note() {
    let s = serve_opts(Opts {
        anonymous: false,
        private: true,
        origin: true,
    })
    .await;
    let keeper = Session::login(s.addr, "keeper").await;
    let off = || Some(json!({ "reflection": "off" }));
    let (status, body) = keeper
        .send(
            reqwest::Method::PATCH,
            "/api/v1/domains/eng/manifest",
            off(),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body["note"]
            .as_str()
            .is_some_and(|note| note.contains("\"share changes\" offers the files in reflections/")),
        "{body}"
    );
    let (status, body) = keeper
        .send(
            reqwest::Method::PATCH,
            "/api/v1/domains/eng/manifest",
            off(),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body.get("note").is_none(), "already off: {body}");
}

/// The two handler-side refusals: an action the route does not know, and a
/// search with nothing to find.
#[tokio::test]
async fn an_unknown_thread_action_is_404_and_a_search_needs_words() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let (status, err) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/threads/t-00000000/close",
            Some(json!({ "path": "guides/deploying.md", "text": "x" })),
        )
        .await;
    assert_eq!(status, 404, "{err}");
    assert_eq!(
        err["detail"],
        "no thread action 'close'; use answer, resolve or reopen"
    );
    for query in ["", "?q=", "?q=%20%20"] {
        let (status, err) = ed
            .get(&format!("/api/v1/domains/eng/reflection/search{query}"))
            .await;
        assert_eq!(status, 422, "search{query}: {err}");
        assert_eq!(err["detail"], "search needs `q`, the words to find");
    }
}

impl Session {
    pub async fn put_bytes(&self, path: &str, bytes: &[u8]) -> (u16, Value) {
        self.put_bytes_with(path, bytes, &[]).await
    }

    /// [`Session::put_bytes`] with extra request headers.
    pub async fn put_bytes_with(
        &self,
        path: &str,
        bytes: &[u8],
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut request = client
            .put(format!("http://{}{path}", self.addr))
            .header("cookie", format!("fluid_session={}", self.cookie))
            .header("x-csrf-token", &self.csrf)
            .header("content-type", "application/octet-stream");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let resp = request.body(bytes.to_vec()).send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    pub async fn get_raw(&self, path: &str) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let resp = client
            .get(format!("http://{}{path}", self.addr))
            .header("cookie", format!("fluid_session={}", self.cookie))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        (status, headers, resp.bytes().await.unwrap().to_vec())
    }
}

/// A Reflection's files over REST: a reader reads, a contributor at
/// knowledge `none` adds, a curator replaces, moves and removes, with the
/// engram attachment headers on every read.
#[tokio::test]
async fn the_file_routes_follow_the_reflection_rights() {
    let s = serve().await;
    let (ann, vera, ed) = (
        Session::login(s.addr, "ann").await,
        Session::login(s.addr, "vera").await,
        Session::login(s.addr, "ed").await,
    );
    const FILE: &str = "/api/v1/domains/eng/reflection/assets/diagrams/flow.png";
    let png = crate::assets::PNG;
    let (status, _) = vera.put_bytes(FILE, png).await;
    assert_eq!(status, 403, "a reader does not add");
    let (status, up) = ann.put_bytes(FILE, png).await;
    assert_eq!(status, 200, "a contributor at knowledge none adds: {up}");
    assert_eq!(up["path"], "assets/diagrams/flow.png");
    assert_eq!(up["created"], true);
    let (status, headers, bytes) = vera.get_raw(FILE).await;
    assert_eq!((status, bytes.as_slice()), (200, png));
    // A person at knowledge none reads the image of a Reflection it reads.
    let (status, _, bytes) = ann.get_raw(FILE).await;
    assert_eq!(
        (status, bytes.as_slice()),
        (200, png),
        "knowledge none reads the file"
    );
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    assert_eq!(headers["cache-control"], "no-cache");
    let etag = headers["etag"].to_str().unwrap().to_string();
    assert_eq!(etag, format!("\"{}\"", up["sha256"].as_str().unwrap()));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let revalidated = client
        .get(format!("http://{}{FILE}", s.addr))
        .header("cookie", format!("fluid_session={}", vera.cookie))
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(revalidated.status().as_u16(), 304, "a matching ETag");
    let (_, listing) = ann.get("/api/v1/domains/eng/reflection/assets").await;
    assert_eq!(
        listing["assets"][0]["path"], "assets/diagrams/flow.png",
        "{listing}"
    );
    let (status, err) = ann.put_bytes(FILE, b"other").await;
    assert_eq!(status, 403, "replacing needs curate: {err}");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .contains("upload it under a new name instead"),
        "{err}"
    );
    let (status, _) = ann.send(reqwest::Method::DELETE, FILE, None).await;
    assert_eq!(status, 403, "removing needs curate");
    let (status, bad) = ed
        .put_bytes("/api/v1/domains/eng/reflection/assets/tool.exe", png)
        .await;
    assert_eq!(status, 422, "{bad}");
    let (status, moved) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move-asset",
            Some(json!({ "from": "assets/diagrams/flow.png", "to": "assets/flow.png" })),
        )
        .await;
    assert_eq!(status, 200, "{moved}");
    assert_eq!(moved["to"], "assets/flow.png", "{moved}");
    assert_eq!(moved["links_rewritten"], 0, "{moved}");
    let (status, up) = ed
        .put_bytes("/api/v1/domains/eng/reflection/assets/other.png", png)
        .await;
    assert_eq!(status, 200, "the setup upload landed: {up}");
    let (status, _) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move-asset",
            Some(json!({ "from": "assets/flow.png", "to": "assets/other.png" })),
        )
        .await;
    assert_eq!(status, 409, "a move never overwrites");
    let (status, _) = ed
        .send(
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/reflection/assets/flow.png",
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, _, _) = vera
        .get_raw("/api/v1/domains/eng/reflection/assets/flow.png")
        .await;
    assert_eq!(status, 404);
    // `other.png` exists under reflections/assets/ (uploaded above): the
    // engram file route must not serve it under either spelling (the probe
    // names a file that is really there).
    for probe in [
        "/api/v1/domains/eng/files/assets/other.png",
        "/api/v1/domains/eng/files/reflections/assets/other.png",
    ] {
        let (status, _, _) = ed.get_raw(probe).await;
        assert_ne!(
            status, 200,
            "a Reflection's file is no engram attachment: {probe}"
        );
    }
}

/// The anonymous viewer of a public domain reads a Reflection's files and
/// changes none: each file write is 401 with the edit sentence.
#[tokio::test]
async fn an_anonymous_visitor_reads_the_files_and_changes_none() {
    let s = serve_with(true).await;
    s.fx.engine
        .reflection_asset_write(
            "eng",
            "assets/flow.png",
            crate::assets::PNG.to_vec(),
            &crystalline_service::ReflectionCaller::owner(),
        )
        .await
        .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!(
            "http://{}/api/v1/domains/eng/reflection/assets/flow.png",
            s.addr
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), crate::assets::PNG);
    for (method, path, body) in [
        (
            reqwest::Method::PUT,
            "/api/v1/domains/eng/reflection/assets/new.png",
            None,
        ),
        (
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/reflection/assets/flow.png",
            None,
        ),
        (
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move-asset",
            Some(json!({ "from": "assets/flow.png", "to": "assets/moved.png" })),
        ),
    ] {
        let (status, err) = anonymous(s.addr, method.clone(), path, body).await;
        assert_eq!(status, 401, "{method} {path}: {err}");
        assert_eq!(err["detail"], ANONYMOUS_EDIT, "{method} {path}");
    }
    assert!(s.fx.root.join("reflections/assets/flow.png").exists());
    assert!(!s.fx.root.join("reflections/assets/new.png").exists());
}

/// The file writes meet the write list (R18) as the page writes do: an
/// off-list editor reads the files and every write answers 403 naming the
/// list. A move waits while a page that links the file is open in a room:
/// 409, and nothing moves.
#[tokio::test]
async fn the_file_writes_meet_the_write_list_and_open_rooms() {
    let s = serve().await;
    let owner = crystalline_service::ReflectionCaller::owner();
    s.fx.engine
        .reflection_asset_write(
            "eng",
            "assets/flow.png",
            crate::assets::PNG.to_vec(),
            &owner,
        )
        .await
        .unwrap();
    s.fx.engine
        .write_reflection(
            &crate::docs::write("links.md", "# Links\n\n![flow](assets/flow.png)\n"),
            &owner,
        )
        .await
        .unwrap();

    // A room over the page that links the file, opened the way Fluid opens
    // it: the co-editing socket.
    let ed = Session::login(s.addr, "ed").await;
    let _room = open_room(s.addr, "/api/v1/collab-reflection/eng/links.md", &ed).await;
    let (status, err) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move-asset",
            Some(json!({ "from": "assets/flow.png", "to": "assets/moved.png" })),
        )
        .await;
    assert_eq!(status, 409, "{err}");
    assert!(
        err["detail"]
            .as_str()
            .unwrap()
            .starts_with("somebody is editing 'links.md'"),
        "{err}"
    );
    assert!(s.fx.root.join("reflections/assets/flow.png").is_file());

    // Off the write list: reads stay, writes are 403 with the list's words.
    s.auth
        .set_write_list("ed", &["agents-*".to_string()])
        .await
        .unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let (status, _, bytes) = ed
        .get_raw("/api/v1/domains/eng/reflection/assets/flow.png")
        .await;
    assert_eq!((status, bytes.as_slice()), (200, crate::assets::PNG));
    const OFF_LIST: &str = "'eng' is not on the write list of 'ed'; an instance admin changes it with crystalline users writes";
    let (status, err) = ed
        .put_bytes(
            "/api/v1/domains/eng/reflection/assets/new.png",
            crate::assets::PNG,
        )
        .await;
    assert_eq!(
        (status, err["detail"].as_str()),
        (403, Some(OFF_LIST)),
        "{err}"
    );
    for (method, path, body) in [
        (
            reqwest::Method::DELETE,
            "/api/v1/domains/eng/reflection/assets/flow.png",
            None,
        ),
        (
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move-asset",
            Some(json!({ "from": "assets/flow.png", "to": "assets/moved.png" })),
        ),
    ] {
        let (status, err) = ed.send(method.clone(), path, body).await;
        assert_eq!(status, 403, "{method} {path}: {err}");
        assert_eq!(err["detail"], OFF_LIST, "{method} {path}");
    }
    assert!(!s.fx.root.join("reflections/assets/new.png").exists());
    assert!(s.fx.root.join("reflections/assets/flow.png").is_file());
}

/// Open the co-editing socket at `path` as `session`, with a same-host
/// Origin, and keep it open for as long as the answer is held.
async fn open_room(
    addr: std::net::SocketAddr,
    path: &str,
    session: &Session,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::handshake::client::{Request, generate_key};
    let request = Request::builder()
        .uri(format!("ws://{addr}{path}"))
        .header("Host", addr.to_string())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Origin", format!("http://{addr}"))
        .header("Cookie", format!("fluid_session={}", session.cookie))
        .body(())
        .unwrap();
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// The page route answers `live.present` while a room is open over it, and
/// no `live` member otherwise, for a reader with or without knowledge.
#[tokio::test]
async fn the_page_route_answers_live_while_a_room_is_open() {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use yrs::updates::encoder::Encode;
    let s = serve().await;
    let owner = crystalline_service::ReflectionCaller::owner();
    s.fx.engine
        .write_reflection(&crate::docs::write("live.md", "# Live\n\nBody.\n"), &owner)
        .await
        .unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let ann = Session::login(s.addr, "ann").await;
    let (status, closed) = ed.get("/api/v1/domains/eng/reflections/live.md").await;
    assert_eq!(status, 200, "{closed}");
    assert!(closed.get("live").is_none(), "no room, no member: {closed}");

    let mut room = open_room(s.addr, "/api/v1/collab-reflection/eng/live.md", &ed).await;
    let mut awareness = yrs::sync::Awareness::new(yrs::Doc::new());
    awareness.set_local_state_raw("{\"user\":{\"name\":\"Ed\"}}".to_string());
    let frame = yrs::sync::Message::Awareness(awareness.update().unwrap()).encode_v1();
    room.send(WsMessage::Binary(frame.into())).await.unwrap();

    for who in [&ed, &ann] {
        let mut present = Value::Null;
        for _ in 0..50 {
            let (status, open) = who.get("/api/v1/domains/eng/reflections/live.md").await;
            assert_eq!(status, 200, "{open}");
            present = open["live"]["present"].clone();
            if present.as_array().is_some_and(|a| !a.is_empty()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(present, json!(["Ed"]));
    }
}

#[tokio::test]
async fn the_search_route_takes_search_type_and_page_and_answers_the_mcp_shape() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let (status, found) = ed
        .get("/api/v1/domains/eng/reflection/search?q=night&search_type=text&limit=1&page=1")
        .await;
    assert_eq!(status, 200, "{found}");
    assert_eq!(found["mode"], "text", "{found}");
    assert_eq!(found["domain"], "eng");
    assert_eq!(
        (
            found["page"].as_u64(),
            found["limit"].as_u64(),
            found["count"].as_u64()
        ),
        (Some(1), Some(1), Some(1)),
        "{found}"
    );
    assert!(found["total"].as_u64().unwrap() >= 1, "{found}");
    let hit = &found["hits"][0];
    for key in ["path", "title", "section", "heading", "snippet"] {
        assert!(hit.get(key).is_some(), "{key}: {hit}");
    }
    assert!(hit.get("score").is_none(), "no score on REST: {hit}");
    let (status, refused) = ed
        .get("/api/v1/domains/eng/reflection/search?q=night&search_type=title")
        .await;
    assert_eq!(status, 422, "{refused}");
    assert!(
        refused["detail"]
            .as_str()
            .unwrap()
            .contains("hybrid, text or semantic"),
        "{refused}"
    );
}

/// The change stream as `cookie`, or as the anonymous viewer.
async fn events_as(addr: std::net::SocketAddr, cookie: Option<&str>) -> reqwest::Response {
    let mut req = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{addr}/api/v1/events"))
        .header("accept", "text/event-stream");
    if let Some(cookie) = cookie {
        req = req.header("cookie", format!("fluid_session={cookie}"));
    }
    req.send().await.unwrap()
}

/// The `event:` names a stream sends within `within`; heartbeat comments
/// carry none.
async fn event_names(resp: &mut reqwest::Response, within: std::time::Duration) -> Vec<String> {
    let mut body = String::new();
    let deadline = tokio::time::Instant::now() + within;
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(deadline, resp.chunk()).await {
        body.push_str(std::str::from_utf8(&chunk).unwrap());
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("event:"))
        .map(|name| name.trim().to_string())
        .collect()
}

/// An engram edit and a reflection write, both in `eng`.
async fn one_of_each(s: &Served) {
    s.fx.engine
        .edit_engram(&crystalline_service::params::EditParams {
            identifier: "runbooks/deploy".into(),
            domain: "eng".into(),
            operation: "append".into(),
            content: Some("- [fact] one more line".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    s.fx.engine
        .write_reflection(
            &crate::docs::write("guides/other.md", "# Other\n"),
            &crystalline_service::ReflectionCaller::owner(),
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contributor_hears_reflection_events_and_no_engram_event() {
    let s = serve().await;
    let ann = Session::login(s.addr, "ann").await;
    s.auth
        .add_user("out", "out", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let out = Session::login(s.addr, "out").await;
    let mut contributor = events_as(s.addr, Some(&ann.cookie)).await;
    let mut stranger = events_as(s.addr, Some(&out.cookie)).await;
    one_of_each(&s).await;
    let heard = event_names(&mut contributor, std::time::Duration::from_secs(3)).await;
    assert_eq!(
        heard,
        vec!["reflection"],
        "the page, never the engram: {heard:?}"
    );
    let heard = event_names(&mut stranger, std::time::Duration::from_secs(3)).await;
    assert!(heard.is_empty(), "a stranger hears neither: {heard:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_anonymous_viewer_of_a_public_domain_hears_both() {
    let s = serve_with(true).await;
    let mut anonymous = events_as(s.addr, None).await;
    one_of_each(&s).await;
    let mut heard = event_names(&mut anonymous, std::time::Duration::from_secs(3)).await;
    heard.sort();
    assert_eq!(
        heard,
        vec!["engram", "reflection"],
        "it reads the engrams and the Reflection: {heard:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contributor_hears_the_switch_go_off() {
    let s = serve().await;
    let ann = Session::login(s.addr, "ann").await;
    let mut contributor = events_as(s.addr, Some(&ann.cookie)).await;
    s.fx.engine
        .set_manifest_policies(
            "eng",
            &[("reflection".to_string(), "off".to_string())],
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = event_names(&mut contributor, std::time::Duration::from_secs(3)).await;
    assert_eq!(
        heard,
        vec!["reflection"],
        "the switch, never the MANIFEST's engram event: {heard:?}"
    );
}

/// The domain listing counts each Reflection's open threads from its threads
/// files alone: twenty listings read no page text and no engram row past
/// the gate's probe.
#[tokio::test]
async fn the_domain_listing_counts_open_threads_without_page_text() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    let mut ids = Vec::new();
    for text in ["Still?", "Who runs them?", "Since when?"] {
        let (status, thread) = ed
            .send(
                reqwest::Method::POST,
                "/api/v1/domains/eng/reflection/threads",
                Some(json!({ "path": "guides/deploying.md", "section": "Night deploys", "text": text })),
            )
            .await;
        assert_eq!(status, 201, "{thread}");
        ids.push(thread["id"].as_str().unwrap().to_string());
    }
    let (status, _) = ed
        .send(
            reqwest::Method::POST,
            &format!("/api/v1/domains/eng/reflection/threads/{}/resolve", ids[0]),
            Some(json!({ "path": "guides/deploying.md", "outcome": "Yes." })),
        )
        .await;
    assert_eq!(status, 200);
    let texts = s.fx.engine.reflection_text_loads();
    let paths = s.fx.engine.recorded_path_loads();
    for _ in 0..20 {
        let (status, listing) = ed.get("/api/v1/reflection/domains").await;
        assert_eq!(status, 200);
        assert_eq!(listing["domains"][0]["domain"], "eng");
        assert_eq!(listing["domains"][0]["open_threads"], 2, "{listing}");
    }
    assert_eq!(
        s.fx.engine.reflection_text_loads(),
        texts,
        "no page text is read to count threads"
    );
    assert_eq!(s.fx.engine.recorded_path_loads(), paths);
}

/// `If-None-Match: *` makes an upload create-only: the first lands, the
/// second is 412 with the bytes unchanged, for a contributor and a curator
/// alike (a contributor is told 412, not the 403 of a refused replace).
#[tokio::test]
async fn a_create_only_upload_refuses_an_existing_file_with_412() {
    let s = serve().await;
    let (ann, ed) = (
        Session::login(s.addr, "ann").await,
        Session::login(s.addr, "ed").await,
    );
    const FILE: &str = "/api/v1/domains/eng/reflection/assets/flow.png";
    let png = crate::assets::PNG;
    let star = [("if-none-match", "*")];
    let (status, up) = ann.put_bytes_with(FILE, png, &star).await;
    assert_eq!(status, 200, "{up}");
    assert_eq!(up["created"], true, "{up}");
    for who in [&ann, &ed] {
        let (status, err) = who.put_bytes_with(FILE, b"other", &star).await;
        assert_eq!(status, 412, "{err}");
        assert_eq!(err["status"], 412, "{err}");
        assert_eq!(
            err["detail"],
            format!(
                "'assets/flow.png' {}",
                crystalline_service::engine::ASSET_EXISTS
            ),
            "{err}"
        );
    }
    // A space around the star is still the star.
    let (status, err) = ed
        .put_bytes_with(FILE, b"other", &[("if-none-match", " * ")])
        .await;
    assert_eq!(status, 412, "{err}");
    assert_eq!(
        std::fs::read(s.fx.root.join("reflections/assets/flow.png")).unwrap(),
        png,
        "nothing replaced the first upload"
    );
    // Without the header a curator still replaces.
    let (status, up) = ed.put_bytes(FILE, b"other").await;
    assert_eq!(status, 200, "{up}");
    assert_eq!(up["created"], false, "{up}");
}

/// Only `*` asks to create: any other `If-None-Match` is 422 and stores
/// nothing.
#[tokio::test]
async fn an_if_none_match_other_than_star_is_422() {
    let s = serve().await;
    let ed = Session::login(s.addr, "ed").await;
    const FILE: &str = "/api/v1/domains/eng/reflection/assets/flow.png";
    for value in ["\"abc\"", "W/*", "*, \"abc\"", "\"*\"", ""] {
        let (status, err) = ed
            .put_bytes_with(FILE, crate::assets::PNG, &[("if-none-match", value)])
            .await;
        assert_eq!(status, 422, "{value:?}: {err}");
        assert_eq!(
            err["detail"], "If-None-Match on an upload takes only *",
            "{value:?}: {err}"
        );
    }
    assert!(!s.fx.root.join("reflections/assets/flow.png").exists());
}

/// A page moved over the JSON API while somebody has it open in the editor:
/// the move is not refused, the open socket is told `closed` with reason
/// `moved` and the new path, and the socket then closes for good (4404).
#[tokio::test]
async fn a_move_over_the_api_tells_the_open_editor_where_the_page_went() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use yrs::updates::decoder::DecoderV1;
    let s = serve().await;
    let owner = crystalline_service::ReflectionCaller::owner();
    s.fx.engine
        .write_reflection(&crate::docs::write("open.md", "# Open\n\nBody.\n"), &owner)
        .await
        .unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let mut room = open_room(s.addr, "/api/v1/collab-reflection/eng/open.md", &ed).await;
    let (status, moved) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move",
            Some(json!({ "from": "open.md", "to": "guides/open.md" })),
        )
        .await;
    assert_eq!(status, 200, "{moved}");
    let mut closed = None;
    let mut code = None;
    while let Some(frame) = tokio::time::timeout(std::time::Duration::from_secs(10), room.next())
        .await
        .expect("the socket hears the close in time")
    {
        match frame.unwrap() {
            WsMessage::Binary(bytes) => {
                let mut decoder = DecoderV1::from(bytes.as_ref());
                for message in yrs::sync::MessageReader::new(&mut decoder).flatten() {
                    if let yrs::sync::Message::Custom(4, payload) = message {
                        let control: Value = serde_json::from_slice(&payload).unwrap();
                        if control["kind"] == "closed" {
                            closed = Some(control);
                        }
                    }
                }
            }
            WsMessage::Close(frame) => {
                code = frame.map(|frame| u16::from(frame.code));
                break;
            }
            _ => {}
        }
    }
    assert_eq!(
        closed,
        Some(json!({ "kind": "closed", "reason": "moved", "path": "guides/open.md" }))
    );
    assert_eq!(
        code,
        Some(4404),
        "a close the editor does not reconnect after"
    );
    let (status, page) = ed
        .get("/api/v1/domains/eng/reflections/guides/open.md")
        .await;
    assert_eq!(status, 200, "{page}");
}

/// An engram no page cites is an `unreflected` item that names its folder and
/// its engrams. Only a reader of engrams is told of it at all.
#[tokio::test]
async fn a_contributor_hears_nothing_of_unreflected_engrams_in_the_queue() {
    let fx = fixture_with(
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            (
                "notes/loose.md",
                &engram("Loose", "notes/loose", "- [fact] nobody cites this"),
            ),
        ],
        |_, _| {},
    )
    .await;
    let s = serve_fx(fx, true).await;
    let queue_path = "/api/v1/domains/eng/reflection/queue";
    let unreflected = |queue: &Value| {
        queue["queue"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["what"] == "unreflected")
            .count()
    };
    let no_folder = |queue: &Value| {
        queue["queue"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("folder").is_none() && r.get("engrams").is_none())
    };
    let ed = Session::login(s.addr, "ed").await;
    let (status, queue) = ed.get(queue_path).await;
    assert_eq!(status, 200, "{queue}");
    assert_eq!(unreflected(&queue), 1, "{queue}");
    assert_eq!(queue["queue"][0]["folder"], "notes", "{queue}");
    let ann = Session::login(s.addr, "ann").await;
    let (status, queue) = ann.get(queue_path).await;
    assert_eq!(status, 200, "{queue}");
    assert_eq!(unreflected(&queue), 0, "{queue}");
    assert!(no_folder(&queue), "{queue}");
    assert!(
        !queue.to_string().contains("notes/loose"),
        "no engram name: {queue}"
    );
    let (status, review) = ann.get("/api/v1/domains/eng/reflection/review").await;
    assert_eq!(status, 200, "{review}");
    assert_eq!(unreflected(&review["queue"]), 0, "{review}");
    assert!(no_folder(&review["queue"]), "{review}");
}

/// An editor that opens the old path after the page moved (its first try was
/// refused while the move ran) is upgraded and told where the page went, the
/// same `moved` close an open editor got, then the socket closes for good.
#[tokio::test]
async fn an_editor_opening_the_old_path_after_a_move_is_told_where_it_went() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use yrs::updates::decoder::DecoderV1;
    let s = serve().await;
    let owner = crystalline_service::ReflectionCaller::owner();
    s.fx.engine
        .write_reflection(&crate::docs::write("late.md", "# Late\n\nBody.\n"), &owner)
        .await
        .unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let (status, moved) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move",
            Some(json!({ "from": "late.md", "to": "guides/late.md" })),
        )
        .await;
    assert_eq!(status, 200, "{moved}");
    let mut room = open_room(s.addr, "/api/v1/collab-reflection/eng/late.md", &ed).await;
    let mut closed = None;
    let mut code = None;
    while let Some(frame) = tokio::time::timeout(std::time::Duration::from_secs(10), room.next())
        .await
        .expect("the socket answers in time")
    {
        match frame.unwrap() {
            WsMessage::Binary(bytes) => {
                let mut decoder = DecoderV1::from(bytes.as_ref());
                for message in yrs::sync::MessageReader::new(&mut decoder).flatten() {
                    if let yrs::sync::Message::Custom(4, payload) = message {
                        closed = Some(serde_json::from_slice::<Value>(&payload).unwrap());
                    }
                }
            }
            WsMessage::Close(frame) => {
                code = frame.map(|frame| u16::from(frame.code));
                break;
            }
            _ => {}
        }
    }
    assert_eq!(
        closed,
        Some(json!({ "kind": "closed", "reason": "moved", "path": "guides/late.md" }))
    );
    assert_eq!(code, Some(4404));
}

/// A page move onto a path that holds a page is refused with 409 before the
/// open editor is touched: the socket hears no close and stays open.
#[tokio::test]
async fn a_move_onto_a_taken_page_is_a_409_and_leaves_the_editor_open() {
    use futures::StreamExt;
    let s = serve().await;
    let owner = crystalline_service::ReflectionCaller::owner();
    for (path, body) in [
        ("open.md", "# Open\n\nBody.\n"),
        ("taken.md", "# Taken\n\nTheirs.\n"),
    ] {
        s.fx.engine
            .write_reflection(&crate::docs::write(path, body), &owner)
            .await
            .unwrap();
    }
    let ed = Session::login(s.addr, "ed").await;
    let mut room = open_room(s.addr, "/api/v1/collab-reflection/eng/open.md", &ed).await;
    let (status, refused) = ed
        .send(
            reqwest::Method::POST,
            "/api/v1/domains/eng/reflection/move",
            Some(json!({ "from": "open.md", "to": "taken.md" })),
        )
        .await;
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["detail"]
            .as_str()
            .unwrap()
            .starts_with("'taken.md' already holds a reflection"),
        "{refused}"
    );
    // Nothing that closes arrives: only the greeting and the sync.
    let quiet = tokio::time::timeout(std::time::Duration::from_millis(300), async {
        while let Some(frame) = room.next().await {
            if let tokio_tungstenite::tungstenite::Message::Close(_) = frame.unwrap() {
                return true;
            }
        }
        true
    })
    .await;
    assert!(quiet.is_err(), "the socket stays open");
}

/// What the Reflection home derives from the review, one reader at a time:
/// the three counts and the open threads by page (path, title, thread ids,
/// first quotes, who each waits on).
fn home_from_review(review: &Value) -> Value {
    let mut pages: std::collections::BTreeMap<String, (Value, Vec<Value>)> =
        std::collections::BTreeMap::new();
    let mut count = |side: &str| {
        let mut n = 0;
        for group in review[side].as_array().unwrap() {
            let entry = pages
                .entry(group["path"].as_str().unwrap().to_string())
                .or_insert_with(|| (group["title"].clone(), Vec::new()));
            for t in group["threads"].as_array().unwrap() {
                n += 1;
                entry
                    .1
                    .push(json!([t["id"], t["anchor"]["quote"], t["waiting_on"]]));
            }
        }
        n
    };
    let (agent, people) = (count("waiting_for_the_agent"), count("waiting_for_people"));
    let mut update_pages: Vec<&str> = review["queue"]["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "update")
        .filter_map(|row| row["path"].as_str())
        .filter(|path| !path.is_empty())
        .collect();
    update_pages.sort_unstable();
    update_pages.dedup();
    json!({
        "agent": agent,
        "people": people,
        "updates": review["queue"]["kinds"]["update"],
        "update_pages": update_pages.len(),
        "pages": pages.into_iter().map(|(path, (title, mut threads))| {
            threads.sort_by_key(|t| t[0].as_str().unwrap().to_string());
            json!([path, title, threads])
        }).collect::<Vec<_>>(),
    })
}

/// Whether the review's first queue page (50 rows) can hold every update,
/// so the distinct pages read off it are all of them.
fn review_queue_fits(summary: &Value) -> bool {
    summary["updates"].as_u64().unwrap() <= 50
}

/// The same, read off the summary route.
fn home_from_summary(summary: &Value) -> Value {
    let mut pages: Vec<Value> = summary["open_by_page"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            let mut threads: Vec<Value> = g["threads"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| json!([t["id"], t["quote"], t["waiting_on"]]))
                .collect();
            threads.sort_by_key(|t| t[0].as_str().unwrap().to_string());
            json!([g["path"], g["title"], threads])
        })
        .collect();
    pages.sort_by_key(|p| p[0].as_str().unwrap().to_string());
    json!({
        "agent": summary["waiting_for_the_agent"],
        "people": summary["waiting_for_people"],
        "updates": summary["updates"],
        "update_pages": summary["pages_to_update"],
        "pages": pages,
    })
}

/// The home's summary says what the review says, for a reader of engrams and
/// for a contributor who reads none (who is not counted the unreflected
/// engram), reads the threads files once, carries no thread entries, and is
/// refused to who may not read the Reflection.
#[tokio::test]
async fn the_reflection_summary_agrees_with_the_review_for_every_reader() {
    let s = serve().await;
    std::fs::create_dir_all(s.fx.root.join("notes")).unwrap();
    std::fs::write(
        s.fx.root.join("notes/loose.md"),
        engram("Loose", "notes/loose", "- [fact] nobody cites this"),
    )
    .unwrap();
    s.fx.engine.sync(None).await.unwrap();
    let mut p = crate::docs::write("ops/second.md", "# Second\n\n## Pager\n\nWho holds it.\n");
    p.sources = std::collections::BTreeMap::new();
    s.fx.engine
        .write_reflection(&p, &crystalline_service::ReflectionCaller::owner())
        .await
        .unwrap();
    let ed = Session::login(s.addr, "ed").await;
    let ann = Session::login(s.addr, "ann").await;
    let mut ids = Vec::new();
    for (who, path, section, quote) in [
        (
            &ed,
            "guides/deploying.md",
            "Night deploys",
            Some("at night"),
        ),
        (&ann, "guides/deploying.md", "Night deploys", None),
        (&ann, "ops/second.md", "Pager", Some("Who holds it")),
    ] {
        let (status, thread) = who
            .send(
                reqwest::Method::POST,
                "/api/v1/domains/eng/reflection/threads",
                Some(json!({ "path": path, "section": section, "quote": quote, "text": "Still true?" })),
            )
            .await;
        assert_eq!(status, 201, "{thread}");
        ids.push(thread["id"].as_str().unwrap().to_string());
    }
    // The agent answers the first: it now waits for people.
    s.fx.engine
        .discuss(
            &crystalline_service::params::DiscussParams {
                domain: "eng".into(),
                action: Some("answer".into()),
                path: Some("guides/deploying.md".into()),
                thread: Some(ids[0].clone()),
                text: Some("Yes.".into()),
                ..Default::default()
            },
            &crate::docs::agent(),
        )
        .await
        .unwrap();

    let summary_path = "/api/v1/domains/eng/reflection/summary";
    let (status, ed_summary) = ed.get(summary_path).await;
    assert_eq!(status, 200, "{ed_summary}");
    assert_eq!(ed_summary["domain"], "eng");
    assert_eq!(ed_summary["waiting_for_people"], 1, "{ed_summary}");
    assert_eq!(ed_summary["waiting_for_the_agent"], 2, "{ed_summary}");
    let first = &ed_summary["open_by_page"][0];
    assert_eq!(
        first["path"], "guides/deploying.md",
        "oldest page first: {ed_summary}"
    );
    assert_eq!(first["title"], "Deploying");
    assert_eq!(
        first["threads"][0]["id"],
        ids[0].as_str(),
        "oldest thread first"
    );
    assert_eq!(first["threads"][0]["quote"], "at night");
    assert_eq!(first["threads"][0]["waiting_on"], "people");
    assert_eq!(
        first["threads"][0]["people"],
        json!([{ "name": "ed", "agent": false }, { "name": "claude-code", "agent": true }]),
        "{ed_summary}"
    );
    assert!(first["threads"][0].get("entries").is_none(), "no entries");

    for (who, name) in [(&ed, "ed"), (&ann, "ann")] {
        let (status, review) = who.get("/api/v1/domains/eng/reflection/review").await;
        assert_eq!(status, 200, "{review}");
        let before = s.fx.engine.reflection_text_loads();
        let (status, summary) = who.get(summary_path).await;
        assert_eq!(status, 200, "{summary}");
        assert_eq!(
            s.fx.engine.reflection_text_loads() - before,
            1,
            "the threads are read once for {name}"
        );
        assert_eq!(
            home_from_summary(&summary),
            home_from_review(&review),
            "{name}: {summary} vs {review}"
        );
    }
    let (_, ann_summary) = ann.get(summary_path).await;
    assert!(
        review_queue_fits(&ed_summary),
        "the review's first page holds every update row: {ed_summary}"
    );
    assert_eq!(
        ed_summary["pages_to_update"], ann_summary["pages_to_update"],
        "unreflected engrams are on no page"
    );
    assert_eq!(
        ed_summary["updates"].as_u64().unwrap(),
        ann_summary["updates"].as_u64().unwrap() + 1,
        "the unreflected engram is counted for a reader of engrams only: {ed_summary} {ann_summary}"
    );
    assert!(!ann_summary.to_string().contains("notes"), "{ann_summary}");

    let nora = Session::login(s.addr, "nora").await;
    let (status, err) = nora.get(summary_path).await;
    assert_eq!(status, 404, "{err}");
    let (status, err) = ed.get("/api/v1/domains/nope/reflection/summary").await;
    assert_eq!(status, 404, "{err}");
}

/// Over REST, a source in a domain the reader may not open reaches them as
/// a count only, on the page, in the queue and on the review screen; a
/// reader of that domain sees it in full. A save of the page as read keeps
/// the source.
#[tokio::test]
async fn a_source_in_another_domain_reaches_only_its_readers() {
    let fx = crate::sources::crossing().await;
    let mut p = crate::docs::write(
        "overview.md",
        "# Overview\n\nFor people.\n\n## Team\n\nFive people.\n",
    );
    p.sources = std::collections::BTreeMap::from([(
        "Team".to_string(),
        vec!["crystalline://harbor/team/harbor-team".to_string()],
    )]);
    fx.engine
        .write_reflection(&p, &crystalline_service::ReflectionCaller::owner())
        .await
        .unwrap();
    std::fs::write(
        crate::fixture::root_of(&fx, "harbor").join("team/harbor-team.md"),
        engram("Harbor team", "team/harbor-team", "- [fact] six people"),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let s = serve_fx(fx, false).await;
    s.auth
        .set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    let (ed, keeper) = (
        Session::login(s.addr, "ed").await,
        Session::login(s.addr, "keeper").await,
    );
    let page = "/api/v1/domains/eng/reflections/overview.md";
    for path in [
        page,
        "/api/v1/domains/eng/reflection/queue",
        "/api/v1/domains/eng/reflection/review",
    ] {
        let (status, body) = ed.get(path).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert!(!body.to_string().contains("harbor"), "{path}: {body}");
        let (status, body) = keeper.get(path).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert!(body.to_string().contains("harbor"), "{path}: {body}");
    }
    let (_, doc) = ed.get(page).await;
    let team = &doc["sections"][0];
    assert_eq!(team["heading"], "Team", "{doc}");
    assert_eq!(team["hidden_sources"], 1, "{doc}");
    let (_, queue) = ed.get("/api/v1/domains/eng/reflection/queue").await;
    let item = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["what"] == "source_changed")
        .unwrap_or_else(|| panic!("{queue}"));
    assert_eq!(item["hidden_sources"], 1, "{queue}");

    let (status, saved) = ed
        .send(
            reqwest::Method::PUT,
            page,
            Some(json!({
                "markdown": doc["markdown"].as_str().unwrap().replace("Five", "Six"),
                "expected_checksum": doc["checksum"],
            })),
        )
        .await;
    assert_eq!(status, 200, "{saved}");
    let stored = std::fs::read_to_string(s.fx.root.join("reflections/overview.md")).unwrap();
    assert!(stored.contains("domain: harbor"), "kept: {stored}");
    assert!(stored.contains("Six people"), "{stored}");
}
