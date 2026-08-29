//! **What a 2026-07-28 client is actually served, over both transports.**
//!
//! Every other rmcp test in this crate drives `serve_client`'s default
//! `ClientLifecycleMode::Initialize` (rmcp 3.1.2 `service/client.rs:633-649`)
//! and therefore exercises the legacy era only. That gap is what this file
//! closes, and it could not be closed before the revision was advertised:
//! `handler/server.rs:64-72` refuses any inline request whose `_meta` names a
//! version outside `supported_protocol_versions()` with `-32022`, before
//! dispatch, so until `crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS`
//! carried 2026-07-28 there was no wire path to a modern
//! `RequestContext::protocol_version()` at all.
//!
//! Three obligations earlier tasks proved another way and handed here to be
//! proved on the wire:
//!
//! - **the caching MUST.** `tests/mcp_cache_hints.rs` builds a
//!   `RequestContext` directly and calls the handler methods, because no modern
//!   peer was reachable. Both transports carry the hints here, `resources/read`
//!   included, and a legacy session on the same binary still carries none.
//! - **subscriptions over HTTP.** `tests/mcp_subscriptions.rs` reaches the
//!   modern dispatch over stdio through the metadata latch
//!   (`service/server.rs:541`, the crate's only call site). Nothing arms that
//!   latch on the streamable-HTTP path, so `subscriptions/listen` was
//!   classified legacy there and answered `method not found` at every revision
//!   we advertised. This file is the first time that path runs.
//! - **identity from request metadata.** `client_actor` reads
//!   `_meta.io.modelcontextprotocol/clientInfo` first for exactly this era; a
//!   modern write now proves it reaches `generated.by`.
//!
//! **The era is sessionless by construction.** No `Mcp-Session-Id` is issued to
//! a modern peer and none is required: rmcp inserts that header at one site
//! (`tower.rs:1911`) inside the legacy session branch, and
//! `is_legacy_request` (`tower.rs:358-408`) routes anything at or above
//! 2026-07-28 past it. Asserted rather than assumed below, because it is the
//! deployment fact an operator behind a load balancer needs.

mod support;

use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{
    DomainEntry, GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::mcp::McpServer;
use serde_json::{Value, json};
use support::MockProvider;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// The revision this whole file is about, spelled once.
const ERA: &str = "2026-07-28";

/// The revision the legacy contrast legs use: the newest one that still has an
/// `initialize` handshake.
const LEGACY: &str = "2025-11-25";

/// Every generated folder index under `root`, as domain-relative paths.
///
/// The engine writes one per directory it holds, and they travel with a share
/// like any other file, so a fixture that wants a tree matching its origin has
/// to put them in the origin too.
fn generated_indexes(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                stack.push((entry.path(), rel));
            } else if crystalline_core::is_index_file(&name) {
                found.push(rel);
            }
        }
    }
    found
}

// --- the engine, and the two wires ------------------------------------------

struct Harness {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    engine: Arc<Engine>,
}

impl Harness {
    async fn new() -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        let dir = root.join("eng");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
        )
        .unwrap();
        cfg.domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        // `configure` reads the GitHub credential once the setting is on;
        // nothing here may reach the developer's real OS keychain.
        let token_store = root.join("token-store");
        std::fs::create_dir_all(&token_store).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_token_store_dir(token_store),
        );
        engine.sync(None).await.unwrap();
        Harness {
            _tmp: tmp,
            root,
            engine,
        }
    }

    /// A harness whose engine has GitHub enabled, a mock forge injected and
    /// one team domain "kb" subscribed at a single-engram commit. Returns the
    /// mock so tests can read its call log and see that round one wrote
    /// nothing to the forge.
    ///
    /// No `sync` runs here: `origin_add` indexes what it downloaded itself,
    /// and every later edit these tests make is read off disk by the share
    /// path rather than out of the index.
    async fn team() -> (Harness, Arc<MockProvider>) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let cfg = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(true),
                ..GitHubConfig::default()
            }),
            service: Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                ..ServiceConfig::default()
            }),
            ..GlobalConfig::default()
        };
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let token_store = root.join("token-store");
        std::fs::create_dir_all(&token_store).unwrap();

        let mock = Arc::new(MockProvider::new());
        let mut origin_tree: std::collections::BTreeMap<String, Vec<u8>> = [
                ("MANIFEST.md".to_string(), b"---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- Everything\n\n## When to Use\n\n- Always\n".to_vec()),
                ("notes/a.md".to_string(), b"---\ntype: engram\ntitle: Alpha\npermalink: notes/a\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nalpha\n".to_vec()),
            ]
            .into_iter()
            .collect();
        let c1 = mock.add_commit(origin_tree.clone());
        mock.set_branch("main", &c1);

        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_token_store_dir(token_store)
                .with_origin_provider(mock.clone())
                .with_origins_dir(root.join("origins")),
        );
        let domain_root = root.join("kb");
        engine
            .origin_add(
                "team/knowledge",
                Some("kb"),
                None,
                None,
                Some(domain_root.to_str().unwrap()),
            )
            .await
            .unwrap();
        // Subscribing generates a folder index per directory, and those travel
        // with a share now: left as they are, this domain would stand one
        // refresh ahead of an origin that has never seen them, and "the tree
        // matches the origin" would not be true of it. So the listings the
        // generator just wrote are seeded into the origin and pulled back,
        // which is exactly what the first share after an upgrade does. From
        // here the tree really does match.
        for rel in generated_indexes(&domain_root) {
            let bytes = std::fs::read(domain_root.join(&rel)).unwrap();
            origin_tree.insert(rel, bytes);
        }
        let c2 = mock.add_commit(origin_tree);
        mock.set_branch("main", &c2);
        engine.origin_update(Some("kb")).await.unwrap();
        (
            Harness {
                _tmp: tmp,
                root,
                engine,
            },
            mock,
        )
    }

    /// Re-open this harness's instance sharing with personal GitHub
    /// identities, on a fresh engine with NO provider injected in front of it.
    ///
    /// Both halves are load-bearing. The mode is what splits the write
    /// credential off the instance one, and dropping the injected provider is
    /// what lets the split actually run: an injected mock short-circuits
    /// credential resolution for both modes (`Engine::resolve_share_provider`),
    /// so a test that kept it would never reach the token store and never see
    /// the refusal. The config, the domain registration and the origin state
    /// the mock already wrote are all on disk, so the new engine picks up the
    /// same team domain; the token-store directory is the empty one the
    /// harness created, which is what "connected nothing yet" means here and
    /// is why no keychain is ever touched.
    async fn share_personally(&mut self, agent_identity: Option<&str>) {
        let config_path = self.root.join("config.yaml");
        let mut cfg: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
        let github = cfg.github.get_or_insert_with(GitHubConfig::default);
        github.enabled = Some(true);
        github.share_identity = Some("personal".to_string());
        github.agent_identity = agent_identity.map(str::to_string);
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        self.engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_token_store_dir(self.root.join("token-store"))
                .with_origins_dir(self.root.join("origins")),
        );
    }

    /// A raw newline-delimited JSON-RPC conversation over the same duplex
    /// transport the stdio bridge uses. The bytes are the subject here, so no
    /// rmcp client sits between the assertions and the wire.
    async fn stdio(&self) -> Wire {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let engine = self.engine.clone();
        let server =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let (read, write) = tokio::io::split(client_io);
        Wire {
            write,
            lines: tokio::io::BufReader::new(read).lines(),
            server: Some(server),
            running: None,
        }
    }

    /// Bind the daemon's real HTTP router on an ephemeral loopback port. The
    /// same construction `run_http` mounts, so the transport behaviour is the
    /// deployed one rather than a test-only assembly.
    async fn http(&self) -> std::net::SocketAddr {
        let auth = Arc::new(
            crystalline_service::rest::AuthStore::open(&self.root.join("web-auth.db"))
                .await
                .unwrap(),
        );
        let router = http_router(
            self.engine.clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            &[],
            auth,
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
        addr
    }
}

type ServerTask = tokio::task::JoinHandle<
    Result<
        rmcp::service::RunningService<rmcp::RoleServer, McpServer>,
        rmcp::service::ServerInitializeError,
    >,
>;

struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    /// The init loop's join handle until the opener has been sent. rmcp's
    /// `serve_server` returns as soon as it has handled the first message -
    /// whether that was an `initialize` or an inline modern request - so the
    /// handle is awaited exactly once and the running service it yields is
    /// kept alive for the rest of the conversation.
    server: Option<ServerTask>,
    running: Option<rmcp::service::RunningService<rmcp::RoleServer, McpServer>>,
}

impl Wire {
    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// The next message the server sends, or `None` if it stays silent.
    async fn recv(&mut self) -> Option<Value> {
        match tokio::time::timeout(Duration::from_millis(1500), self.lines.next_line()).await {
            Ok(Ok(Some(line))) => Some(serde_json::from_str(&line).unwrap()),
            _ => None,
        }
    }

    /// Send the session opener and read its answer, collecting the running
    /// service the init loop hands back.
    async fn open(&mut self, message: Value) -> Value {
        self.send(message).await;
        let task = self.server.take().expect("the opener is sent once");
        self.running = Some(task.await.unwrap().expect("the server opened a session"));
        self.recv().await.expect("the opener was answered")
    }

    /// Send a request on an open session and read its answer.
    async fn call(&mut self, message: Value) -> Value {
        self.send(message).await;
        self.recv().await.expect("the request was answered")
    }
}

/// The `_meta` a modern request carries: the two `DRAFT_REQUIRED_KEYS`
/// (rmcp 3.1.2 `model/meta.rs:400-403`) plus the optional `clientInfo` this
/// server reads for write provenance and for nothing else.
fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "modern-era-test", "version": "9.9.9" },
    })
}

fn request(id: u32, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A modern request: the method, its parameters and the era's `_meta`.
fn modern(id: u32, method: &str, mut params: Value) -> Value {
    params["_meta"] = modern_meta();
    request(id, method, params)
}

/// [`modern_meta`] from a client that can put a question to its user: the same
/// two required keys, with an `elicitation` capability declared beside them.
///
/// Capabilities travel per request in this era rather than per session
/// (`RequestContext::client_capabilities` reads `_meta` first and reads only
/// `_meta` once the metadata latch is armed, rmcp 3.1.2 `service.rs:1243-1251`),
/// so one connection can send both shapes and the two halves of the gate are
/// testable against the same server.
fn eliciting_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": { "elicitation": { "form": {} } },
        "io.modelcontextprotocol/clientInfo": { "name": "modern-era-test", "version": "9.9.9" },
    })
}

/// A modern request from an eliciting client.
fn eliciting(id: u32, method: &str, mut params: Value) -> Value {
    params["_meta"] = eliciting_meta();
    request(id, method, params)
}

/// The three fields SEP-2549 and SEP-2322 put on a complete result for a
/// modern peer. `ttlMs: 0` and `cacheScope: "public"` are what rmcp's own
/// `#[tool_handler]` emits (`rmcp-macros-3.1.2/src/tool_handler.rs:79-81`), so
/// a hand-written endpoint and a generated one are indistinguishable here.
fn assert_hinted(label: &str, result: &Value) {
    assert_eq!(
        result["resultType"],
        json!("complete"),
        "{label} carries the SEP-2322 discriminator: {result}"
    );
    assert_eq!(result["ttlMs"], json!(0), "{label} carries ttlMs: {result}");
    assert_eq!(
        result["cacheScope"],
        json!("public"),
        "{label} carries cacheScope: {result}"
    );
}

// --- stdio ------------------------------------------------------------------

