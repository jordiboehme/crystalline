//! Access on domains GitHub governs, every row through every surface.
//!
//! `github_governed_matrix` drives each kind of caller (a GitHub admin,
//! maintainer, writer, triager and reader, a linked account GitHub does not
//! list, unlinked accounts, an instance `none` account, a viewer with GitHub
//! write on and off its write list, a Reflection-only collaborator, an
//! account with the instance Reflection level contributor, an MCP token
//! agent, the MCP open tier acting as `github.agent_identity`, an instance
//! admin named as that identity, the anonymous visitor and an instance
//! admin) through the MCP tools `search_engrams`, `read_engram`,
//! `write_engram`, `read_reflection` and `write_reflection`, the JSON API's
//! `GET /domains` and engram `PUT`, the collab WebSocket upgrade and the
//! `/events` fan-out, on five governed domains: shared on a private
//! repository, shared on a public repository, private, and a shared and a
//! private domain whose repository lacks the App (closed, R22).
//!
//! The expected cells are written from the rulings (Decision 6, R16, R17,
//! R20, R22, R23), not read off the code. Every cell is collected first and
//! the whole table is compared once, so a failure names every wrong cell.
//!
//! It lives in the origins binary rather than `domains/` because the forge
//! that answers the collaborator listing, the installation and the token
//! routes is this binary's (`origins::github_app`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use crystalline_core::config::{
    AuthConfig, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::daemon::http_router;
use crystalline_service::engine::{ConfigureAction, Invalidate};
use crystalline_service::params::{EditParams, WriteParams, WriteReflectionParams};
use crystalline_service::rest::{AuthStore, ReflectionLevel, Role};
use crystalline_service::{Engine, EnvOverlay, ReflectionCaller};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::github_app::{Forge, app_env, connect, engram, files, manifest, spawn_forge};
use crate::support::MockProvider;

/// The five governed domains, each on its own repository `acme/<name>`.
const SHARED: &str = "shared";
const PUBLIC: &str = "public";
const PRIVATE: &str = "private";
const CLOSED_SHARED: &str = "cshared";
const CLOSED_PRIVATE: &str = "cprivate";
const DOMAINS: [&str; 5] = [SHARED, PUBLIC, PRIVATE, CLOSED_SHARED, CLOSED_PRIVATE];

/// A domain with no origin, which GitHub does not govern.
const PLAIN: &str = "plain";

const PASSWORD: &str = "pw12345678";

