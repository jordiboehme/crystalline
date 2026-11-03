//! A domain whose GitHub repository is archived is read-only for everybody:
//! a GitHub writer, an instance admin and the machine owner. Every write
//! path refuses with one sentence, and reads, search and a pull keep
//! working. An `archived` delivery marks the domain at once; only GitHub's
//! own answer clears the mark again.
//!
//! The instance here serves the whole HTTP router, so the MCP tools and the
//! JSON API are both reached the way a client reaches them. The deliveries
//! go straight to `Engine::apply_github_event`; the signed route is covered
//! in `github_webhook`.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use crystalline_core::config::{
    AuthConfig, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::daemon::http_router;
use crystalline_service::params::{WriteParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, EnvOverlay, ReflectionCaller, Scope};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::github_app::{Forge, app_env, connect, engram, files, manifest, spawn_forge};
use super::github_governed::{Mcp, PASSWORD, client, login, with_session};
use crate::support::MockProvider;

const KB: &str = "kb";
const KB_REPO: &str = "acme/kb";
const VAULT: &str = "vault";

/// GitHub id of `gwrite`, who has write on both repositories.
const GWRITE_ID: u64 = 103;

/// The sentence every write into `kb` meets while `acme/kb` is archived.
const SENTENCE: &str = "'kb' is read-only because its GitHub repository acme/kb is archived; reading still works, and the domain can change again once the repository is unarchived on GitHub";

struct Fixture {
    addr: std::net::SocketAddr,
    eng: Arc<Engine>,
    auth: Arc<AuthStore>,
    forge: Arc<Forge>,
    _tmp: tempfile::TempDir,
}

/// A repository with a MANIFEST that switches the Reflection on, a seed
/// engram whose body says `quokka` and the engram `row-gwrite`.
fn seeded_repo(domain: &str) -> Arc<MockProvider> {
    let mock = Arc::new(MockProvider::new());
    let head = mock.add_commit(files(&[
        (
            "MANIFEST.md",
            manifest("reflection: on\nreflection_home: https://kb.example.com\n"),
        ),
        (
            "seed.md",
            engram("Seed", "seed", &format!("The quokka lives in {domain}.")),
        ),
        (
            "row-gwrite.md",
            engram("Row gwrite", "row-gwrite", "A row's own engram."),
        ),
        (
            "mover.md",
            engram("Mover", "mover", "An engram that moves."),
        ),
    ]));
    mock.set_branch("main", &head);
    mock
}

/// An instance with the App, `kb` on `acme/kb` and `vault` on
/// `acme/vault`, the accounts `root` (instance admin) and `gwrite`
/// (instance editor, GitHub write), and the HTTP router served on
/// loopback. `kb` carries a Reflection page `seed.md` and a Reflection file
/// `assets/pic.svg`, written by the machine owner.
async fn instance() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (kb, vault) = (seeded_repo(KB), seeded_repo(VAULT));
    let (forge, url) = spawn_forge(
        &[(KB_REPO, kb), ("acme/vault", vault)],
        &[(KB_REPO, 1), ("acme/vault", 1)],
        &[(1, "ghs_test")],
    )
    .await;
    for repo in [KB_REPO, "acme/vault"] {
        forge.collaborators.lock().unwrap().insert(
            repo.to_string(),
            vec![json!({"id": GWRITE_ID, "login": "gwrite-gh", "role_name": "write"})],
        );
    }
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            api_url: Some(url.clone()),
            ..GitHubConfig::default()
        }),
        service: Some(ServiceConfig {
            public_url: Some("https://kb.example.com".into()),
            response_format: Some(ResponseFormat::Json),
            ..Default::default()
        }),
        auth: Some(AuthConfig {
            mcp: Some(true),
            oauth: Some(false),
            anonymous: Some(false),
            ..AuthConfig::default()
        }),
        ..GlobalConfig::default()
    };
    let overlay = EnvOverlay::from_vars(
        app_env()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let eng = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(config_path),
        )
        .with_origins_dir(root.join("origins"))
        .with_state_dir(root.to_path_buf())
        .with_token_store_dir(root.join("tokens"))
        .with_env_overlay(overlay),
    );
    connect(&eng, KB_REPO, KB, &root.join(KB)).await;
    connect(&eng, "acme/vault", VAULT, &root.join(VAULT)).await;
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&root.join("auth.db")).await.unwrap());
    auth.add_user("root", "root", None, Role::Admin, PASSWORD)
        .await
        .unwrap();
    auth.add_user("gwrite", "gwrite", None, Role::Editor, PASSWORD)
        .await
        .unwrap();
    auth.link_github("gwrite", &host, GWRITE_ID, "gwrite-gh")
        .await
        .unwrap();
    for (path, action, markdown, content) in [
        (
            "seed.md",
            None,
            Some("# Seed\n\n## Wombat\n\nThe wombat page.\n"),
            None,
        ),
        (
            "assets/pic.svg",
            Some("write_asset"),
            None,
            Some("<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
        ),
    ] {
        eng.write_reflection(
            &WriteReflectionParams {
                domain: KB.to_string(),
                path: path.to_string(),
                action: action.map(str::to_string),
                markdown: markdown.map(str::to_string),
                content: content.map(str::to_string),
                ..Default::default()
            },
            &ReflectionCaller::owner(),
        )
        .await
        .unwrap();
    }
    let router = http_router(
        eng.clone(),
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
        eng,
        auth,
        forge,
        _tmp: tmp,
    }
}