/// **A modern client is served with no handshake at all.**
///
/// The 2026-07-28 schema has no `initialize` and no `InitializeResult`
/// (`grep -i initialize schema.ts` returns zero hits). rmcp's stdio init loop
/// implements that: a first message that is not `initialize` and carries both
/// required `_meta` keys is dispatched inline, answered, and the session
/// continues (`service/server.rs:527-556`). Before this revision was
/// advertised the same request was answered `-32022` instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_client_is_served_with_no_handshake_at_all() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let answer = wire.open(modern(1, "tools/list", json!({}))).await;
    assert_eq!(answer["id"], json!(1));
    let tools = answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tool list in {answer}"));
    assert_eq!(
        tools.len(),
        18,
        "a default install's list, unchanged by the era"
    );
    assert_hinted("tools/list", &answer["result"]);
}

/// Discovery is the era's only onboarding channel, and it carries the routing
/// block plus the caching hints rmcp's own `DiscoverResult::from_server_info`
/// sets (`ttl_ms: 0`, `cache_scope: Private`, `model.rs:1258-1263`).
///
/// `supportedVersions` is the other half of the answer, and it is what a
/// dual-era client reads to decide which revision to speak: it now names the
/// era, which is exactly what this task changed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_carries_the_routing_block_and_names_the_era() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let answer = wire.open(modern(1, "server/discover", json!({}))).await;
    let result = &answer["result"];
    assert!(
        result["instructions"]
            .as_str()
            .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
        "the modern era's onboarding path carries the block: {answer}"
    );
    assert!(
        result["instructions"]
            .as_str()
            .is_some_and(|s| s.contains("- eng: Route here for eng questions")),
        "and the block is the live one: {answer}"
    );
    let versions: Vec<&str> = result["supportedVersions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        versions.contains(&ERA),
        "a probing client is told the era is available: {versions:?}"
    );
    assert_eq!(result["resultType"], json!("complete"));
    assert_eq!(result["ttlMs"], json!(0));
    assert_eq!(
        result["cacheScope"],
        json!("private"),
        "discover is private because the block follows this deployment's \
         configuration (rmcp sets it, model.rs:1262)"
    );
}

/// **The caching MUST, over the wire, for all six operations.**
///
/// `/server/utilities/caching`: "Servers MUST include caching hints on results
/// with `resultType: "complete"` returned by the following operations:
/// `server/discover`, `tools/list`, `prompts/list`, `resources/list`,
/// `resources/templates/list`, `resources/read`." `tests/mcp_cache_hints.rs`
/// proves the same six by calling the handler methods with a fabricated
/// context, which was the only way to reach a modern peer before the era was
/// advertised. This is the leg it could not run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_operation_the_caching_must_names_carries_its_hints_over_stdio() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    // The opener also arms the metadata latch, so every request after it is
    // dispatched on the modern path.
    let discovered = wire.open(modern(1, "server/discover", json!({}))).await;
    assert_eq!(discovered["result"]["ttlMs"], json!(0));

    for (id, method, params) in [
        (2, "tools/list", json!({})),
        (3, "prompts/list", json!({})),
        (4, "resources/list", json!({})),
        (5, "resources/templates/list", json!({})),
        (
            6,
            "resources/read",
            json!({ "uri": "skill://crystalline-routing/SKILL.md" }),
        ),
    ] {
        let answer = wire.call(modern(id, method, params)).await;
        assert!(
            answer["error"].is_null(),
            "{method} is served to a modern peer: {answer}"
        );
        assert_hinted(method, &answer["result"]);
    }
}

/// The legacy contrast, on the same binary and the same handler: a peer below
/// the era carries none of the three fields.
///
/// **A guard rather than a red**: it passes before and after the era is
/// advertised, and its job is to fail if the hints are ever emitted
/// unconditionally instead of for the peers the revision defines them for. They are 2026-07-28 wire shape,
/// so emitting them to a 2025-11-25 client would be inventing a shape that
/// revision has no field for. rmcp strips `resultType` itself below the era
/// (`handler/server.rs:246-260`); the other two are our own gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_on_the_same_binary_is_handed_none_of_it() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": {},
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    for (id, method) in [(2, "tools/list"), (3, "resources/list")] {
        let answer = wire.call(request(id, method, json!({}))).await;
        let result = answer["result"].as_object().unwrap();
        for field in ["resultType", "ttlMs", "cacheScope"] {
            assert!(
                !result.contains_key(field),
                "{method} carries no {field} for a legacy peer: {answer}"
            );
        }
    }
}

/// **V4 on the wire: `ping` is removed, and we inherit the removal.**
///
/// rmcp answers `method_not_found` unless the request is on the legacy
/// lifecycle (`handler/server.rs:112-118`); we implement no `ping` at all, so
/// there is nothing of ours in this behaviour beyond which era a peer is on.
/// Both halves are asserted, because a removal that also broke the legacy era
/// would be a regression rather than conformance.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_is_removed_for_a_modern_peer_and_still_answered_for_a_legacy_one() {
    let h = Harness::new().await;

    let mut modern_wire = h.stdio().await;
    modern_wire
        .open(modern(1, "server/discover", json!({})))
        .await;
    let refused = modern_wire.call(modern(2, "ping", json!({}))).await;
    assert_eq!(
        refused["error"]["code"],
        json!(-32601),
        "ping is gone from the era: {refused}"
    );

    let mut legacy_wire = h.stdio().await;
    legacy_wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": {},
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    let answered = legacy_wire.call(request(2, "ping", json!({}))).await;
    assert!(
        answered["error"].is_null() && answered["result"].is_object(),
        "a legacy peer keeps ping: {answered}"
    );
}

/// A request that omits a required `_meta` key on a connection whose latch is
/// armed is refused `-32602`, and the message names both keys.
///
/// The message text is asserted rather than the code alone: a bare code can be
/// produced by a malformed request for reasons that have nothing to do with
/// the lifecycle, which is the trap this program recorded at Task 4.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_missing_its_required_meta_is_refused_with_invalid_params() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    // The opener has to be *answered*, not merely accepted: rmcp arms the
    // latch at `service/server.rs:541` before dispatch, so a probe refused
    // `-32022` for an unadvertised version would arm it too and make the
    // refusal below pass on a connection that was never modern at all.
    let opened = wire.open(modern(1, "server/discover", json!({}))).await;
    assert!(
        opened["result"]["instructions"].is_string(),
        "the connection is a modern one: {opened}"
    );

    let refused = wire.call(request(2, "tools/list", json!({}))).await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["error"]["message"],
        json!(
            "request _meta is missing or has malformed required fields: \
             io.modelcontextprotocol/protocolVersion, \
             io.modelcontextprotocol/clientCapabilities"
        ),
        "{refused}"
    );
}

/// The tool list does not move across a `configure` that flips a setting the
/// list used to depend on, and both readings carry their hints.
///
/// `skills.serve` is the case: it is `configure`-settable and it once shaped
/// three lists live, so a flip moved them on the very connection that made the
/// call. The effective value is snapshotted while the engine is built
/// (`Engine::skills_serve`), so the write applies at the next daemon start and
/// nothing on this connection moves. `github.enabled` is deliberately not this
/// test's subject - that one does move the list, on purpose, and
/// `enabling_github_through_configure_makes_the_five_appear_on_the_next_list`
/// below is where it is pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_list_does_not_move_across_a_configure_for_a_modern_client() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let before = wire.open(modern(1, "tools/list", json!({}))).await;
    assert_hinted("tools/list", &before["result"]);

    let configured = wire
        .call(modern(
            2,
            "tools/call",
            json!({ "name": "configure", "arguments": { "set": { "skills.serve": "false" } } }),
        ))
        .await;
    let text = configured["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    let snapshot: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let written = snapshot["settings"]
        .as_array()
        .unwrap_or_else(|| panic!("no settings snapshot in {configured}"))
        .iter()
        .find(|s| s["key"] == json!("skills.serve"))
        .unwrap_or_else(|| panic!("skills.serve is not in the snapshot: {configured}"))
        .clone();
    assert_eq!(
        written["value"],
        json!("false"),
        "the write has to land, or the invariance below proves nothing: {configured}"
    );

    let after = wire.call(modern(3, "tools/list", json!({}))).await;
    assert_eq!(
        before["result"]["tools"], after["result"]["tools"],
        "the list is the same on both sides of a configure"
    );
    assert_hinted("tools/list", &after["result"]);
}

/// A modern write records who asked for it, read from
/// `_meta.io.modelcontextprotocol/clientInfo`.
///
/// There is no handshake in this era, so `ctx.peer.peer_info()` carries
/// rmcp's synthesized `Implementation::default()` with an empty name; without
/// the `_meta` read every modern write would fall back to the generic actor.
/// Reading `clientInfo` for provenance is what the specification intends it
/// for - what it forbids is changing *behaviour* on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_write_records_the_client_from_its_request_metadata() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let written = wire
        .open(modern(
            1,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Provenance",
                    "content": "Who wrote this.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the write succeeds: {written}"
    );

    let text = std::fs::read_to_string(h.root.join("eng/provenance.md")).unwrap();
    assert!(
        text.contains("generated: { by: modern-era-test/9.9.9, at: "),
        "the request metadata identity reaches generated.by: {text}"
    );
}

// --- streamable HTTP --------------------------------------------------------