/// A GitHub link: the GitHub id, and the role GitHub lists it with (`None`:
/// linked, but not listed).
type Link = (u64, Option<&'static str>);

/// The accounts: (name, instance role, GitHub id and role when linked; a
/// linked account with no role is linked but not listed by GitHub).
const ACCOUNTS: &[(&str, Role, Option<Link>)] = &[
    ("root", Role::Admin, None),
    ("gadmin", Role::Editor, Some((101, Some("admin")))),
    ("gmaint", Role::Editor, Some((102, Some("maintain")))),
    ("gwrite", Role::Editor, Some((103, Some("write")))),
    ("gtriage", Role::Editor, Some((104, Some("triage")))),
    ("gread", Role::Editor, Some((105, Some("read")))),
    ("gnone", Role::Editor, Some((106, None))),
    ("unlinked", Role::Viewer, None),
    ("ueditor", Role::Editor, None),
    ("inone", Role::None, Some((107, Some("write")))),
    ("vwrite", Role::Viewer, Some((108, Some("write")))),
    ("voff", Role::Viewer, Some((109, Some("write")))),
    ("refl", Role::None, None),
    ("rcontrib", Role::Viewer, None),
    ("bot", Role::Viewer, Some((110, Some("write")))),
    ("rootgh", Role::Admin, Some((111, Some("read")))),
];

/// How a row reaches the instance.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Via {
    /// A person: a password session for the JSON API, the collab socket and
    /// `/events`, and their own MCP token for the MCP columns.
    Person(&'static str),
    /// An MCP token agent of the account: the MCP columns only.
    Token(&'static str),
    /// The MCP open tier (`auth.mcp` off) with `github.agent_identity` set
    /// to the account: the MCP columns only.
    OpenTier(&'static str),
    /// The anonymous visitor (`auth.anonymous` on) for the JSON API, the
    /// collab socket and `/events`, and the MCP open tier with no
    /// `github.agent_identity` for the MCP columns.
    Anonymous,
}

/// The columns, in the order the expected strings spell them.
const COLUMNS: [&str; 9] = [
    "search_engrams",
    "read_engram",
    "write_engram",
    "GET /domains",
    "engram PUT",
    "collab edit",
    "/events",
    "read_reflection",
    "write_reflection",
];

/// Shorthands for a row on one domain. `y` allowed, `n` refused, `-` not a
/// surface this row has.
const ALL: &str = "yyyyyyyyy";
const NOTHING: &str = "nnnnnnnnn";
/// Reads engrams and the Reflection, writes neither.
const READS: &str = "yynynnyyn";
/// Reads engrams, works the Reflection (instance contributor level).
const READS_EDITS_REFLECTION: &str = "yynynnyyy";
/// No engram, the Reflection read and edited (a Reflection grant).
const REFLECTION_ONLY: &str = "nnnnnnnyy";
const MCP_ALL: &str = "yyy----yy";
const MCP_READS: &str = "yyn----yn";
const MCP_NOTHING: &str = "nnn----nn";

struct Row {
    label: &'static str,
    via: Via,
    /// One string per domain of [`DOMAINS`], one character per column of
    /// [`COLUMNS`].
    expected: [&'static str; 5],
}

/// The table of expected cells, from Decision 6 and the rulings:
///
/// * a shared governed domain is read by every signed-in account at
///   instance viewer or above, linked or not; a private one only by linked
///   accounts with GitHub read or more;
/// * engram writes need a link and GitHub write or more; the instance role
///   neither raises nor caps them, the write list still restricts, instance
///   `none` gets nothing (R16);
/// * the Reflection follows its ladder (R17): engram write curates, engram
///   read reads, the instance Reflection level holds on a shared domain
///   only, a per-domain grant raises, also for instance `none` (R23), and
///   the write list caps it to read;
/// * a closed domain is the instance admins' and the machine owner's only,
///   grants included (R22);
/// * the open tier acts as `github.agent_identity`, clamped to editor even
///   for an instance admin (R20); without one it is anonymous;
/// * anonymous reads only a shared domain on a public repository.
fn rows() -> Vec<Row> {
    let writer = [ALL, ALL, ALL, NOTHING, NOTHING];
    let reader = [READS, READS, READS, NOTHING, NOTHING];
    let base_only = [READS, READS, NOTHING, NOTHING, NOTHING];
    let mcp_writer = [MCP_ALL, MCP_ALL, MCP_ALL, MCP_NOTHING, MCP_NOTHING];
    let mcp_reader = [MCP_READS, MCP_READS, MCP_READS, MCP_NOTHING, MCP_NOTHING];
    vec![
        Row {
            label: "instance admin",
            via: Via::Person("root"),
            expected: [ALL; 5],
        },
        Row {
            label: "GitHub admin",
            via: Via::Person("gadmin"),
            expected: writer,
        },
        Row {
            label: "GitHub maintain",
            via: Via::Person("gmaint"),
            expected: writer,
        },
        Row {
            label: "GitHub write",
            via: Via::Person("gwrite"),
            expected: writer,
        },
        Row {
            label: "GitHub triage",
            via: Via::Person("gtriage"),
            expected: reader,
        },
        Row {
            label: "GitHub read",
            via: Via::Person("gread"),
            expected: reader,
        },
        Row {
            label: "GitHub none (linked, not listed)",
            via: Via::Person("gnone"),
            expected: base_only,
        },
        Row {
            label: "unlinked viewer",
            via: Via::Person("unlinked"),
            expected: base_only,
        },
        Row {
            label: "unlinked editor",
            via: Via::Person("ueditor"),
            expected: base_only,
        },
        Row {
            label: "instance none with GitHub write",
            via: Via::Person("inone"),
            expected: [NOTHING; 5],
        },
        Row {
            label: "viewer with GitHub write",
            via: Via::Person("vwrite"),
            expected: writer,
        },
        Row {
            label: "viewer with GitHub write, off its write list",
            via: Via::Person("voff"),
            expected: reader,
        },
        Row {
            label: "Reflection-only collaborator (instance none, grant)",
            via: Via::Person("refl"),
            expected: [
                REFLECTION_ONLY,
                REFLECTION_ONLY,
                REFLECTION_ONLY,
                NOTHING,
                NOTHING,
            ],
        },
        Row {
            label: "unlinked, instance Reflection contributor",
            via: Via::Person("rcontrib"),
            expected: [
                READS_EDITS_REFLECTION,
                READS_EDITS_REFLECTION,
                NOTHING,
                NOTHING,
                NOTHING,
            ],
        },
        Row {
            label: "MCP token agent of a GitHub writer",
            via: Via::Token("gwrite"),
            expected: mcp_writer,
        },
        Row {
            label: "open tier as agent identity (GitHub write)",
            via: Via::OpenTier("bot"),
            expected: mcp_writer,
        },
        Row {
            label: "open tier as admin agent identity (GitHub read)",
            via: Via::OpenTier("rootgh"),
            expected: mcp_reader,
        },
        Row {
            label: "anonymous",
            via: Via::Anonymous,
            expected: [NOTHING, READS, NOTHING, NOTHING, NOTHING],
        },
    ]
}

// --- the instance ------------------------------------------------------------

struct Fixture {
    addr: std::net::SocketAddr,
    eng: Arc<Engine>,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

/// A repository with a MANIFEST that switches the Reflection on and one
/// seed engram, `seed-<domain>-end`, whose body says `quokka`.
fn seeded_repo(domain: &str) -> Arc<MockProvider> {
    let mock = Arc::new(MockProvider::new());
    let head = mock.add_commit(files(&[
        (
            "MANIFEST.md",
            manifest("reflection: on\nreflection_home: https://kb.example.com\n"),
        ),
        (
            &format!("seed-{domain}-end.md"),
            engram(
                &format!("Seed {domain} end"),
                &format!("seed-{domain}-end"),
                "The quokka lives here.",
            ),
        ),
    ]));
    mock.set_branch("main", &head);
    mock
}

/// The engine, the accounts and the HTTP router of one instance. With
/// `mcp_auth` the MCP transport resolves personal tokens; without it, it is
/// the open tier. `auth.anonymous` is on in both.
async fn instance(mcp_auth: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let repos: Vec<(String, Arc<MockProvider>)> = DOMAINS
        .iter()
        .map(|d| (format!("acme/{d}"), seeded_repo(d)))
        .collect();
    let repo_refs: Vec<(&str, Arc<MockProvider>)> = repos
        .iter()
        .map(|(name, mock)| (name.as_str(), mock.clone()))
        .collect();
    let installed: Vec<(&str, u64)> = repos.iter().map(|(name, _)| (name.as_str(), 1)).collect();
    let (forge, url) = spawn_forge(&repo_refs, &installed, &[(1, "ghs_test")]).await;
    forge
        .public
        .lock()
        .unwrap()
        .insert(format!("acme/{PUBLIC}"));
    let collaborators: Vec<Value> = ACCOUNTS
        .iter()
        .filter_map(|(name, _, link)| match link {
            Some((id, Some(role))) => {
                Some(json!({"id": id, "login": format!("{name}-gh"), "role_name": role}))
            }
            _ => None,
        })
        .collect();
    for (repo, _) in &repos {
        forge
            .collaborators
            .lock()
            .unwrap()
            .insert(repo.clone(), collaborators.clone());
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
            mcp: Some(mcp_auth),
            oauth: Some(false),
            anonymous: Some(true),
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
    for d in DOMAINS {
        connect(&eng, &format!("acme/{d}"), d, &root.join(d)).await;
    }
    // A domain GitHub does not govern, where the instance role decides.
    eng.domain_add_virtual(PLAIN).await.unwrap();
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&root.join("auth.db")).await.unwrap());
    for (name, role, link) in ACCOUNTS {
        auth.add_user(name, name, None, *role, PASSWORD)
            .await
            .unwrap();
        if let Some((id, _)) = link {
            auth.link_github(name, &host, *id, &format!("{name}-gh"))
                .await
                .unwrap();
        }
    }
    auth.set_write_list("voff", &["elsewhere".to_string()])
        .await
        .unwrap();
    auth.set_reflection_level("rcontrib", ReflectionLevel::Contributor)
        .await
        .unwrap();
    for d in DOMAINS {
        auth.set_reflection_grant(d, "refl", ReflectionLevel::Contributor, "root")
            .await
            .unwrap();
    }
    for d in [PRIVATE, CLOSED_PRIVATE] {
        auth.set_governed_private(d, crystalline_service::scope::OWNER_IDENTITY_NAME)
            .await
            .unwrap();
    }
    eng.write_engram(&row_engram(PLAIN, "vwrite"))
        .await
        .unwrap();
    // Every domain carries a Reflection page and one engram per row for the
    // PUT and the collab columns, written by the machine owner.
    for d in DOMAINS {
        eng.write_reflection(
            &WriteReflectionParams {
                domain: d.to_string(),
                path: "seed.md".to_string(),
                markdown: Some("# Seed\n\n## Wombat\n\nThe wombat page.\n".to_string()),
                ..Default::default()
            },
            &ReflectionCaller::owner(),
        )
        .await
        .unwrap();
        for (name, _, _) in ACCOUNTS {
            eng.write_engram(&row_engram(d, name)).await.unwrap();
        }
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
    // The App is taken off the two closed repositories: the next lookup
    // learns it, and the domains close.
    for d in [CLOSED_SHARED, CLOSED_PRIVATE] {
        forge.uninstall(&format!("acme/{d}"));
        eng.invalidate_github_access(Invalidate::Repo(format!("acme/{d}")));
    }
    // An account's question, which asks the governor (the machine owner's
    // never does).
    let asker = crystalline_service::Scope::User {
        account: "gwrite".to_string(),
        admin: false,
    };
    for _ in 0..500 {
        for d in DOMAINS {
            let _ = eng.domain_right(&asker, d).await;
        }
        if eng.github_access_closed().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut closed: Vec<String> = eng
        .github_access_closed()
        .into_iter()
        .map(|c| c.domain)
        .collect();
    closed.sort();
    assert_eq!(
        closed,
        vec![CLOSED_PRIVATE.to_string(), CLOSED_SHARED.to_string()],
        "the two domains without the App are closed"
    );
    Fixture {
        addr,
        eng,
        auth,
        _tmp: tmp,
    }
}

/// The engram `row-<account>` in `domain`, which the PUT and the collab
/// columns work on.
fn row_engram(domain: &str, account: &str) -> WriteParams {
    WriteParams {
        domain: domain.to_string(),
        title: format!("Row {account}"),
        content: "- [fact] a row's own engram".to_string(),
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

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token); `None` when the
/// login is refused.
async fn login(addr: std::net::SocketAddr, name: &str) -> Option<(String, String)> {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&json!({"name": name, "password": PASSWORD}))
        .send()
        .await
        .unwrap();
    if resp.status() != 200 {
        return None;
    }
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
    Some((cookie, body["csrf"].as_str().unwrap().to_string()))
}

fn with_session(
    request: reqwest::RequestBuilder,
    session: Option<&(String, String)>,
) -> reqwest::RequestBuilder {
    match session {
        Some((cookie, csrf)) => request
            .header("cookie", format!("fluid_session={cookie}"))
            .header("x-csrf-token", csrf),
        None => request,
    }
}

// --- MCP ---------------------------------------------------------------------

/// One MCP conversation over the HTTP transport, as `token` or as nobody.
struct Mcp {
    addr: std::net::SocketAddr,
    token: Option<String>,
    session: Option<String>,
}

impl Mcp {
    async fn open(addr: std::net::SocketAddr, token: Option<String>) -> Option<Mcp> {
        let mut mcp = Mcp {
            addr,
            token,
            session: None,
        };
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "matrix", "version": "0.0.0" },
            },
        });
        let (status, session, _) = mcp.post(&init).await;
        if status != 200 {
            return None;
        }
        mcp.session = session;
        let (status, _, _) = mcp
            .post(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        assert!((200..300).contains(&status), "initialized: {status}");
        Some(mcp)
    }

    /// POST one message; answers the status, the session id the response
    /// carried and the JSON-RPC answer (the first message with a `result`
    /// or an `error`), read off the SSE stream without waiting for its end.
    async fn post(&self, body: &Value) -> (u16, Option<String>, Value) {
        let mut request = client()
            .post(format!("http://{}/", self.addr))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let mut resp = request.send().await.unwrap();
        let status = resp.status().as_u16();
        let session = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let mut buf = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            for line in buf.lines() {
                let line = line.strip_prefix("data:").unwrap_or(line).trim();
                if let Ok(value) = serde_json::from_str::<Value>(line)
                    && (value.get("result").is_some() || value.get("error").is_some())
                {
                    return (status, session, value);
                }
            }
            match tokio::time::timeout_at(deadline, resp.chunk()).await {
                Ok(Ok(Some(bytes))) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                _ => return (status, session, Value::Null),
            }
        }
    }

    /// Call `tool`: `Ok(text)` for an answer, `Err(text)` for a refusal.
    async fn call(&self, tool: &str, arguments: Value) -> Result<String, String> {
        let (_, _, answer) = self
            .post(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": tool, "arguments": arguments },
            }))
            .await;
        if let Some(message) = answer["error"]["message"].as_str() {
            return Err(message.to_string());
        }
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if answer["result"]["isError"] == true || answer.is_null() {
            return Err(text);
        }
        Ok(text)
    }
}