/// A `repository` event with `action` for `repo`.
fn repository_event(action: &str, repo: &str) -> Value {
    json!({
        "action": action,
        "repository": {
            "full_name": repo,
            "name": repo.split('/').nth(1).unwrap(),
            "archived": action == "archived",
        },
        "installation": {"id": 1},
    })
}

/// Polls `done` for up to five seconds; answers whether it came true.
async fn eventually(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..250 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    done()
}

/// Archives `acme/kb` on the forge, then delivers `archived`: the domain is
/// read-only at once, and GitHub's answer to the lookup that follows keeps
/// it so.
async fn archive(fx: &Fixture) {
    fx.forge.archive(KB_REPO);
    let outcome = fx
        .eng
        .apply_github_event("repository", &repository_event("archived", KB_REPO))
        .await
        .unwrap();
    assert!(!outcome.readers_changed);
    assert_eq!(fx.eng.github_archived_repo(KB).as_deref(), Some(KB_REPO));
}

/// An MCP session as `account`'s own token.
async fn mcp_as(fx: &Fixture, account: &str) -> Mcp {
    let issued = fx.auth.issue_mcp_token(account, "agent").await.unwrap();
    Mcp::open(fx.addr, Some(issued.token)).await.unwrap()
}

/// `write_engram` arguments for a new engram `title` in `domain`.
fn new_engram(domain: &str, title: &str) -> Value {
    json!({"domain": domain, "title": title, "content": "- [fact] something new"})
}

/// The machine owner's `write_engram`.
fn owner_write(title: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: KB.to_string(),
        title: title.to_string(),
        content: Some("- [fact] the owner writes".to_string()),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
    }
}

/// One request as `session`; answers the status and the body as JSON
/// (`null` for an empty or non-JSON one).
async fn send(
    fx: &Fixture,
    session: &(String, String),
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = with_session(
        client().request(method, format!("http://{}{path}", fx.addr)),
        Some(session),
    );
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Saves `row-gwrite` in `kb` over the JSON API as `session`.
async fn rest_save(fx: &Fixture, session: &(String, String)) -> (u16, Value) {
    let url = format!("http://{}/api/v1/domains/{KB}/engrams/row-gwrite", fx.addr);
    let detail = with_session(client().get(&url), Some(session))
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), 200, "the engram reads");
    let etag = detail.headers()["etag"].to_str().unwrap().to_string();
    let text = engram("Row gwrite", "row-gwrite", "Saved through the JSON API.");
    let put = with_session(client().put(&url), Some(session))
        .header("if-match", etag)
        .json(&json!({ "content": String::from_utf8(text).unwrap() }))
        .send()
        .await
        .unwrap();
    let status = put.status().as_u16();
    let text = put.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// The refusal text of `answer`, or the answer itself when it is none.