/// One raw HTTP/1.1 POST, read for a bounded window. Mirrors
/// `tests/http_stream.rs`'s helper rather than sharing it, because an
/// integration test binary cannot reach another one's helpers.
async fn post(
    addr: std::net::SocketAddr,
    body: &str,
    version: Option<&str>,
    method: Option<&str>,
    name: Option<&str>,
    session_id: Option<&str>,
) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Connection: close\r\n"
        .to_string();
    if let Some(version) = version {
        head.push_str(&format!("MCP-Protocol-Version: {version}\r\n"));
    }
    if let Some(method) = method {
        head.push_str(&format!("Mcp-Method: {method}\r\n"));
    }
    if let Some(name) = name {
        head.push_str(&format!("Mcp-Name: {name}\r\n"));
    }
    if let Some(id) = session_id {
        head.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    let request = format!("{head}Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = stream.write_all(request.as_bytes()).await;
    let _ = stream.flush().await;

    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// A modern POST: the era's `_meta` in the body and the SEP-2243 standard
/// headers beside it, which rmcp requires from any client declaring this
/// revision (`validate_standard_headers`, `tower.rs:673-700`).
async fn modern_post(addr: std::net::SocketAddr, id: u32, method: &str, params: Value) -> String {
    let name = params
        .get("name")
        .or_else(|| params.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let body = modern(id, method, params).to_string();
    post(addr, &body, Some(ERA), Some(method), name.as_deref(), None).await
}

/// [`modern_post`] from a client that can put a question to its user: the same
/// headers, with the elicitation capability declared in the body's `_meta`.
async fn eliciting_post(
    addr: std::net::SocketAddr,
    id: u32,
    method: &str,
    params: Value,
) -> String {
    let name = params
        .get("name")
        .or_else(|| params.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let body = eliciting(id, method, params).to_string();
    post(addr, &body, Some(ERA), Some(method), name.as_deref(), None).await
}

fn head_of(raw: &str) -> String {
    raw.split("\r\n\r\n").next().unwrap_or(raw).to_string()
}

/// The first JSON-RPC payload in an SSE-framed or plain-JSON response.
fn payload(raw: &str) -> Value {
    let line = raw
        .lines()
        .map(|line| line.strip_prefix("data: ").unwrap_or(line).trim())
        .find(|line| line.starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON-RPC payload in:\n{raw}"));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("payload is not JSON ({e}):\n{line}"))
}

fn has_session_header(raw: &str) -> bool {
    raw.split("\r\n").any(|line| {
        line.split(':')
            .next()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("mcp-session-id"))
    })
}

/// **A modern peer is served over HTTP without a session, with its hints.**
///
/// No `Mcp-Session-Id` is presented and none is returned: rmcp inserts that
/// header at one site (`tower.rs:1911`) inside the legacy session branch, and
/// a request at this revision never reaches it. That is the deployment fact
/// SEP-2575 exists for - a team server behind a load balancer needs no session
/// affinity for these clients.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_request_over_http_is_served_statelessly_with_its_hints() {
    let h = Harness::new().await;
    let addr = h.http().await;

    for (id, method, params) in [
        (1, "tools/list", json!({})),
        (2, "prompts/list", json!({})),
        (3, "resources/list", json!({})),
        (4, "resources/templates/list", json!({})),
        (
            5,
            "resources/read",
            json!({ "uri": "skill://crystalline-routing/SKILL.md" }),
        ),
    ] {
        let raw = modern_post(addr, id, method, params).await;
        assert!(
            raw.starts_with("HTTP/1.1 200 OK"),
            "{method} is served at {ERA}:\n{}",
            head_of(&raw)
        );
        assert!(
            !has_session_header(&raw),
            "{method} gets no session id:\n{}",
            head_of(&raw)
        );
        let answer = payload(&raw);
        assert!(answer["error"].is_null(), "{method}: {answer}");
        assert_hinted(method, &answer["result"]);
    }
}

/// `server/discover` over HTTP answers the routing block. rmcp takes any
/// discover request on the stateless path (`tower.rs:1822`), so this is the
/// onboarding channel for a remote modern client - and it is a different code
/// path from the stdio one, which is why it is asserted separately.
///
/// A **bare** probe over HTTP is still the `422` `tests/http_stream.rs` pins:
/// it carries no `_meta`, so it is classified legacy and takes the session
/// branch. The era changes nothing about that, and the stdio bridge's
/// normalization is what closes it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_over_http_answers_the_routing_block() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let raw = modern_post(addr, 1, "server/discover", json!({})).await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "discover is served:\n{}",
        head_of(&raw)
    );
    assert!(!has_session_header(&raw), "and needs no session");
    let answer = payload(&raw);
    assert!(
        answer["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
        "{answer}"
    );
}

/// **The HTTP subscription path, running for the first time.**
///
/// `subscriptions/listen` is refused `method not found` while a request is
/// classified legacy (`handler/server.rs:147-150`), and nothing on the
/// streamable-HTTP path arms the metadata latch, so before the era was
/// advertised this request could not be anything but legacy there. Now it can.
///
/// The two server MUSTs are rmcp's (`SubscriptionContext::establish`,
/// `service/server.rs:337-375`): the acknowledgment is the first message on
/// the stream and it carries the subscription id in `_meta`. Nothing follows
/// it here because nothing moved a list during this request - the one mover is
/// a `configure` flipping `github.enabled`, and
/// `tests/mcp_subscriptions.rs::a_subscribed_client_is_told_when_the_tool_list_moves`
/// is where the announcement itself is pinned. This POST reads its stream once
/// and returns, so a second connection would be needed to make the flip land
/// while it is open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_http_subscription_stream_acknowledges_first_and_stays_silent() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let raw = post(
        addr,
        &modern(
            1,
            "subscriptions/listen",
            json!({ "notifications": { "toolsListChanged": true } }),
        )
        .to_string(),
        Some(ERA),
        Some("subscriptions/listen"),
        None,
        None,
    )
    .await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "the subscription stream opens:\n{}",
        head_of(&raw)
    );
    assert!(!has_session_header(&raw), "and it is sessionless");

    let events: Vec<Value> = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line.trim()).unwrap())
        .collect();
    let first = events
        .first()
        .unwrap_or_else(|| panic!("no SSE event in:\n{raw}"));
    assert_eq!(
        first["method"],
        json!("notifications/subscriptions/acknowledged"),
        "the acknowledgment is the first message: {first}"
    );
    assert_eq!(
        first["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        json!(1),
        "and it names the subscription: {first}"
    );
    assert!(
        events
            .iter()
            .skip(1)
            .all(|event| event["method"] != json!("notifications/tools/list_changed")),
        "nothing is pushed on the stream: {events:?}"
    );
}

/// **What a legacy-shaped handshake naming the era gets, recorded because it
/// is the one shape the era leaves ragged.**
///
/// Before this task an HTTP `initialize` declaring 2026-07-28 was refused
/// `-32022`, because we did not serve the revision. Now it is served and
/// echoed - and it gets **no session id**, because `is_legacy_request` routed
/// it statelessly from the version in its own body (`tower.rs:358-408`,
/// `:1727`) before any handler ran. A client that goes on to speak the era's
/// request shape works; a client that sends a bare follow-up is asking for the
/// session branch, has no session to present, and gets rmcp's
/// `422 Unexpected message, expect initialize request`.
///
/// That is the era's session model rather than a wedge this task introduced:
/// the handshake is deleted from the 2026-07-28 schema, so a client using it
/// while declaring that revision is contradicting itself. Pinned here so the
/// behaviour is known rather than discovered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handshake_declaring_the_era_is_served_and_gets_no_session() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": ERA,
            "capabilities": {},
            "clientInfo": { "name": "modern-era-test", "version": "9.9.9" },
        },
    })
    .to_string();
    let raw = post(addr, &body, None, None, None, None).await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "the era is no longer refused at the handshake:\n{}",
        head_of(&raw)
    );
    let answer = payload(&raw);
    assert_eq!(answer["result"]["protocolVersion"], json!(ERA), "{answer}");
    assert!(
        !has_session_header(&raw),
        "a modern peer is sessionless:\n{}",
        head_of(&raw)
    );

    // The era's own request shape works on the same endpoint, with no session.
    let served = modern_post(addr, 2, "tools/list", json!({})).await;
    assert!(served.starts_with("HTTP/1.1 200 OK"));
    assert!(payload(&served)["result"]["tools"].is_array());

    // A legacy-shaped follow-up asks for the session branch there is none of.
    let bare = post(
        addr,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(
        bare.starts_with("HTTP/1.1 422 Unprocessable Entity"),
        "a bare follow-up has no session to present:\n{}",
        head_of(&bare)
    );
}

// --- the confirmation round (SEP-2322 MRTR) ---------------------------------
//
// `delete_engram` is the first tool that answers a round of its own. The gate
// is two-sided and both sides are proved here: the peer must be on this era
// *and* must have declared that it can put a question to its user. A peer
// failing either half is served exactly what 0.15.0 served it, which is the
// contrast the last two tests carry.

/// The arguments that delete the engram the helper below writes.
fn delete_doomed(responses: Option<Value>) -> Value {
    let mut params = json!({
        "name": "delete_engram",
        "arguments": { "domain": "eng", "identifier": "doomed" },
    });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The client's answer to the `confirm` question, as an `ElicitResult`.
fn answer(action: &str, confirm: bool) -> Value {
    json!({ "confirm": { "action": action, "content": { "confirm": confirm } } })
}

/// Open a modern connection by writing the engram the delete tests kill, and
/// hand back both the connection and the file that must still be there.
async fn doomed_engram(h: &Harness) -> (Wire, std::path::PathBuf) {
    let mut wire = h.stdio().await;
    let written = wire
        .open(modern(
            1,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Doomed",
                    "content": "Delete me.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the engram to delete has to exist first: {written}"
    );
    let path = h.root.join("eng/doomed.md");
    assert!(path.exists(), "the write landed on disk");
    (wire, path)
}

/// Round one: an eliciting modern peer is asked before anything is deleted.
///
/// The whole point is the negative half of the assertion - the file is still
/// there when the question comes back - because an `input_required` result
/// that had already deleted the engram would be a confirmation in name only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_eliciting_delete_gets_a_confirmation_question_first() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let asked = wire
        .call(eliciting(2, "tools/call", delete_doomed(None)))
        .await;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"],
        json!("input_required"),
        "the call answers with a round rather than a deletion: {asked}"
    );

    let question = &result["inputRequests"]["confirm"];
    assert_eq!(
        question["method"],
        json!("elicitation/create"),
        "the round is an elicitation keyed `confirm`: {asked}"
    );
    let schema = &question["params"]["requestedSchema"];
    assert_eq!(
        schema["properties"]["confirm"]["type"],
        json!("boolean"),
        "one boolean property: {asked}"
    );
    assert_eq!(
        schema["required"],
        json!(["confirm"]),
        "and it is required: {asked}"
    );

    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("eng/doomed"),
        "the question names what dies: {message}"
    );
    assert!(
        message.contains("cannot be undone"),
        "and says so plainly: {message}"
    );

    assert!(
        path.exists(),
        "round one deletes nothing: {}",
        path.display()
    );
}

/// Round two with a yes: the same call, now carrying the answer, deletes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_delete_round_two_deletes() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let asked = wire
        .call(eliciting(2, "tools/call", delete_doomed(None)))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let done = wire
        .call(eliciting(
            3,
            "tools/call",
            delete_doomed(Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the confirmed round deletes: {done}"
    );
    assert_eq!(
        done["result"]["resultType"],
        json!("complete"),
        "and it is an ordinary complete result: {done}"
    );
    assert!(!path.exists(), "the engram is gone: {}", path.display());
}

/// Round two with a no: the call refuses and the engram survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_delete_round_two_deletes_nothing() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let asked = wire
        .call(eliciting(2, "tools/call", delete_doomed(None)))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let refused = wire
        .call(eliciting(
            3,
            "tools/call",
            delete_doomed(Some(answer("decline", false))),
        ))
        .await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a decline is a refusal the model can read: {refused}"
    );
    let text = refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("nothing was deleted"),
        "and it says what did not happen: {text}"
    );
    assert!(
        path.exists(),
        "the engram survives a decline: {}",
        path.display()
    );
}

/// A modern peer that declared no elicitation capability gets 0.15.0's
/// behaviour: the delete happens on the first call, with no round at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_peer_without_elicitation_deletes_immediately() {
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;

    let done = wire
        .call(modern(2, "tools/call", delete_doomed(None)))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the delete runs on the first call: {done}"
    );
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a peer that cannot answer a question is not asked one: {done}"
    );
    assert!(!path.exists(), "the engram is gone: {}", path.display());
}

/// A legacy peer is served byte for byte what it was before: no `resultType`
/// at all, and the delete on the first call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_deletes_immediately_with_no_input_required() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    let written = wire
        .call(request(
            2,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Doomed",
                    "content": "Delete me.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the write lands: {written}"
    );
    let path = h.root.join("eng/doomed.md");
    assert!(path.exists());

    let done = wire
        .call(request(3, "tools/call", delete_doomed(None)))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the delete runs on the first call: {done}"
    );
    assert!(
        !done["result"]
            .as_object()
            .unwrap()
            .contains_key("resultType"),
        "a legacy result carries no discriminator: {done}"
    );
    assert!(!path.exists(), "the engram is gone: {}", path.display());
}