fn mark(allowed: bool) -> char {
    if allowed { 'y' } else { 'n' }
}

/// The five MCP cells of `mcp` on every domain, keyed by domain, as
/// (search, read, write, read_reflection, write_reflection). `None` (no
/// session) answers all `n`.
async fn mcp_cells(mcp: Option<&Mcp>, label: &str) -> BTreeMap<&'static str, [char; 5]> {
    let mut cells = BTreeMap::new();
    let Some(mcp) = mcp else {
        for d in DOMAINS {
            cells.insert(d, ['n'; 5]);
        }
        return cells;
    };
    let found = mcp
        .call("search_engrams", json!({ "query": "quokka", "limit": 50 }))
        .await
        .unwrap_or_default();
    let tag: String = label
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_lowercase();
    for d in DOMAINS {
        let search = found.contains(&format!("seed-{d}-end"));
        let read = mcp
            .call(
                "read_engram",
                json!({ "identifier": format!("seed-{d}-end"), "domain": d }),
            )
            .await
            .is_ok_and(|text| text.contains("quokka"));
        let write = mcp
            .call(
                "write_engram",
                json!({
                    "domain": d,
                    "title": format!("By {tag} {d}"),
                    "content": "- [fact] written through MCP",
                }),
            )
            .await
            .is_ok();
        let read_reflection = mcp
            .call("read_reflection", json!({ "domain": d, "path": "seed.md" }))
            .await
            .is_ok_and(|text| text.contains("wombat page"));
        let write_reflection = mcp
            .call(
                "write_reflection",
                json!({
                    "domain": d,
                    "path": format!("by-{tag}.md"),
                    "markdown": format!("# By {tag}\n\n## Note\n\nA note.\n"),
                }),
            )
            .await
            .is_ok();
        cells.insert(
            d,
            [
                mark(search),
                mark(read),
                mark(write),
                mark(read_reflection),
                mark(write_reflection),
            ],
        );
    }
    cells
}