fn refused(answer: Result<String, String>) -> String {
    match answer {
        Err(text) => text,
        Ok(text) => format!("not refused: {text}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_archived_repository_refuses_a_writer_an_admin_and_the_machine_owner_and_keeps_reads() {
    let fx = instance().await;
    let writer = mcp_as(&fx, "gwrite").await;
    let admin = mcp_as(&fx, "root").await;
    // Before: both write.
    writer
        .call("write_engram", new_engram(KB, "Before archive"))
        .await
        .unwrap();
    archive(&fx).await;

    assert!(
        refused(
            writer
                .call("write_engram", new_engram(KB, "Writer note"))
                .await
        )
        .contains(SENTENCE),
        "the GitHub writer is refused"
    );
    assert!(
        refused(
            admin
                .call("write_engram", new_engram(KB, "Admin note"))
                .await
        )
        .contains(SENTENCE),
        "the instance admin is refused"
    );
    let owner = fx.eng.write_engram(&owner_write("Owner note")).await;
    assert_eq!(
        owner.map(|_| ()).unwrap_err().to_string(),
        SENTENCE,
        "the machine owner is refused"
    );
    // Another domain is not touched.
    writer
        .call("write_engram", new_engram(VAULT, "Vault note"))
        .await
        .unwrap();

    // Reads, search and a pull keep working.
    let read = writer
        .call("read_engram", json!({"identifier": "seed", "domain": KB}))
        .await
        .unwrap();
    assert!(read.contains("quokka"), "{read}");
    let found = writer
        .call(
            "search_engrams",
            json!({"query": "quokka", "domains": [KB], "search_type": "text"}),
        )
        .await
        .unwrap();
    assert!(found.contains("seed"), "{found}");
    fx.eng
        .origin_update(Some(KB), &Scope::Unrestricted)
        .await
        .expect("the pull works on an archived domain");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_write_path_refuses_on_an_archived_domain() {
    let fx = instance().await;
    archive(&fx).await;
    let writer = mcp_as(&fx, "gwrite").await;
    let session = login(fx.addr, "gwrite").await.unwrap();
    let root = login(fx.addr, "root").await.unwrap();
    let mut wrong = Vec::new();
    let mut expect = |what: &str, text: String| {
        if !text.contains(SENTENCE) {
            wrong.push(format!("{what}: {text}"));
        }
    };

    // MCP.
    for (what, tool, args) in [
        (
            "write_engram into assets/",
            "write_engram",
            json!({"domain": KB, "path": "assets/note.txt", "content": "a note"}),
        ),
        (
            "move into the domain",
            "move_engram",
            json!({
                "identifier": "mover",
                "domain": VAULT,
                "destination": "mover",
                "destination_domain": KB,
            }),
        ),
        (
            "Reflection page write",
            "write_reflection",
            json!({"domain": KB, "path": "new.md", "markdown": "# New\n\n## Part\n\nText.\n"}),
        ),
        (
            "Reflection write_asset",
            "write_reflection",
            json!({
                "domain": KB,
                "action": "write_asset",
                "path": "assets/new.svg",
                "content": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            }),
        ),
        (
            "Reflection move_asset",
            "write_reflection",
            json!({
                "domain": KB,
                "action": "move_asset",
                "path": "assets/pic.svg",
                "to": "assets/moved.svg",
            }),
        ),
        (
            "Reflection delete_asset",
            "write_reflection",
            json!({"domain": KB, "action": "delete_asset", "path": "assets/pic.svg"}),
        ),
        ("share_changes", "share_changes", json!({"domain": KB})),
    ] {
        expect(what, refused(writer.call(tool, args).await));
    }

    // The JSON API.
    let (status, body) = rest_save(&fx, &session).await;
    expect(&format!("REST save ({status})"), body.to_string());
    let put = with_session(
        client().put(format!(
            "http://{}/api/v1/domains/{KB}/files/assets/upload.txt",
            fx.addr
        )),
        Some(&session),
    )
    .body("bytes")
    .send()
    .await
    .unwrap();
    let status = put.status().as_u16();
    expect(
        &format!("attachment upload ({status})"),
        put.text().await.unwrap_or_default(),
    );
    let (status, body) = send(
        &fx,
        &session,
        reqwest::Method::POST,
        &format!("/api/v1/domains/{KB}/reflection/threads"),
        Some(json!({ "path": "seed.md", "section": "## Wombat", "text": "question: why?" })),
    )
    .await;
    expect(&format!("thread comment ({status})"), body.to_string());
    for (who, s) in [("gwrite", &session), ("root", &root)] {
        let (status, body) = send(
            &fx,
            s,
            reqwest::Method::POST,
            &format!("/api/v1/domains/{KB}/sync/share"),
            Some(json!({})),
        )
        .await;
        // A refusal is exactly 403: the caller sees the domain.
        let text = if status == 403 {
            body.to_string()
        } else {
            format!("answered {status}: {body}")
        };
        expect(&format!("REST share as {who}"), text);
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    // Nothing went out.
    assert!(fx.forge.commits().is_empty(), "{:?}", fx.forge.commits());
    // And the Reflection file is where it was.
    let pic = fx._tmp.path().join(KB).join("reflections/assets/pic.svg");
    assert!(pic.exists(), "{}", pic.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listing_and_the_members_answer_say_an_archived_domain_is_read_only() {
    let fx = instance().await;
    archive(&fx).await;
    for name in ["gwrite", "root"] {
        let session = login(fx.addr, name).await.unwrap();
        let (status, listing) =
            send(&fx, &session, reqwest::Method::GET, "/api/v1/domains", None).await;
        assert_eq!(status, 200, "{listing}");
        let row = |d: &str| {
            listing["domains"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["name"] == d)
                .unwrap()
                .clone()
        };
        assert_eq!(row(KB)["read_only"], true, "{name}: {listing}");
        assert_eq!(row(KB)["may_share"], false, "{name}: {listing}");
        assert_ne!(row(VAULT)["read_only"], true, "{name}: {listing}");
        assert_eq!(row(VAULT)["may_share"], true, "{name}: {listing}");
        let (status, members) = send(
            &fx,
            &session,
            reqwest::Method::GET,
            &format!("/api/v1/domains/{KB}/members"),
            None,
        )
        .await;
        assert_eq!(status, 200, "{members}");
        assert_eq!(members["archived_reason"], SENTENCE, "{name}: {members}");
        let (_, members) = send(
            &fx,
            &session,
            reqwest::Method::GET,
            &format!("/api/v1/domains/{VAULT}/members"),
            None,
        )
        .await;
        assert_eq!(members["archived_reason"], Value::Null, "{members}");
    }
    let block = fx
        .eng
        .write_block(&Scope::Unrestricted, KB)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(block.sentence(KB), SENTENCE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_unarchived_writes_work_again_once_github_says_so() {
    let fx = instance().await;
    archive(&fx).await;
    let writer = mcp_as(&fx, "gwrite").await;
    // The payload alone widens nothing: GitHub still says archived.
    fx.eng
        .apply_github_event("repository", &repository_event("unarchived", KB_REPO))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fx.eng.github_archived_repo(KB).as_deref(), Some(KB_REPO));
    assert!(
        refused(
            writer
                .call("write_engram", new_engram(KB, "Still archived"))
                .await
        )
        .contains(SENTENCE)
    );
    // GitHub unarchives it, and the delivery asks again.
    fx.forge.unarchive(KB_REPO);
    fx.eng
        .apply_github_event("repository", &repository_event("unarchived", KB_REPO))
        .await
        .unwrap();
    assert!(
        eventually(|| fx.eng.github_archived_repo(KB).is_none()).await,
        "GitHub's answer clears the mark"
    );
    writer
        .call("write_engram", new_engram(KB, "Writable again"))
        .await
        .unwrap();
    fx.eng
        .write_engram(&owner_write("Owner again"))
        .await
        .unwrap();
}

/// A signed `archived` body can be sent again with a fresh delivery id
/// long after the repository was unarchived. It narrows at once, and the
/// lookup it starts clears the mark without anybody asking.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replayed_archived_body_for_a_live_repository_is_cleared_by_the_warm_up_lookup() {
    let fx = instance().await;
    fx.eng
        .apply_github_event("repository", &repository_event("archived", KB_REPO))
        .await
        .unwrap();
    let marked = fx.eng.github_archived_repo(KB);
    assert_eq!(marked.as_deref(), Some(KB_REPO), "it narrows at once");
    assert!(
        eventually(|| fx.eng.github_archived_repo(KB).is_none()).await,
        "the warm-up lookup clears the mark"
    );
    let writer = mcp_as(&fx, "gwrite").await;
    writer
        .call("write_engram", new_engram(KB, "Live again"))
        .await
        .unwrap();
}

/// An `archived` delivery for a repository no domain is connected to does
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_archived_delivery_for_an_unconnected_repository_marks_nothing() {
    let fx = instance().await;
    fx.eng
        .apply_github_event("repository", &repository_event("archived", "acme/other"))
        .await
        .unwrap();
    assert_eq!(fx.eng.github_archived_repo(KB), None);
    assert_eq!(fx.eng.github_archived_repo(VAULT), None);
}

/// Without an App nothing is governed: a domain connected through the
/// instance token writes as it always did, even when its repository is
/// archived on GitHub, and nobody asks GitHub whether it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_an_app_an_archived_repository_writes_as_today_and_is_never_looked_up() {
    let tmp = tempfile::tempdir().unwrap();
    let (forge, url) = spawn_forge(&[(KB_REPO, seeded_repo(KB))], &[], &[]).await;
    let env = [(
        "CRYSTALLINE_GITHUB_TOKEN",
        super::github_app::INSTANCE_TOKEN,
    )];
    let eng = Arc::new(super::github_app::engine(tmp.path(), &url, &env).await);
    connect(&eng, KB_REPO, KB, &tmp.path().join(KB)).await;
    let auth = Arc::new(AuthStore::open(&tmp.path().join("auth.db")).await.unwrap());
    eng.set_domain_access(Arc::new(
        crystalline_service::DomainAccess::new(auth).with_governor(eng.knowledge_governor()),
    ));
    forge.archive(KB_REPO);
    let lookups = || {
        forge
            .hits()
            .iter()
            .filter(|h| {
                h.path == format!("/repos/{KB_REPO}") || h.path.starts_with("/repositories/")
            })
            .count()
    };
    let before = lookups();
    eng.write_engram(&owner_write("Owner note")).await.unwrap();
    assert!(!eng.github_governs(KB));
    assert_eq!(eng.github_archived_repo(KB), None);
    assert!(
        !eng.read_only_mark(&Scope::Unrestricted, KB).await,
        "the domain is not marked read-only"
    );
    assert_eq!(lookups(), before, "{:?}", forge.hits());
}