/// **A peer outside the gate is not half-served: its answers are not read at
/// all.**
///
/// Both excluded shapes can put an `inputResponses` object on the wire - the
/// field is ordinary `tools/call` parameters at every revision rmcp parses - so
/// "the gate is two-sided" has a second, quieter half worth pinning: a peer the
/// gate excludes is served exactly what 0.15.0 served it, and a decline it was
/// never asked for changes nothing. The alternative shape, reading the answer
/// whenever one is present, would let a client that cannot be asked veto its own
/// calls, which is a different contract than the one shipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_from_a_gate_excluded_peer_are_ignored() {
    // A modern peer that declared no elicitation capability.
    let h = Harness::new().await;
    let (mut wire, path) = doomed_engram(&h).await;
    let done = wire
        .call(modern(
            2,
            "tools/call",
            delete_doomed(Some(answer("decline", false))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the unasked-for decline is not read as a refusal: {done}"
    );
    assert!(
        !path.exists(),
        "and the delete ran on the first call: {}",
        path.display()
    );

    // A legacy peer, which cannot be asked whatever it declares.
    let h = Harness::new().await;
    let mut wire = h.stdio().await;
    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));
    let written = wire
        .call(request(
            2,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Doomed",
                    "content": "Delete me.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the write lands: {written}"
    );
    let path = h.root.join("eng/doomed.md");
    assert!(path.exists());

    let done = wire
        .call(request(
            3,
            "tools/call",
            delete_doomed(Some(answer("decline", false))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "a legacy answer is not read either: {done}"
    );
    assert!(
        !path.exists(),
        "and the legacy delete is unchanged: {}",
        path.display()
    );
}

/// **An eliciting peer keeps every delete a legacy peer has**, including the
/// one this verb exists as an escape hatch for: a stray file above the
/// attachment ceiling, which the walker skips and so never gives a row.
///
/// Round one has to succeed before a user can be asked anything, so a preview
/// that refused an over-cap file would not merely lose the byte count - it
/// would refuse the delete outright, and only for the clients that ask before
/// destroying. The whole round trip is driven here rather than the question
/// alone, because "asks, then cannot act" would be the same regression one
/// step later.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_peer_can_still_delete_an_over_cap_attachment() {
    let h = Harness::new().await;
    let assets = h.root.join("eng/assets");
    std::fs::create_dir_all(&assets).unwrap();
    let over_cap = crystalline_core::MAX_ATTACHMENT_BYTES + 1;
    let stray = assets.join("big.png");
    std::fs::write(&stray, vec![0u8; over_cap as usize]).unwrap();

    let mut wire = h.stdio().await;
    let call = json!({
        "name": "delete_engram",
        "arguments": { "domain": "eng", "identifier": "assets/big.png" },
    });

    let asked = wire.open(eliciting(1, "tools/call", call.clone())).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "the oversized file is previewable, so it is asked about: {asked}"
    );
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("assets/big.png") && message.contains(&format!("{over_cap} bytes")),
        "the question names the file and its real size: {message}"
    );
    assert!(stray.exists(), "round one deletes nothing");

    let mut confirmed = call;
    confirmed["inputResponses"] = answer("accept", true);
    let done = wire.call(eliciting(2, "tools/call", confirmed)).await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "and the confirmed round removes it: {done}"
    );
    assert!(
        !stray.exists(),
        "the stray file is gone: {}",
        stray.display()
    );
}

/// **The confirmation round exists on the HTTP wire too, not only over stdio.**
///
/// Every other test of the round drives the duplex stdio transport, where the
/// capability reaches the handler through rmcp's metadata latch. Nothing arms
/// that latch on the streamable-HTTP path, so a request there is classified from
/// its own `_meta` on each call - a genuinely different route to
/// `client_capabilities`, and the one a remote client actually takes. Pinned
/// here so a deployment behind a load balancer is known to ask before it
/// destroys rather than assumed to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_peer_gets_a_question_over_http() {
    let h = Harness::new().await;
    let addr = h.http().await;

    let written = modern_post(
        addr,
        1,
        "tools/call",
        json!({
            "name": "write_engram",
            "arguments": {
                "domain": "eng",
                "title": "Doomed",
                "content": "Delete me.",
            },
        }),
    )
    .await;
    assert!(
        written.starts_with("HTTP/1.1 200 OK"),
        "the engram to delete has to exist first:\n{}",
        head_of(&written)
    );
    let path = h.root.join("eng/doomed.md");
    assert!(path.exists(), "the write landed on disk");

    let raw = eliciting_post(addr, 2, "tools/call", delete_doomed(None)).await;
    assert!(
        raw.starts_with("HTTP/1.1 200 OK"),
        "the round is served:\n{}",
        head_of(&raw)
    );
    assert!(
        !has_session_header(&raw),
        "and it needs no session to carry it:\n{}",
        head_of(&raw)
    );
    let asked = payload(&raw);
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "the call answers with a round rather than a deletion: {asked}"
    );
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("eng/doomed"),
        "the question names what dies: {message}"
    );
    assert!(
        path.exists(),
        "round one deletes nothing: {}",
        path.display()
    );
}

// --- acknowledgments, recorded and taken back -------------------------------
//
// `set_frontmatter` on `evolve_ack` is the second act that asks first: it
// silences a finding, or resurfaces one, on the user's behalf. The gate is the
// same two-sided one the delete tests prove, and it arms for this one key
// only - every other `edit_engram` operation runs on the first call, whatever
// the peer.

/// One `edit_engram` call assigning `value` to `evolve_ack` on `acked`.
fn ack_call(value: &str, responses: Option<Value>) -> Value {
    ack_call_on("acked", value, responses)
}

/// The same call aimed at `identifier`, for the tests that care what round one
/// resolves before it asks.
fn ack_call_on(identifier: &str, value: &str, responses: Option<Value>) -> Value {
    let mut params = json!({
        "name": "edit_engram",
        "arguments": {
            "domain": "eng",
            "identifier": identifier,
            "operation": "set_frontmatter",
            "key": "evolve_ack",
            "value": value,
        },
    });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The engine payload a tool result carries, as JSON.
fn payload_of(answer: &Value) -> Value {
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// Open a connection by writing the engram the acknowledgment tests annotate.
async fn acked_engram(h: &Harness) -> (Wire, std::path::PathBuf) {
    let mut wire = h.stdio().await;
    let written = wire
        .open(modern(
            1,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Acked",
                    "content": "A finding will be ruled intentional here.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the engram to acknowledge has to exist first: {written}"
    );
    let path = h.root.join("eng/acked.md");
    assert!(path.exists(), "the write landed on disk");
    (wire, path)
}

/// Round one of a record: the peer is asked, and nothing is written yet.
///
/// The call names the engram by its title, so the question can only carry the
/// permalink if round one resolved the engram before asking - which is the
/// point: a user confirms the engram the write would land on, not the string
/// the model typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_ack_gets_a_confirmation_question() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            ack_call_on("Acked", "V101 lineage citation, keep", None),
        ))
        .await;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"],
        json!("input_required"),
        "the call answers with a round rather than an acknowledgment: {asked}"
    );
    let question = &result["inputRequests"]["confirm"];
    assert_eq!(question["method"], json!("elicitation/create"), "{asked}");

    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Acknowledge V101"),
        "the question names the rule: {message}"
    );
    assert!(
        message.contains("'acked'") && message.contains("'eng'"),
        "and the engram it lands on, by resolved permalink: {message}"
    );
    assert!(
        message.contains("lineage citation, keep"),
        "and the note it would record: {message}"
    );

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        !on_disk.contains("evolve_ack"),
        "round one records nothing: {on_disk}"
    );
}

/// Round one resolves before it asks: a call naming an engram nobody has fails
/// in round one rather than putting a question about it to the user.
///
/// The seam this closes is a user confirming an acknowledgment against a
/// mistyped identifier and only round two reporting that there was nothing
/// there - a yes given to a question that was never answerable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_ack_round_one_resolves_before_asking() {
    let h = Harness::new().await;
    let (mut wire, _path) = acked_engram(&h).await;

    let answered = wire
        .call(eliciting(
            2,
            "tools/call",
            ack_call_on("no-such-engram", "V101 lineage citation, keep", None),
        ))
        .await;
    assert_ne!(
        answered["result"]["resultType"],
        json!("input_required"),
        "an unresolvable identifier is never put to a user: {answered}"
    );
    assert!(
        !answered["error"].is_null(),
        "it errors in round one instead: {answered}"
    );
    let message = answered["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("no-such-engram"),
        "and the error names what could not be found: {message}"
    );
}

/// The same for a take-back, which resolves on the same path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_unack_round_one_resolves_before_asking() {
    let h = Harness::new().await;
    let (mut wire, _path) = acked_engram(&h).await;

    let answered = wire
        .call(eliciting(
            2,
            "tools/call",
            ack_call_on("no-such-engram", "remove V101", None),
        ))
        .await;
    assert_ne!(
        answered["result"]["resultType"],
        json!("input_required"),
        "an unresolvable identifier is never put to a user: {answered}"
    );
    assert!(
        !answered["error"].is_null(),
        "it errors in round one instead: {answered}"
    );
}

/// The whole take-back: acknowledge, then confirm a removal, and the receipt
/// names the rule it resurfaced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_unack_removes_and_reports_evolve_ack_removed() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;

    // Recorded by a peer that cannot be asked, so the removal is the only
    // round in play.
    let recorded = wire
        .call(modern(2, "tools/call", ack_call("V101 keep it", None)))
        .await;
    assert!(
        recorded["error"].is_null() && recorded["result"]["isError"] != json!(true),
        "the acknowledgment lands: {recorded}"
    );
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("V101"),
        "it is in the file"
    );

    let asked = wire
        .call(eliciting(3, "tools/call", ack_call("remove V101", None)))
        .await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "a take-back is asked about too: {asked}"
    );
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains("Remove the V101 acknowledgment"),
        "the question names what goes: {message}"
    );
    assert!(
        message.contains("resurfaces"),
        "and what comes back: {message}"
    );
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("V101"),
        "round one removes nothing"
    );

    let done = wire
        .call(eliciting(
            4,
            "tools/call",
            ack_call("remove V101", Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the confirmed round removes it: {done}"
    );
    assert_eq!(
        payload_of(&done)["evolve_ack_removed"],
        json!("V101"),
        "and the receipt names the rule: {done}"
    );
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("evolve_ack"),
        "the entry is gone from the file"
    );
}

/// Round two of a record with a yes: the entry lands, receipt and file alike.
///
/// The take-back has had its whole round trip pinned since it shipped; the
/// record only ever had its question and its refusal, so the one path that
/// actually writes on a confirmed round was covered by inference.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_ack_record_lands_on_round_two() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            ack_call("V101 lineage citation, keep", None),
        ))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("evolve_ack"),
        "round one records nothing"
    );

    let done = wire
        .call(eliciting(
            3,
            "tools/call",
            ack_call("V101 lineage citation, keep", Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the confirmed round records: {done}"
    );
    assert_eq!(
        done["result"]["resultType"],
        json!("complete"),
        "and it is an ordinary complete result: {done}"
    );
    let entry = &payload_of(&done)["evolve_ack"];
    assert_eq!(entry["rule"], json!("V101"), "{done}");
    assert_eq!(entry["note"], json!("lineage citation, keep"), "{done}");

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains("evolve_ack") && on_disk.contains("V101"),
        "and it is in the frontmatter: {on_disk}"
    );
}

/// Round two of a record with a no: nothing is written, byte for byte, and the
/// refusal says which half of the verb did not happen.
///
/// The two refusals are worded apart on purpose - a declined record leaves
/// nothing behind, a declined removal leaves the entry standing - so this
/// asserts the record's wording rather than merely that something was refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_ack_record_changes_nothing() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;
    let before = std::fs::read(&path).unwrap();

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            ack_call("V101 lineage citation, keep", None),
        ))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let refused = wire
        .call(eliciting(
            3,
            "tools/call",
            ack_call(
                "V101 lineage citation, keep",
                Some(answer("decline", false)),
            ),
        ))
        .await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a decline is a refusal the model can read: {refused}"
    );
    let text = refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("nothing was recorded"),
        "and it says what did not happen, in the record's own words: {text}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the engram is untouched byte for byte: {}",
        path.display()
    );
}