// --- the JSON API, collab and events -----------------------------------------

/// `GET /domains`, engram `PUT` and the collab upgrade on every domain, as
/// (list, put, collab). The PUT and the room are over the row's own engram
/// `row-<account>` (`row-root` for the anonymous visitor).
async fn rest_cells(
    fx: &Fixture,
    session: Option<&(String, String)>,
    account: &str,
    root: &(String, String),
) -> BTreeMap<&'static str, [char; 3]> {
    let listed = with_session(
        client().get(format!("http://{}/api/v1/domains", fx.addr)),
        session,
    )
    .send()
    .await
    .unwrap();
    let listed: Value = if listed.status() == 200 {
        listed.json().await.unwrap()
    } else {
        Value::Null
    };
    let listed = listed.to_string();
    let mut cells = BTreeMap::new();
    for d in DOMAINS {
        let list = listed.contains(&format!("\"name\":\"{d}\""));
        let permalink = format!("row-{account}");
        let put_status = put_row(fx, session, d, account, root).await;
        let collab = collab_join(fx.addr, session, d, &permalink).await;
        cells.insert(d, [mark(list), mark(put_status < 300), mark(collab)]);
    }
    cells
}

/// `PUT` the engram `row-<account>` of `domain` as `session`, with the
/// version `root` reads first; answers the status (2xx, 401, 403 or 404).
async fn put_row(
    fx: &Fixture,
    session: Option<&(String, String)>,
    domain: &str,
    account: &str,
    root: &(String, String),
) -> u16 {
    let url = format!(
        "http://{}/api/v1/domains/{domain}/engrams/row-{account}",
        fx.addr
    );
    let detail = with_session(client().get(&url), Some(root))
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), 200, "{domain}/row-{account} as root");
    let etag = detail.headers()["etag"].to_str().unwrap().to_string();
    let text = engram(
        &format!("Row {account}"),
        &format!("row-{account}"),
        "Saved through the JSON API.",
    );
    let put = with_session(client().put(&url), session)
        .header("if-match", etag)
        .json(&json!({ "content": String::from_utf8(text).unwrap() }))
        .send()
        .await
        .unwrap();
    let status = put.status().as_u16();
    assert!(
        status < 300 || [401, 403, 404].contains(&status),
        "{account} PUT {domain}: {status} {}",
        put.text().await.unwrap_or_default()
    );
    status
}

/// Whether the collab socket for `domain/permalink` opens as `session`.
async fn collab_join(
    addr: std::net::SocketAddr,
    session: Option<&(String, String)>,
    domain: &str,
    permalink: &str,
) -> bool {
    use tokio_tungstenite::tungstenite;
    let mut request = tungstenite::handshake::client::Request::builder()
        .uri(format!("ws://{addr}/api/v1/collab/{domain}/{permalink}"))
        .header("Host", addr.to_string())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .header("Origin", format!("http://{addr}"));
    if let Some((cookie, _)) = session {
        request = request.header("Cookie", format!("fluid_session={cookie}"));
    }
    match tokio_tungstenite::connect_async(request.body(()).unwrap()).await {
        Ok((mut socket, _)) => {
            let _ = socket.close(None).await;
            true
        }
        Err(tungstenite::Error::Http(response)) => {
            let status = response.status().as_u16();
            assert!(
                [401, 403, 404].contains(&status),
                "collab {domain}/{permalink}: {status}"
            );
            false
        }
        Err(e) => panic!("collab {domain}/{permalink}: {e}"),
    }
}

/// Open `/events` as `session`; `None` when the stream is refused.
async fn open_events(
    addr: std::net::SocketAddr,
    session: Option<&(String, String)>,
) -> Option<reqwest::Response> {
    let resp = with_session(
        client()
            .get(format!("http://{addr}/api/v1/events"))
            .header("accept", "text/event-stream"),
        session,
    )
    .send()
    .await
    .unwrap();
    (resp.status() == 200).then_some(resp)
}

/// The domains of the engram frames `resp` carries within `within`.
async fn heard(resp: Option<reqwest::Response>, within: Duration) -> Vec<String> {
    let Some(mut resp) = resp else {
        return Vec::new();
    };
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + within;
    while let Ok(Ok(Some(bytes))) = tokio::time::timeout_at(deadline, resp.chunk()).await {
        buf.push_str(&String::from_utf8_lossy(&bytes));
    }
    buf.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .filter(|frame| {
            frame["permalink"]
                .as_str()
                .is_some_and(|p| p.starts_with("seed-"))
                || frame.to_string().contains("seed-")
        })
        .filter_map(|frame| frame["domain"].as_str().map(str::to_string))
        .collect()
}

// --- the matrix --------------------------------------------------------------