/// **A yes to a removal whose acknowledgment vanished between the rounds fails
/// loudly, naming the rule.**
///
/// The confirmation round is not a transaction: round one resolves the engram
/// and asks, round two acts on whatever the file holds by then. Something else -
/// another agent, a Fluid tab, a CLI run - can take the acknowledgment away
/// while the user is being asked, and the honest outcome is the engine's own
/// "nothing to remove", carrying the rule id so the model can tell which
/// acknowledgment it was. What must not happen is the round silently succeeding
/// on an engram that no longer carries the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_removal_of_a_vanished_ack_errors_naming_the_rule() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;

    // Recorded by a peer that cannot be asked, so only the removal has rounds.
    let recorded = wire
        .call(modern(2, "tools/call", ack_call("V101 keep it", None)))
        .await;
    assert!(
        recorded["error"].is_null() && recorded["result"]["isError"] != json!(true),
        "the acknowledgment lands: {recorded}"
    );

    let asked = wire
        .call(eliciting(3, "tools/call", ack_call("remove V101", None)))
        .await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "round one resolves the engram and asks: {asked}"
    );

    // Out of band, while the user is being asked: the acknowledgment goes. A
    // modern non-eliciting take-back is the shortest stand-in for the other
    // agent or Fluid tab that would do it, and it rewrites the file the way any
    // real removal does rather than leaving a half-edited one behind.
    let removed = wire
        .call(modern(4, "tools/call", ack_call("remove V101", None)))
        .await;
    assert_eq!(
        payload_of(&removed)["evolve_ack_removed"],
        json!("V101"),
        "the entry is genuinely gone: {removed}"
    );
    let before = std::fs::read(&path).unwrap();

    let answered = wire
        .call(eliciting(
            5,
            "tools/call",
            ack_call("remove V101", Some(answer("accept", true))),
        ))
        .await;
    assert_ne!(
        answered["result"]["resultType"],
        json!("input_required"),
        "the answered round does not ask again: {answered}"
    );
    assert!(
        !answered["error"].is_null(),
        "a removal with nothing to remove is an error, not a quiet success: {answered}"
    );
    let message = answered["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("V101"),
        "and it names the rule the user said yes about: {message}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file is untouched by the failed round: {}",
        path.display()
    );
}

/// A declined take-back leaves the acknowledgment exactly where it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_unack_keeps_the_acknowledgment() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;

    wire.call(modern(2, "tools/call", ack_call("V101 keep it", None)))
        .await;
    let refused = wire
        .call(eliciting(
            3,
            "tools/call",
            ack_call("remove V101", Some(answer("decline", false))),
        ))
        .await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a decline is a refusal the model can read: {refused}"
    );
    let text = refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("still there"),
        "and it says what did not happen: {text}"
    );
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("V101"),
        "the acknowledgment survives"
    );
}

/// Every other `edit_engram` operation is untouched: an eliciting peer editing
/// a different frontmatter key is served on the first call, with no round.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_peer_edits_every_other_key_immediately() {
    let h = Harness::new().await;
    let (mut wire, path) = acked_engram(&h).await;

    let done = wire
        .call(eliciting(
            2,
            "tools/call",
            json!({
                "name": "edit_engram",
                "arguments": {
                    "domain": "eng",
                    "identifier": "acked",
                    "operation": "set_frontmatter",
                    "key": "status",
                    "value": "deprecated",
                },
            }),
        ))
        .await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "only the acknowledgment key asks: {done}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("status: deprecated"),
        "the edit landed"
    );
}

/// A legacy peer acknowledges on the first call and gets 0.15.0's receipt: the
/// `evolve_ack` entry, no discriminator, no round.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_acks_immediately() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    let written = wire
        .call(request(
            2,
            "tools/call",
            json!({
                "name": "write_engram",
                "arguments": {
                    "domain": "eng",
                    "title": "Acked",
                    "content": "A finding will be ruled intentional here.",
                },
            }),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the write lands: {written}"
    );

    let done = wire
        .call(request(
            3,
            "tools/call",
            ack_call("V101 lineage citation, keep", None),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the acknowledgment runs on the first call: {done}"
    );
    assert!(
        !done["result"]
            .as_object()
            .unwrap()
            .contains_key("resultType"),
        "a legacy result carries no discriminator: {done}"
    );
    let entry = &payload_of(&done)["evolve_ack"];
    assert_eq!(entry["rule"], json!("V101"), "{done}");
    assert_eq!(entry["note"], json!("lineage citation, keep"), "{done}");

    // And the take-back is open to it too, prose-guided rather than asked.
    let removed = wire
        .call(request(4, "tools/call", ack_call("remove V101", None)))
        .await;
    assert_eq!(
        payload_of(&removed)["evolve_ack_removed"],
        json!("V101"),
        "{removed}"
    );
    assert!(
        !std::fs::read_to_string(h.root.join("eng/acked.md"))
            .unwrap()
            .contains("evolve_ack"),
        "the entry is gone"
    );
}

// --- a permalink collision, resolved rather than reported -------------------
//
// `write_engram` is the third tool that answers a round, and the first whose
// question is not a yes-or-no: the engine's collision error names a real
// choice, so an eliciting peer is offered it as a single-select rather than
// handed the error and left to reconstruct `overwrite=true` from prose. The
// gate is the same two-sided one the delete round proves, plus a third
// condition of its own - the call did not already ask for an overwrite.

/// The body the first write lands, and the body every collision tries to land
/// over it. Different bytes, so "did the overwrite happen" is readable off
/// disk rather than inferred from a receipt.
const FIRST_BODY: &str = "The first body.";
const SECOND_BODY: &str = "The second body.";

/// One `write_engram` call landing `content` under the title `Taken`, with an
/// optional explicit overwrite and an optional answer to the round.
fn write_taken(content: &str, overwrite: bool, responses: Option<Value>) -> Value {
    let mut arguments = json!({
        "domain": "eng",
        "title": "Taken",
        "content": content,
    });
    if overwrite {
        arguments["overwrite"] = json!(true);
    }
    let mut params = json!({ "name": "write_engram", "arguments": arguments });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The client's answer to the `resolution` question, as an `ElicitResult`.
fn resolution(action: &str, choice: &str) -> Value {
    json!({ "resolution": { "action": action, "content": { "resolution": choice } } })
}

/// Open a modern connection by writing the engram every collision below lands
/// on, and hand back the connection and the file that must survive a cancel.
///
/// The opener is a plain modern write rather than an eliciting one so the
/// first write is never itself a round: nothing exists to collide with yet.
async fn taken_engram(h: &Harness) -> (Wire, std::path::PathBuf) {
    let mut wire = h.stdio().await;
    let written = wire
        .open(modern(
            1,
            "tools/call",
            write_taken(FIRST_BODY, false, None),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the engram to collide with has to exist first: {written}"
    );
    let path = h.root.join("eng/taken.md");
    assert!(path.exists(), "the write landed on disk");
    (wire, path)
}

/// Round one: the collision comes back as a choice, and nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collision_on_an_eliciting_peer_offers_overwrite_or_cancel() {
    let h = Harness::new().await;
    let (mut wire, path) = taken_engram(&h).await;
    let before = std::fs::read(&path).unwrap();

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            write_taken(SECOND_BODY, false, None),
        ))
        .await;
    let result = &asked["result"];
    assert_eq!(
        result["resultType"],
        json!("input_required"),
        "the call answers with a round rather than the bare error: {asked}"
    );

    let question = &result["inputRequests"]["resolution"];
    assert_eq!(
        question["method"],
        json!("elicitation/create"),
        "the round is an elicitation keyed `resolution`: {asked}"
    );
    let schema = &question["params"]["requestedSchema"];
    assert_eq!(
        schema["required"],
        json!(["resolution"]),
        "and the one property is required: {asked}"
    );
    let property = &schema["properties"]["resolution"];
    assert_eq!(property["type"], json!("string"), "a string enum: {asked}");
    let options: Vec<&str> = property["oneOf"]
        .as_array()
        .expect("a titled single-select carries oneOf")
        .iter()
        .map(|option| option["const"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        options,
        vec!["overwrite", "cancel"],
        "exactly the two choices, in that order: {asked}"
    );
    for option in property["oneOf"].as_array().unwrap() {
        let title = option["title"].as_str().unwrap_or_default();
        assert!(
            !title.is_empty(),
            "each option is titled for a human to read: {asked}"
        );
    }

    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("'Taken'"),
        "the question names the title that would land: {message}"
    );
    assert!(
        message.contains("'taken'"),
        "and the permalink it would land at: {message}"
    );
    assert!(
        message.contains("'eng'"),
        "and the domain it collides in: {message}"
    );
    assert!(
        message.contains("Overwrite it, or cancel?"),
        "and puts the choice plainly: {message}"
    );

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "round one writes nothing: {}",
        path.display()
    );
}

/// Round two with `overwrite`: the same call replaces the existing engram.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choosing_overwrite_replaces_the_engram() {
    let h = Harness::new().await;
    let (mut wire, path) = taken_engram(&h).await;

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            write_taken(SECOND_BODY, false, None),
        ))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let done = wire
        .call(eliciting(
            3,
            "tools/call",
            write_taken(SECOND_BODY, false, Some(resolution("accept", "overwrite"))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the resolved round writes: {done}"
    );
    assert_eq!(
        done["result"]["resultType"],
        json!("complete"),
        "and it is an ordinary complete result: {done}"
    );

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        on_disk.contains(SECOND_BODY) && !on_disk.contains(FIRST_BODY),
        "the new body replaced the old one: {on_disk}"
    );
}

/// Round two with `cancel`: the call refuses and the engram is untouched, byte
/// for byte. A decline - the other way a client says no - is the same answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choosing_cancel_leaves_the_engram() {
    let h = Harness::new().await;
    let (mut wire, path) = taken_engram(&h).await;
    let before = std::fs::read(&path).unwrap();

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            write_taken(SECOND_BODY, false, None),
        ))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    let refused = wire
        .call(eliciting(
            3,
            "tools/call",
            write_taken(SECOND_BODY, false, Some(resolution("accept", "cancel"))),
        ))
        .await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a cancel is a refusal the model can read: {refused}"
    );
    let text = refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("the existing engram was left in place; nothing was written"),
        "and it says what did not happen: {text}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the engram survives a cancel byte for byte: {}",
        path.display()
    );

    // A declined question never carries a choice at all, and must read as a no
    // rather than as a missing answer that reopens round one.
    let declined = wire
        .call(eliciting(
            4,
            "tools/call",
            write_taken(SECOND_BODY, false, Some(resolution("decline", ""))),
        ))
        .await;
    assert_eq!(
        declined["result"]["isError"],
        json!(true),
        "a decline refuses too: {declined}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "and writes nothing either"
    );
}

/// **A cancel is a cancel even when the thing it was about is gone.**
///
/// The collision is discovered by attempting the write, so the shape that
/// suggests itself reads the answer off the engine's failure - and that shape
/// has a hole exactly here. Between the two rounds something else removes the
/// engram in the way; the round-two call no longer collides, so there is no
/// error to read the "cancel" off, and the write the user refused lands as an
/// ordinary success. Nothing about the wire says this went wrong, which is why
/// it is pinned rather than reasoned about: the handler reads the refusal
/// before it calls the engine, and this is the test that fails if it stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_still_refuses_when_the_collision_vanished_between_rounds() {
    let h = Harness::new().await;
    let (mut wire, path) = taken_engram(&h).await;

    let asked = wire
        .call(eliciting(
            2,
            "tools/call",
            write_taken(SECOND_BODY, false, None),
        ))
        .await;
    assert_eq!(asked["result"]["resultType"], json!("input_required"));

    // Out of band, while the user is being asked: something else takes the
    // engram away. A modern non-eliciting delete is the shortest stand-in for
    // the other agent, Fluid tab or CLI invocation that would do it, and it
    // clears the index row as well as the file - a bare unlink would leave the
    // row behind and the collision would simply persist, which would make this
    // test pass without ever reaching the case it is about.
    let deleted = wire
        .call(modern(
            3,
            "tools/call",
            json!({
                "name": "delete_engram",
                "arguments": { "domain": "eng", "identifier": "taken" },
            }),
        ))
        .await;
    assert!(
        deleted["error"].is_null() && deleted["result"]["isError"] != json!(true),
        "the engram in the way is removed: {deleted}"
    );
    assert!(!path.exists(), "the collision is genuinely gone");

    let refused = wire
        .call(eliciting(
            4,
            "tools/call",
            write_taken(SECOND_BODY, false, Some(resolution("accept", "cancel"))),
        ))
        .await;
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "the cancel still refuses, collision or no collision: {refused}"
    );
    let text = refused["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("the existing engram was left in place; nothing was written"),
        "with the same refusal: {text}"
    );
    assert!(
        !path.exists(),
        "and the write the user cancelled did not happen after all: {}",
        path.display()
    );
}

/// An eliciting peer that already asked for an overwrite is not asked again:
/// the caller answered the question before it was put.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_overwrite_never_elicits() {
    let h = Harness::new().await;
    let (mut wire, path) = taken_engram(&h).await;

    let done = wire
        .call(eliciting(
            2,
            "tools/call",
            write_taken(SECOND_BODY, true, None),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "the write runs on the first call: {done}"
    );
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a caller that asked for the overwrite is not asked about it: {done}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains(SECOND_BODY),
        "and the overwrite happened"
    );
}

/// A modern peer that declared no elicitation capability gets 0.15.0's
/// behaviour: the bare collision error, on the first call, with the hint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_peer_without_elicitation_gets_the_bare_collision_error() {
    let h = Harness::new().await;
    let (mut wire, path) = taken_engram(&h).await;
    let before = std::fs::read(&path).unwrap();

    let refused = wire
        .call(modern(
            2,
            "tools/call",
            write_taken(SECOND_BODY, false, None),
        ))
        .await;
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("already exists in domain")
            && message.contains("pass overwrite=true to replace"),
        "a peer that cannot answer a question is handed the error: {refused}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "and nothing was written"
    );
}

/// A legacy peer is served byte for byte what it was before, elicitation
/// capability or not: no round, and the collision error with its hint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_collision_still_errors_with_the_overwrite_hint() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    let written = wire
        .call(request(
            2,
            "tools/call",
            write_taken(FIRST_BODY, false, None),
        ))
        .await;
    assert!(
        written["error"].is_null() && written["result"]["isError"] != json!(true),
        "the first write lands: {written}"
    );
    let path = h.root.join("eng/taken.md");
    let before = std::fs::read(&path).unwrap();

    let refused = wire
        .call(request(
            3,
            "tools/call",
            write_taken(SECOND_BODY, false, None),
        ))
        .await;
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("already exists in domain")
            && message.contains("pass overwrite=true to replace"),
        "the collision is reported, not negotiated: {refused}"
    );
    assert!(
        refused["result"].is_null(),
        "and there is no round to carry: {refused}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the existing engram is untouched"
    );
}

// --- the share confirmation round (SEP-2322 MRTR) ---------------------------
//
// `share_changes` publishes to a place the user cannot take it back from
// unilaterally - a repository their team reviews - so the eliciting peer is
// asked what would be published before anything is. The gate is the same
// two-sided one `delete_engram` carries, and the same contrast legs prove
// both halves: a modern peer that declared no elicitation capability, and a
// legacy peer, are served exactly one round.

/// A share call's params, optionally carrying a round 2 answer.
fn share_kb(responses: Option<Value>) -> Value {
    let mut params = json!({
        "name": "share_changes",
        "arguments": { "domain": "kb" },
    });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// Write one engram straight into the team domain's working tree, the way a
/// person editing files beside the agent would.
fn write_kb_engram(h: &Harness, path: &str, title: &str, permalink: &str, body: &str) {
    std::fs::write(
        h.root.join("kb").join(path),
        format!(
            "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
        ),
    )
    .unwrap();
}

/// Edit one engram in the team domain so there is something to share.
fn edit_kb(h: &Harness) {
    write_kb_engram(h, "notes/a.md", "Alpha", "notes/a", "alpha, refined");
}

/// Edit one engram and put the domain's first proposal on the mock forge
/// through the real two-round flow, on a fresh eliciting connection. Returns
/// the open wire (ids 1 and 2 are spent), the proposal's number and its
/// branch, which is what a test needs to amend the branch behind the agent's
/// back.
async fn first_shared_proposal(h: &Harness) -> (Wire, u64, String) {
    edit_kb(h);
    let mut wire = h.stdio().await;
    let asked = wire.open(eliciting(1, "tools/call", share_kb(None))).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "{asked}"
    );
    let done = wire
        .call(eliciting(
            2,
            "tools/call",
            share_kb(Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "{done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "proposed", "{body}");
    (
        wire,
        body["number"].as_u64().unwrap(),
        body["branch"].as_str().unwrap().to_string(),
    )
}

/// Round one: the question names the action and the files, and the forge sees
/// no proposal at all.
///
/// The negative half is the point, as it is for the delete round: an
/// `input_required` answered after the proposal was already opened would be a
/// confirmation of something the team can already see.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_share_is_asked_before_anything_is_shared() {
    let (h, mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;

    let asked = wire.open(eliciting(1, "tools/call", share_kb(None))).await;
    let result = &asked["result"];
    assert_eq!(result["resultType"], json!("input_required"), "{asked}");
    let message = result["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("Open a new proposal"), "{message}");
    assert!(message.contains("notes/a.md"), "names the file: {message}");
    // Every `create_` prefix, not just `create_proposal:`: a round that
    // uploaded blobs, built a tree or cut a branch and then asked would have
    // published most of the share already.
    assert!(
        !mock.calls().iter().any(|c| c.starts_with("create_")),
        "round one shares nothing: {:?}",
        mock.calls()
    );
}

/// Round two with a yes shares, and the next round one names the update.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_share_round_two_shares_and_an_update_names_the_proposal() {
    let (h, _mock) = Harness::team().await;
    let (mut wire, number, _branch) = first_shared_proposal(&h).await;

    // A second edited share's round 1 names the update rather than a create.
    write_kb_engram(&h, "notes/b.md", "Beta", "notes/b", "beta");
    let asked = wire.call(eliciting(3, "tools/call", share_kb(None))).await;
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains(&format!("Update open proposal #{number}")),
        "{message}"
    );
}

/// The same round one on a forge that stacks: the question names the layer
/// the new proposal would land on, and the forge is still untouched.
///
/// This is the stacked model's own version of the assertion above, and it is
/// worth its own test because the two differ in what the user is agreeing to.
/// An update moves a proposal reviewers are already looking at; a stack opens
/// a second one on top of it. A gate that let the stacked plan through
/// unasked would publish a pull request the user never saw a word about,
/// which is precisely what `share_plan_needs_confirmation` fails safe on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_share_on_a_stacking_forge_is_asked_before_the_layer_is_opened() {
    let (h, mock) = Harness::team().await;
    // On before the first share, so the capability answer this domain caches
    // is the stacked one from the start.
    mock.enable_stacks();
    let (mut wire, number, _branch) = first_shared_proposal(&h).await;

    // Everything the forge was told while the first proposal was legitimately
    // opened, so the silence asserted below is about this round alone.
    let before = mock.calls().len();

    write_kb_engram(&h, "notes/b.md", "Beta", "notes/b", "beta");
    let asked = wire.call(eliciting(3, "tools/call", share_kb(None))).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "a stacked share is confirmed too, not waved through: {asked}"
    );
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains(&format!("Stack a new proposal on top of #{number}")),
        "the question names the layer it lands on: {message}"
    );
    assert!(
        message.contains("notes/b.md"),
        "and the file it carries: {message}"
    );

    assert!(
        !mock.calls()[before..]
            .iter()
            .any(|c| c.starts_with("create_")),
        "round one opens no layer: {:?}",
        &mock.calls()[before..]
    );
}

/// And the yes that follows: round two on a stacking forge publishes the
/// layer, opened against the branch below it and grouped into a stack.
///
/// The round-one test above proves the question is asked; this proves what
/// the answer buys, over the wire rather than in the remote crate's own
/// harness - a second pull request, based on the layer below it rather than
/// on the trunk, and a `create_stack` linking the two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_stacked_share_opens_the_layer_on_the_one_below_it() {
    let (h, mock) = Harness::team().await;
    mock.enable_stacks();
    let (mut wire, first, first_branch) = first_shared_proposal(&h).await;

    write_kb_engram(&h, "notes/b.md", "Beta", "notes/b", "beta");
    let asked = wire.call(eliciting(3, "tools/call", share_kb(None))).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "{asked}"
    );
    let done = wire
        .call(eliciting(
            4,
            "tools/call",
            share_kb(Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "{done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        body["outcome"], "proposed",
        "a second proposal, not an update: {body}"
    );
    let second = body["number"].as_u64().unwrap();
    assert_ne!(second, first, "the layer below is left alone");
    assert_eq!(
        body["stack_position"],
        json!([2, 2]),
        "layer 2 of 2: {body}"
    );

    // The layer targets the branch below it, not the trunk, and the two were
    // grouped on the forge.
    assert_eq!(
        mock.proposal_base(second).as_deref(),
        Some(first_branch.as_str()),
        "the new layer is based on the one below it"
    );
    assert!(
        mock.calls()
            .contains(&format!("create_stack:[{first},{second}]")),
        "the chain is linked bottom first: {:?}",
        mock.calls()
    );
    let stack = body["stack_number"]
        .as_u64()
        .unwrap_or_else(|| panic!("the chain is linked: {body}"));
    let members: Vec<u64> = mock
        .stack(stack)
        .expect("the stack is in the registry")
        .members
        .iter()
        .map(|member| member.number)
        .collect();
    assert_eq!(members, vec![first, second]);
}

/// Round two with a yes on an *update* lands on the open proposal rather than
/// opening a second one.
///
/// The confirmed create is proved above; this is the other half, and it is
/// the half the description promises hardest ("same proposal number, same
/// URL, it never opens a duplicate"), so the number is asserted rather than
/// just the outcome word.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_confirmed_update_round_two_lands_on_the_same_proposal() {
    let (h, mock) = Harness::team().await;
    let (mut wire, number, _branch) = first_shared_proposal(&h).await;

    write_kb_engram(&h, "notes/b.md", "Beta", "notes/b", "beta");
    let asked = wire.call(eliciting(3, "tools/call", share_kb(None))).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "an update is confirmed too, not waved through: {asked}"
    );

    let done = wire
        .call(eliciting(
            4,
            "tools/call",
            share_kb(Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "{done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "updated", "{body}");
    assert_eq!(
        body["proposal"]["number"].as_u64(),
        Some(number),
        "the same proposal, not a duplicate: {body}"
    );
    assert!(
        mock.calls()
            .iter()
            .any(|c| c.starts_with("update_proposal:")),
        "the open proposal was patched: {:?}",
        mock.calls()
    );
}

/// A diverged proposal is reported in round one rather than asked about.
///
/// There is nothing to confirm: the share cannot proceed at all until the
/// user settles the review on GitHub or withdraws, so putting a yes/no
/// question in front of them would offer a choice neither answer to changes
/// anything. The guidance that names both ways out has to survive the round.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_diverged_proposal_answers_round_one_with_guidance() {
    let (h, mock) = Harness::team().await;
    let (mut wire, number, branch) = first_shared_proposal(&h).await;

    // A reviewer pushes a commit onto the proposal branch.
    let amended = mock.add_commit(
        [(
            "MANIFEST.md".to_string(),
            b"---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# kb\n\n## Scope\n\n- Everything, reviewed\n\n## When to Use\n\n- Always\n".to_vec(),
        )]
        .into_iter()
        .collect(),
    );
    mock.set_branch(&branch, &amended);
    write_kb_engram(&h, "notes/c.md", "Gamma", "notes/c", "gamma");

    // Everything the forge was told before the diverged round, so the
    // assertion below is about this round rather than about the share that
    // legitimately opened the proposal.
    let before = mock.calls().len();

    let done = wire.call(eliciting(3, "tools/call", share_kb(None))).await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a share that cannot proceed is reported, not negotiated: {done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "proposal_diverged", "{body}");
    assert_eq!(body["proposal"]["number"].as_u64(), Some(number), "{body}");
    let guidance = body["guidance"].as_str().unwrap_or_default();
    assert!(
        guidance.contains("withdraw") && guidance.contains("GitHub"),
        "both ways out are named: {guidance}"
    );

    let during = &mock.calls()[before..];
    assert!(
        !during
            .iter()
            .any(|c| c.starts_with("create_") || c.starts_with("update_")),
        "the diverged round publishes nothing: {during:?}"
    );
}

/// Round two with a no refuses, and the forge is never written to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_share_refuses_with_no_provider_writes() {
    let (h, mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;
    let _ = wire.open(eliciting(1, "tools/call", share_kb(None))).await;

    let refused = wire
        .call(eliciting(
            2,
            "tools/call",
            share_kb(Some(answer("decline", false))),
        ))
        .await;
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    assert!(
        refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("nothing was shared"),
        "and it says what did not happen, exactly as the delete refusal does: {refused}"
    );
    assert!(
        !mock.calls().iter().any(|c| c.starts_with("create_")),
        "a decline uploads no blob, builds no tree, cuts no branch and opens \
         no proposal: {:?}",
        mock.calls()
    );
}

/// The same no on the *update* path, which the create-path test above cannot
/// speak for: a declined update has an open proposal standing behind it, so
/// "nothing happened" has to mean the forge was not written to AND the record
/// still describes the proposal the reviewer is looking at.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declined_update_leaves_the_open_proposal_exactly_as_it_was() {
    let (h, mock) = Harness::team().await;
    let (mut wire, number, _branch) = first_shared_proposal(&h).await;
    let state_path = h.root.join("origins").join("kb").join("state.json");

    write_kb_engram(&h, "notes/b.md", "Beta", "notes/b", "beta");
    let asked = wire.call(eliciting(3, "tools/call", share_kb(None))).await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "round one asks about the update: {asked}"
    );
    // Snapshotted after round one, so the subject is what the DECLINE changed:
    // round one previews, and a preview legitimately pulls and saves.
    let before = std::fs::read_to_string(&state_path).unwrap();
    let calls_before = mock.calls().len();

    let refused = wire
        .call(eliciting(
            4,
            "tools/call",
            share_kb(Some(answer("decline", false))),
        ))
        .await;
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    assert!(
        refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("nothing was shared"),
        "{refused}"
    );

    let during = &mock.calls()[calls_before..];
    assert!(
        !during
            .iter()
            .any(|c| c.starts_with("create_") || c.starts_with("update_")),
        "a declined update pushes no commit and patches no proposal: {during:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&state_path).unwrap(),
        before,
        "and proposal #{number}'s record is byte for byte what it was"
    );
}