/// Every actual cell of every row, as the same strings the table spells.
async fn collect() -> Vec<(String, [String; 5])> {
    let rows = rows();
    let a = instance(true).await;
    let b = instance(false).await;
    let root = login(a.addr, "root").await.expect("root logs in");

    // Sessions first, then every `/events` stream, then one change per
    // domain, so a stream hears exactly the changes it may hear.
    let mut sessions: BTreeMap<&str, Option<(String, String)>> = BTreeMap::new();
    for row in &rows {
        if let Via::Person(account) = row.via {
            sessions.insert(account, login(a.addr, account).await);
        }
    }
    let mut streams = Vec::new();
    for row in &rows {
        let stream = match row.via {
            Via::Person(account) => match &sessions[account] {
                Some(session) => open_events(a.addr, Some(session)).await,
                None => None,
            },
            Via::Anonymous => open_events(a.addr, None).await,
            _ => None,
        };
        streams.push(stream);
    }
    for d in DOMAINS {
        a.eng
            .edit_engram(&EditParams {
                identifier: format!("seed-{d}-end"),
                domain: d.to_string(),
                operation: "append".to_string(),
                content: Some("- [fact] an event".to_string()),
                ..EditParams::default()
            })
            .await
            .unwrap();
    }
    let events: Vec<Vec<String>> = futures::future::join_all(
        streams
            .into_iter()
            .map(|stream| heard(stream, Duration::from_secs(3))),
    )
    .await;

    let mut actual = Vec::new();
    for (row, heard) in rows.iter().zip(events) {
        let mut cells: BTreeMap<&str, Vec<char>> =
            DOMAINS.iter().map(|d| (*d, vec!['-'; 9])).collect();
        // The MCP columns.
        let mcp = match row.via {
            Via::Person(account) | Via::Token(account) => {
                match a.auth.issue_mcp_token(account, "agent").await {
                    Ok(issued) => Mcp::open(a.addr, Some(issued.token)).await,
                    Err(_) => None,
                }
            }
            Via::OpenTier(agent) => {
                b.eng
                    .configure(&ConfigureAction::Set {
                        key: "github.agent_identity".to_string(),
                        value: agent.to_string(),
                    })
                    .await
                    .unwrap();
                Mcp::open(b.addr, None).await
            }
            Via::Anonymous => {
                b.eng
                    .configure(&ConfigureAction::Unset {
                        key: "github.agent_identity".to_string(),
                    })
                    .await
                    .unwrap();
                Mcp::open(b.addr, None).await
            }
        };
        let mcp_answers = mcp_cells(mcp.as_ref(), row.label).await;
        for d in DOMAINS {
            let [s, r, w, rr, rw] = mcp_answers[d];
            let cell = cells.get_mut(d).unwrap();
            cell[0] = s;
            cell[1] = r;
            cell[2] = w;
            cell[7] = rr;
            cell[8] = rw;
        }
        // The JSON API, collab and events columns.
        let rest = match row.via {
            Via::Person(account) => Some((sessions[account].as_ref(), account)),
            Via::Anonymous => Some((None, "root")),
            _ => None,
        };
        if let Some((session, account)) = rest {
            // A refused login reaches nothing.
            let refused_login = matches!(row.via, Via::Person(_)) && session.is_none();
            let rest = if refused_login {
                DOMAINS.iter().map(|d| (*d, ['n'; 3])).collect()
            } else {
                rest_cells(&a, session, account, &root).await
            };
            for d in DOMAINS {
                let [l, p, c] = rest[d];
                let cell = cells.get_mut(d).unwrap();
                cell[3] = l;
                cell[4] = p;
                cell[5] = c;
                cell[6] = mark(heard.iter().any(|h| h == d));
            }
        }
        let strings: Vec<String> = DOMAINS.iter().map(|d| cells[d].iter().collect()).collect();
        actual.push((row.label.to_string(), strings.try_into().unwrap()));
    }
    actual
}

/// Renders every mismatching cell as `row / domain / column: expected x, got y`.
fn mismatches(actual: &[(String, [String; 5])]) -> Vec<String> {
    let mut out = Vec::new();
    for (row, (label, got)) in rows().iter().zip(actual) {
        assert_eq!(row.label, label);
        for (i, d) in DOMAINS.iter().enumerate() {
            let want: Vec<char> = row.expected[i].chars().collect();
            let got: Vec<char> = got[i].chars().collect();
            for (c, column) in COLUMNS.iter().enumerate() {
                if want[c] != got[c] {
                    out.push(format!(
                        "{label} / {d} / {column}: expected {}, got {}",
                        want[c], got[c]
                    ));
                }
            }
        }
    }
    out
}

/// The JSON API's write gate and the collab upgrade let GitHub decide on a
/// governed domain: an instance viewer with GitHub write saves and opens a
/// room there, while on a domain GitHub does not govern the instance role
/// still refuses that viewer. Instance `none` and the anonymous visitor stay
/// refused on the governed domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_on_the_json_api_follow_github_not_the_instance_role() {
    let fx = instance(true).await;
    let root = login(fx.addr, "root").await.unwrap();
    let vwrite = login(fx.addr, "vwrite").await.unwrap();
    assert_eq!(
        put_row(&fx, Some(&vwrite), SHARED, "vwrite", &root).await,
        200
    );
    assert_eq!(
        put_row(&fx, Some(&vwrite), PRIVATE, "vwrite", &root).await,
        200
    );
    assert!(collab_join(fx.addr, Some(&vwrite), SHARED, "row-vwrite").await);
    // Where the instance role decides, a viewer writes nothing.
    assert_eq!(
        put_row(&fx, Some(&vwrite), PLAIN, "vwrite", &root).await,
        403
    );
    assert!(!collab_join(fx.addr, Some(&vwrite), PLAIN, "row-vwrite").await);
    // Off its write list it writes nothing, GitHub write or not.
    let voff = login(fx.addr, "voff").await.unwrap();
    assert_eq!(put_row(&fx, Some(&voff), SHARED, "voff", &root).await, 403);
    // Nobody signed in: 401, as on every other write.
    assert_eq!(put_row(&fx, None, PUBLIC, "root", &root).await, 401);
    // Instance `none` with GitHub write: nothing (R16).
    if let Some(inone) = login(fx.addr, "inone").await {
        assert_ne!(
            put_row(&fx, Some(&inone), SHARED, "inone", &root).await,
            200
        );
        assert!(!collab_join(fx.addr, Some(&inone), SHARED, "row-inone").await);
    }
}