/// A modern peer that declared no elicitation capability shares on the first
/// call, exactly as it did before the round existed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_eliciting_modern_share_is_single_round() {
    let (h, _mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;
    let done = wire.open(modern(1, "tools/call", share_kb(None))).await;
    assert!(done["error"].is_null(), "{done}");
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "proposed", "one round, shared: {body}");
}

/// A legacy peer is served byte for byte what it was before, elicitation
/// capability declared or not: the share happens on the first call and the
/// result carries no `resultType` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_share_is_single_round_with_no_input_required() {
    let (h, _mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;

    let handshake = wire
        .open(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": { "elicitation": {} },
                "clientInfo": { "name": "legacy-era-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(handshake["result"]["protocolVersion"], json!(LEGACY));

    let done = wire.call(request(2, "tools/call", share_kb(None))).await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "{done}"
    );
    assert!(
        done["result"]["resultType"].is_null(),
        "a legacy result carries no discriminator: {done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "proposed", "{body}");
}

/// A share that would write nothing is answered in round one rather than
/// asked about: there is no decision for the user to make.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_to_share_answers_round_one_without_a_question() {
    let (h, _mock) = Harness::team().await;
    // No edit: the tree matches the origin.
    let mut wire = h.stdio().await;
    let done = wire.open(eliciting(1, "tools/call", share_kb(None))).await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "nothing to confirm: {done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "nothing_to_share", "{body}");
}

/// The same round over streamable HTTP, which reaches the modern dispatch by
/// a different route than stdio does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_share_round_runs_over_http_too() {
    let (h, _mock) = Harness::team().await;
    edit_kb(&h);
    let addr = h.http().await;
    let raw = eliciting_post(addr, 1, "tools/call", share_kb(None)).await;
    assert!(raw.starts_with("HTTP/1.1 200 OK"), "{}", head_of(&raw));
    let answered = payload(&raw);
    assert_eq!(
        answered["result"]["resultType"],
        json!("input_required"),
        "{answered}"
    );
}

// --- personal share identity, resolved from the transport -------------------
//
// An instance can be configured to share as the acting person's own GitHub
// account rather than as the one instance credential. MCP has two transports
// and they are two different actors: a stdio session is a process this
// machine's harness started, so it acts as the machine owner, while an HTTP
// session carries no user auth at all and acts as the account
// `github.agent_identity` names. Wiring that up is the release gate these
// three tests stand on - without it a remote agent's share would go out under
// the machine owner's name.

/// The two refusal texts, quoted from `crate::engine` rather than retyped:
/// they travel to the caller verbatim, so a paraphrase here would pass while
/// the product said something else.
const PERSONAL_TOKEN_MISSING: &str = "This instance shares with personal GitHub identities. Connect yours in Fluid (profile > GitHub identity) or run 'crystalline connect github --personal', then share again.";
const AGENT_IDENTITY_UNSET: &str = "This instance shares with personal GitHub identities and no agent identity is configured: set github.agent_identity to the account whose GitHub connection agent shares should use, or share from Fluid or the CLI.";

/// **A stdio session shares as the machine owner**, so an instance sharing
/// personally with no owner connection on file teaches the two ways to make
/// one - as `invalid_params`, because it is a situation the caller can get out
/// of rather than a server fault.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_stdio_share_without_an_owner_connection_teaches_the_fix() {
    let (mut h, _mock) = Harness::team().await;
    edit_kb(&h);
    h.share_personally(None).await;
    let mut wire = h.stdio().await;

    let refused = wire.open(modern(1, "tools/call", share_kb(None))).await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["error"]["message"],
        json!(PERSONAL_TOKEN_MISSING),
        "{refused}"
    );
}

/// **Strictness surfaces in round one.** An eliciting peer is asked before a
/// share, and a question about a proposal this instance would then refuse to
/// open is a question it cannot honour - so the preview resolves the same
/// identity the confirmed call would and answers the refusal instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_share_refuses_in_round_one_rather_than_asking() {
    let (mut h, _mock) = Harness::team().await;
    edit_kb(&h);
    h.share_personally(None).await;
    let mut wire = h.stdio().await;

    let refused = wire.open(eliciting(1, "tools/call", share_kb(None))).await;
    assert!(
        refused["result"]["resultType"] != json!("input_required"),
        "no question it could not honour: {refused}"
    );
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["error"]["message"],
        json!(PERSONAL_TOKEN_MISSING),
        "{refused}"
    );
}

/// **An HTTP session shares as the configured agent identity**, and with none
/// configured the refusal names the setting an admin has to write rather than
/// reporting a missing token for an identity the caller never chose.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_http_share_without_an_agent_identity_names_the_setting() {
    let (mut h, _mock) = Harness::team().await;
    edit_kb(&h);
    h.share_personally(None).await;
    let addr = h.http().await;

    // A refused call is answered as a JSON-RPC error, which rmcp's HTTP
    // transport frames with a 400 rather than the 200 a result gets - the
    // status is that layer's business, the message is ours.
    let raw = modern_post(addr, 1, "tools/call", share_kb(None)).await;
    let refused = payload(&raw);
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["error"]["message"],
        json!(AGENT_IDENTITY_UNSET),
        "{refused}"
    );
}

// --- the conflict resolution round ------------------------------------------

/// Manufacture one conflict in the team domain: edit locally, advance the
/// origin with a different edit of the same engram, pull.
async fn conflicted_kb(h: &Harness, mock: &MockProvider) -> String {
    std::fs::write(
        h.root.join("kb/notes/a.md"),
        "---\ntype: engram\ntitle: Alpha\npermalink: notes/a\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nalpha, my local edit\n",
    )
    .unwrap();
    let c2 = mock.add_commit(
        [
            ("MANIFEST.md".to_string(), std::fs::read(h.root.join("kb/MANIFEST.md")).unwrap()),
            ("notes/a.md".to_string(), b"---\ntype: engram\ntitle: Alpha\npermalink: notes/a\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\nalpha, the team's edit\n".to_vec()),
        ]
        .into_iter()
        .collect(),
    );
    mock.set_branch("main", &c2);
    let update = h.engine.origin_update(Some("kb")).await.unwrap();
    let conflicts = update["domains"][0]["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{update}");
    conflicts[0]["path"].as_str().unwrap().to_string()
}