/// The editor's dry run (`POST /validate`) follows the same rule as the save
/// it checks: an instance viewer with GitHub write may run it for a governed
/// domain, and still not for a domain the instance role decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn github_writers_may_dry_run_what_they_save() {
    let fx = instance(true).await;
    let vwrite = login(fx.addr, "vwrite").await.unwrap();
    let gread = login(fx.addr, "gread").await.unwrap();
    let validate = |session: Option<(String, String)>, domain: &'static str| {
        let addr = fx.addr;
        async move {
            let text = engram("Draft", "draft", "A draft.");
            with_session(
                client().post(format!("http://{addr}/api/v1/validate")),
                session.as_ref(),
            )
            .json(&json!({ "content": String::from_utf8(text).unwrap(), "domain": domain }))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
        }
    };
    assert_eq!(validate(Some(vwrite.clone()), SHARED).await, 200);
    assert_eq!(validate(Some(vwrite), PLAIN).await, 403);
    // An instance editor keeps its dry run wherever it had one.
    assert_eq!(validate(Some(gread), SHARED).await, 200);
    assert_eq!(validate(None, SHARED).await, 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn github_governed_matrix() {
    let actual = collect().await;
    let mut table = String::new();
    for (label, cells) in &actual {
        table.push_str(&format!("{label:<55} {}\n", cells.join(" ")));
    }
    let wrong = mismatches(&actual);
    assert!(
        wrong.is_empty(),
        "{} wrong cells:\n{}\n\nactual (columns {:?}, domains {:?}):\n{table}",
        wrong.len(),
        wrong.join("\n"),
        COLUMNS,
        DOMAINS
    );
}

// --- removing the App, and a rate limit ---------------------------------------

/// One governed domain per repository in `repos`, connected shared, on an
/// engine whose App comes from the environment (`saved` false) or is saved
/// in the store the way `crystalline github app set` saves it (`saved`
/// true, so `remove_github_app` can take it away). Accounts: `root`
/// (instance admin), `ada` (instance editor, not linked) and `vic`
/// (instance viewer, GitHub write as id 5).
async fn small_instance(
    tmp: &std::path::Path,
    repos: &[&str],
    saved: bool,
) -> (Arc<Forge>, Arc<Engine>, Arc<AuthStore>) {
    let mocks: Vec<(String, Arc<MockProvider>)> = repos
        .iter()
        .map(|d| (format!("acme/{d}"), seeded_repo(d)))
        .collect();
    let refs: Vec<(&str, Arc<MockProvider>)> = mocks
        .iter()
        .map(|(name, mock)| (name.as_str(), mock.clone()))
        .collect();
    let installed: Vec<(&str, u64)> = mocks.iter().map(|(name, _)| (name.as_str(), 1)).collect();
    let (forge, url) = spawn_forge(&refs, &installed, &[(1, "ghs_test")]).await;
    for (repo, _) in &mocks {
        forge.collaborators.lock().unwrap().insert(
            repo.clone(),
            vec![json!({"id": 5, "login": "vic-gh", "role_name": "write"})],
        );
    }
    let env: Vec<(&str, &str)> = if saved {
        vec![(
            "CRYSTALLINE_GITHUB_TOKEN",
            super::github_app::INSTANCE_TOKEN,
        )]
    } else {
        app_env()
    };
    let eng = Arc::new(super::github_app::engine(tmp, &url, &env).await);
    if saved {
        eng.set_github_app(crystalline_remote::AppCredentials {
            app_id: 7,
            client_id: "Iv23.test".to_string(),
            client_secret: None,
            private_key: super::github_app::PEM.to_string(),
            webhook_secret: None,
            slug: None,
            host: String::new(),
        })
        .await
        .unwrap();
    }
    for d in repos {
        connect(&eng, &format!("acme/{d}"), d, &tmp.join(d)).await;
    }
    let host = eng.github_app().unwrap().credentials().host.clone();
    let auth = Arc::new(AuthStore::open(&tmp.join("auth.db")).await.unwrap());
    for (name, role) in [
        ("root", Role::Admin),
        ("ada", Role::Editor),
        ("vic", Role::Viewer),
    ] {
        auth.add_user(name, name, None, role, PASSWORD)
            .await
            .unwrap();
    }
    auth.link_github("vic", &host, 5, "vic-gh").await.unwrap();
    eng.set_domain_access(Arc::new(
        crystalline_service::DomainAccess::new(auth.clone())
            .with_governor(eng.knowledge_governor()),
    ));
    (forge, eng, auth)
}

fn user(name: &str) -> crystalline_service::Scope {
    crystalline_service::Scope::User {
        account: name.to_string(),
        admin: false,
    }
}

/// Taking the App away hands every domain back to Crystalline's own rules
/// on the next request. A domain that was private before the App keeps its
/// old owner and members. A domain made private while GitHub governed it
/// records the machine owner as its owner (R26), so afterwards only instance
/// admins and the machine owner reach it until an admin hands it over: it
/// fails closed rather than open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_the_app_restores_old_grants() {
    use crystalline_service::scope::DomainRight;
    let tmp = tempfile::tempdir().unwrap();
    let (_forge, eng, auth) = small_instance(tmp.path(), &["old", "new"], true).await;
    // `old` was private before the App: ada owns it, vic is a viewer there.
    auth.set_domain_visibility("old", true, "ada")
        .await
        .unwrap();
    auth.upsert_domain_member(
        "old",
        "vic",
        crystalline_service::rest::MemberLevel::Viewer,
        None,
        "ada",
    )
    .await
    .unwrap();
    // `new` was made private while governed.
    auth.set_governed_private("new", crystalline_service::scope::OWNER_IDENTITY_NAME)
        .await
        .unwrap();

    // Governed: GitHub decides. ada has no link and reads neither private
    // domain; vic writes both through GitHub write.
    assert!(eng.github_governs("old") && eng.github_governs("new"));
    for d in ["old", "new"] {
        assert_eq!(
            eng.domain_right(&user("ada"), d).await.unwrap(),
            DomainRight::None,
            "{d}"
        );
        assert_eq!(
            eng.domain_right(&user("vic"), d).await.unwrap(),
            DomainRight::Write,
            "{d}"
        );
    }

    eng.remove_github_app().await.unwrap();
    assert!(eng.github_app().is_none());

    // The next request: Crystalline's own grants are back.
    assert!(!eng.github_governs("old") && !eng.github_governs("new"));
    assert_eq!(
        eng.domain_right(&user("ada"), "old").await.unwrap(),
        DomainRight::Own
    );
    assert_eq!(
        eng.domain_right(&user("vic"), "old").await.unwrap(),
        DomainRight::Read
    );
    // `new` fails closed: owned by the machine owner, nobody invited.
    assert_eq!(
        auth.private_domains()
            .await
            .unwrap()
            .into_iter()
            .find(|acl| acl.domain == "new")
            .unwrap()
            .owner,
        crystalline_service::scope::OWNER_IDENTITY_NAME
    );
    assert_eq!(
        eng.domain_right(&user("ada"), "new").await.unwrap(),
        DomainRight::None
    );
    assert_eq!(
        eng.domain_right(&user("vic"), "new").await.unwrap(),
        DomainRight::None
    );
    assert!(eng.hidden_for(&user("vic")).await.unwrap().contains("new"));
    let root = crystalline_service::Scope::User {
        account: "root".to_string(),
        admin: true,
    };
    assert_eq!(
        eng.domain_right(&root, "new").await.unwrap(),
        DomainRight::Own
    );
    assert_eq!(
        eng.domain_right(&crystalline_service::Scope::Unrestricted, "new")
            .await
            .unwrap(),
        DomainRight::Own
    );
}