fn resolve_kb(path: &str, resolution: Option<&str>, responses: Option<Value>) -> Value {
    let mut arguments = json!({ "domain": "kb", "path": path });
    if let Some(resolution) = resolution {
        arguments["resolution"] = json!(resolution);
    }
    let mut params = json!({ "name": "resolve_conflict", "arguments": arguments });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The client's enum answer to the `resolution` question.
fn resolution_answer(action: &str, choice: &str) -> Value {
    json!({ "resolution": { "action": action, "content": { "resolution": choice } } })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_resolve_without_a_resolution_is_offered_the_choice() {
    let (h, mock) = Harness::team().await;
    let path = conflicted_kb(&h, &mock).await;
    let mut wire = h.stdio().await;

    let asked = wire
        .open(eliciting(1, "tools/call", resolve_kb(&path, None, None)))
        .await;
    let result = &asked["result"];
    assert_eq!(result["resultType"], json!("input_required"), "{asked}");
    let question = &result["inputRequests"]["resolution"];
    let schema = &question["params"]["requestedSchema"];
    assert_eq!(schema["required"], json!(["resolution"]), "{asked}");
    // A titled single-select is rendered as `oneOf` rather than a flat `enum`,
    // the same shape the collision question already ships.
    let property = &schema["properties"]["resolution"];
    let options: Vec<&str> = property["oneOf"]
        .as_array()
        .expect("a titled single-select carries oneOf")
        .iter()
        .map(|option| option["const"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(options, vec!["mine", "theirs"], "{asked}");
    let message = question["params"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&path), "names the path: {message}");
    assert!(message.contains("local (mine)"), "{message}");
    assert!(message.contains("upstream (theirs)"), "{message}");
    assert!(
        message.contains("my local edit"),
        "previews my side: {message}"
    );
    assert!(
        message.contains("the team's edit"),
        "previews theirs: {message}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chosen_resolution_round_two_applies_it() {
    let (h, mock) = Harness::team().await;
    let path = conflicted_kb(&h, &mock).await;
    let mut wire = h.stdio().await;
    let _ = wire
        .open(eliciting(1, "tools/call", resolve_kb(&path, None, None)))
        .await;

    let done = wire
        .call(eliciting(
            2,
            "tools/call",
            resolve_kb(&path, None, Some(resolution_answer("accept", "theirs"))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "{done}"
    );
    let text = std::fs::read_to_string(h.root.join("kb").join(&path)).unwrap();
    assert!(text.contains("the team's edit"), "theirs won: {text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_eliciting_resolve_without_a_resolution_refuses_naming_the_three() {
    let (h, mock) = Harness::team().await;
    let path = conflicted_kb(&h, &mock).await;
    let mut wire = h.stdio().await;
    let refused = wire
        .open(modern(1, "tools/call", resolve_kb(&path, None, None)))
        .await;
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    let text = refused["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("mine"), "{text}");
    assert!(text.contains("theirs"), "{text}");
    assert!(text.contains("merged"), "{text}");
    // And the conflict is still open.
    let status = h.engine.origin_status(Some("kb")).await.unwrap();
    assert_eq!(
        status["domains"][0]["conflicts"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_resolution_stays_single_round_for_everyone() {
    let (h, mock) = Harness::team().await;
    let path = conflicted_kb(&h, &mock).await;
    let mut wire = h.stdio().await;
    let done = wire
        .open(eliciting(
            1,
            "tools/call",
            resolve_kb(&path, Some("mine"), None),
        ))
        .await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "{done}"
    );
    assert!(done["result"]["isError"] != json!(true), "{done}");
}

/// A share over a domain with an unresolved conflict is answered in round one
/// rather than asked about: the share cannot proceed until the conflict is
/// settled, so there is nothing for the user to say yes to, and the count of
/// what needs resolving has to survive the round.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicted_share_answers_round_one_with_the_pending_count() {
    let (h, mock) = Harness::team().await;
    let _path = conflicted_kb(&h, &mock).await;
    let mut wire = h.stdio().await;

    let done = wire.open(eliciting(1, "tools/call", share_kb(None))).await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a share that cannot proceed is reported, not negotiated: {done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["outcome"], "conflicts_pending", "{body}");
    assert_eq!(body["count"], json!(1), "{body}");
}

// --- withdraw_proposal, and its own confirmation round -----------------------
//
// Withdrawing closes a pull request the team is looking at, and a `revert`
// rewrites the working tree besides, so the eliciting peer is asked the same
// way `share_changes` asks. The gate is the same two-sided one: a peer that
// declared no elicitation capability is served exactly one round, unchanged.

/// A withdraw call's params, optionally naming a layer and carrying a round 2
/// answer.
fn withdraw_kb(proposal: Option<u64>, responses: Option<Value>) -> Value {
    let mut arguments = json!({ "domain": "kb" });
    if let Some(number) = proposal {
        arguments["proposal"] = json!(number);
    }
    let mut params = json!({
        "name": "withdraw_proposal",
        "arguments": arguments,
    });
    if let Some(responses) = responses {
        params["inputResponses"] = responses;
    }
    params
}

/// The peer that cannot be asked keeps today's behaviour: one round, and the
/// proposal is closed on the forge by the time it answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn withdraw_proposal_closes_the_open_proposal_single_round() {
    let (h, mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;
    let shared = wire.open(modern(1, "tools/call", share_kb(None))).await;
    assert!(shared["error"].is_null(), "{shared}");

    let done = wire
        .call(modern(2, "tools/call", withdraw_kb(None, None)))
        .await;
    assert_ne!(
        done["result"]["resultType"],
        json!("input_required"),
        "a peer that declared no elicitation is never asked: {done}"
    );
    assert!(done["result"]["isError"] != json!(true), "{done}");
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["status"], "withdrawn");
    assert_eq!(body["closed"], true);
    assert!(
        mock.calls()
            .iter()
            .any(|c| c.starts_with("close_proposal:")),
        "{:?}",
        mock.calls()
    );
}

/// Round one names the proposal and closes nothing; round two closes it.
///
/// The negative half is the point, as it is for the share and delete rounds:
/// an `input_required` answered after the pull request was already closed
/// would be a confirmation of something the team can already see.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_withdrawal_is_asked_before_the_proposal_is_closed() {
    let (h, mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;
    let shared = wire.open(modern(1, "tools/call", share_kb(None))).await;
    let body: Value =
        serde_json::from_str(shared["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let number = body["number"].as_u64().unwrap();

    let asked = wire
        .call(eliciting(2, "tools/call", withdraw_kb(None, None)))
        .await;
    assert_eq!(
        asked["result"]["resultType"],
        json!("input_required"),
        "{asked}"
    );
    let message = asked["result"]["inputRequests"]["confirm"]["params"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.contains(&format!("Withdraws proposal #{number}")),
        "the question names the layer it would close: {message}"
    );
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("close_proposal:")),
        "round one closes nothing: {:?}",
        mock.calls()
    );

    let done = wire
        .call(eliciting(
            3,
            "tools/call",
            withdraw_kb(None, Some(answer("accept", true))),
        ))
        .await;
    assert!(
        done["error"].is_null() && done["result"]["isError"] != json!(true),
        "{done}"
    );
    let body: Value =
        serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["number"], json!(number));
    assert_eq!(body["status"], "withdrawn");
    assert_eq!(body["closed"], true);
    assert!(
        mock.calls()
            .iter()
            .any(|c| c.starts_with("close_proposal:")),
        "and round two does close it: {:?}",
        mock.calls()
    );
}

/// A withdrawal that cannot resolve a target is reported, not negotiated: the
/// teaching text arrives verbatim as an `invalid_params` error rather than as
/// a question about a proposal that does not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eliciting_withdrawal_of_an_unknown_number_is_refused_rather_than_asked() {
    let (h, mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;
    let shared = wire.open(modern(1, "tools/call", share_kb(None))).await;
    assert!(shared["error"].is_null(), "{shared}");

    let refused = wire
        .call(eliciting(2, "tools/call", withdraw_kb(Some(99), None)))
        .await;
    assert!(
        refused["result"]["resultType"] != json!("input_required"),
        "{refused}"
    );
    assert_eq!(
        refused["error"]["code"],
        json!(-32602),
        "an unresolvable target is the caller's mistake: {refused}"
    );
    assert_eq!(
        refused["error"]["message"],
        json!("no open or declined proposal #99 found for this domain"),
        "{refused}"
    );
    assert!(
        !mock
            .calls()
            .iter()
            .any(|c| c.starts_with("close_proposal:")),
        "and nothing was closed: {:?}",
        mock.calls()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn withdraw_proposal_is_gated_exactly_like_share_changes() {
    // The default (github off) harness withholds the tool from the listing and
    // still refuses the call with the enablement message, byte for byte the
    // share_changes refusal.
    let h = Harness::new().await;
    let mut wire = h.stdio().await;
    let listed = wire.open(modern(1, "tools/list", json!({}))).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(!names.contains(&"withdraw_proposal"), "{names:?}");

    let refused = wire
        .call(modern(
            2,
            "tools/call",
            json!({ "name": "withdraw_proposal", "arguments": { "domain": "eng" } }),
        ))
        .await;
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    assert!(
        refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("github.enabled"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn origin_status_is_lean_and_update_domain_carries_the_bodies() {
    let (h, mock) = Harness::team().await;
    edit_kb(&h);
    let mut wire = h.stdio().await;
    let shared = wire.open(modern(1, "tools/call", share_kb(None))).await;
    let body: Value =
        serde_json::from_str(shared["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let number = body["number"].as_u64().unwrap();
    mock.set_feedback(
        number,
        crystalline_remote::provider::Feedback {
            review_state: Some("changes_requested".to_string()),
            items: vec![crystalline_remote::state::FeedbackItem {
                author: "ana".to_string(),
                body: "needs a source".to_string(),
                path: None,
                line: None,
                submitted_at: "2026-08-21T10:00:00Z".to_string(),
                kind: crystalline_remote::state::FeedbackKind::Comment,
            }],
        },
    );

    // update_domain fetches and carries the comment text.
    let updated = wire
        .call(modern(
            2,
            "tools/call",
            json!({ "name": "update_domain", "arguments": { "domain": "kb" } }),
        ))
        .await;
    let update_body: Value =
        serde_json::from_str(updated["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let prop = &update_body["domains"][0]["open_proposals"][0];
    assert_eq!(
        prop["feedback"][0]["body"], "needs a source",
        "{update_body}"
    );

    // origin_status stays lean: count, not bodies.
    let status = wire
        .call(modern(
            3,
            "tools/call",
            json!({ "name": "origin_status", "arguments": { "domain": "kb" } }),
        ))
        .await;
    let status_body: Value =
        serde_json::from_str(status["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let entry = &status_body["domains"][0]["open_proposals"][0];
    assert_eq!(entry["number"], number);
    assert_eq!(entry["review_state"], "changes_requested");
    assert_eq!(entry["feedback_count"], 1);
    assert!(
        entry.get("feedback").is_none(),
        "no bodies in status: {entry}"
    );
    assert_eq!(entry["amended_upstream"], false);
}

// --- the collaboration surface appears when it is enabled -------------------

/// The five GitHub-gated tool names, in the order the listing carries them.
const COLLAB_GATED: [&str; 5] = [
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
    "withdraw_proposal",
];

fn listed_names(answer: &Value) -> Vec<String> {
    answer["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tool list in {answer}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// **A default install does not list the collaboration tools at all.**
///
/// `github.enabled` is off out of the box, and five of the six collaboration
/// tools do nothing but talk to a forge nobody connected. They are withheld
/// from the listing rather than listed-and-refusing, so a default install
/// spends no context on a surface it cannot use. `configure` is the one that
/// stays, because it is the only way to turn the rest on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_default_install_lists_configure_but_none_of_the_gated_collaboration_tools() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let answer = wire.open(modern(1, "tools/list", json!({}))).await;
    let names = listed_names(&answer);
    assert!(
        names.contains(&"configure".to_string()),
        "configure is the enable path and is always listed: {names:?}"
    );
    for tool in COLLAB_GATED {
        assert!(
            !names.contains(&tool.to_string()),
            "{tool} must not be listed while github.enabled is off: {names:?}"
        );
    }
}

/// **Turning the setting on through the tool makes the five appear.**
///
/// The listing gate reads `github.enabled` live, exactly as the call-time
/// refusal does, so the very connection that flipped the setting sees the
/// wider list on its next `tools/list`. The invariance MCP 2026-07-28 requires
/// is per-instant: every client listing at the same moment gets the same
/// answer, and the change is announced to whoever subscribed for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabling_github_through_configure_makes_the_five_appear_on_the_next_list() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let before = listed_names(&wire.open(modern(1, "tools/list", json!({}))).await);
    let flip = wire
        .call(modern(
            2,
            "tools/call",
            json!({
                "name": "configure",
                "arguments": { "set": { "github.enabled": "true" } },
            }),
        ))
        .await;
    assert_ne!(
        flip["result"]["isError"],
        json!(true),
        "the flip must land, or the list below proves nothing: {flip}"
    );

    let after = listed_names(&wire.call(modern(3, "tools/list", json!({}))).await);
    for tool in COLLAB_GATED {
        assert!(
            after.contains(&tool.to_string()),
            "{tool} appears once collaboration is on: {after:?}"
        );
    }
    assert_eq!(
        after.len(),
        before.len() + COLLAB_GATED.len(),
        "exactly the five arrived: {before:?} -> {after:?}"
    );
}

/// **Hidden is not disabled.** A client holding a cached list from before the
/// setting went off - or one that simply guessed the name - still reaches the
/// handler and is told which setting to turn on, rather than being answered
/// "no such tool" and left to guess.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calling_a_hidden_collaboration_tool_still_teaches_rather_than_vanishing() {
    let h = Harness::new().await;
    let mut wire = h.stdio().await;

    let answer = wire
        .open(modern(
            1,
            "tools/call",
            json!({ "name": "share_changes", "arguments": { "domain": "eng" } }),
        ))
        .await;
    assert!(
        answer["error"].is_null(),
        "a hidden tool answers rather than failing at the protocol level: {answer}"
    );
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("not enabled") && text.contains("github.enabled"),
        "the refusal names the setting to turn on: {answer}"
    );
}