/// The `/collaborators` calls the forge answered for `repo`.
fn listings(forge: &Forge, repo: &str) -> usize {
    forge
        .hits()
        .iter()
        .filter(|h| h.path == format!("/repos/{repo}/collaborators"))
        .count()
}

/// Moves the tokio clock (which the governor's cache reads) on by `by`.
/// The pause is only around the jump, so the forge answers in real time.
async fn jump(by: Duration) {
    tokio::time::pause();
    tokio::time::advance(by).await;
    tokio::time::resume();
}

/// GitHub's rate limit on the collaborator listing does not take a role
/// away: past the five fresh minutes the governor asks again, is refused
/// with the rate limit, and keeps serving the role it knew, until the hour
/// past expiry is over.
#[tokio::test]
async fn a_rate_limited_lookup_serves_the_cached_role() {
    use crystalline_service::engine::{FRESH_FOR, STALE_FOR};
    use crystalline_service::scope::DomainRight;
    let tmp = tempfile::tempdir().unwrap();
    let (forge, eng, _auth) = small_instance(tmp.path(), &["kb"], false).await;
    // vic is an instance viewer: only GitHub write lets it write.
    assert_eq!(
        eng.domain_right(&user("vic"), "kb").await.unwrap(),
        DomainRight::Write
    );
    assert_eq!(listings(&forge, "acme/kb"), 1);
    assert_eq!(eng.github_access_stale_since(), None);

    forge
        .collaborators_rate_limited
        .store(true, std::sync::atomic::Ordering::SeqCst);
    jump(FRESH_FOR + Duration::from_secs(60)).await;
    assert_eq!(
        eng.domain_right(&user("vic"), "kb").await.unwrap(),
        DomainRight::Write,
        "the cached role holds while GitHub rate-limits"
    );
    assert_eq!(listings(&forge, "acme/kb"), 2, "GitHub was asked again");
    assert!(eng.github_access_stale_since().is_some());

    // Past the hour after expiry the old role is gone: vic reads the shared
    // domain and writes nothing.
    jump(STALE_FOR).await;
    assert_eq!(
        eng.domain_right(&user("vic"), "kb").await.unwrap(),
        DomainRight::Read
    );
}
