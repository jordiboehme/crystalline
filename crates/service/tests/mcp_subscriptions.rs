//! The 2026-07-28 notification channel: `subscriptions/listen`, and the
//! unsolicited pushes it replaces.
//!
//! From that revision on there is no unsolicited channel left. `/basic/patterns/
//! subscriptions` says the server "MUST NOT send notification types the client
//! has not explicitly requested", the HTTP GET stream is gone, and per-request
//! SSE notifications must relate to the originating request. So a list-changed
//! notification either rides a subscription stream the client opened or it does
//! not exist.
//!
//! **Why these tests could run before the era was advertised**, which is why
//! they drive stdio and a discover opener rather than the advertised revision.
//! rmcp refuses
//! `subscriptions/listen` with `method_not_found` while `legacy_request` is true
//! (rmcp 3.1.2 `handler/server.rs:147-150`), and `uses_legacy_lifecycle`
//! (`service.rs:196-202`) is `!requires_request_metadata && version <
//! 2026-07-28`. A client that opens with `server/discover` arms the server
//! peer's one-way metadata latch (`service/server.rs:541`, the only call site in
//! the crate), which makes `requires_request_metadata` true for every later
//! request on that connection. So the modern dispatch is reachable over stdio at
//! any advertised revision, and every test here drives it that way.
//!
//! The HTTP transport arms nothing, so before 2026-07-28 was advertised the
//! same request was classified legacy there and refused `method not found`.
//! That gap is closed: a `subscriptions/listen` POST declaring the era in its
//! own `_meta` reaches this handler, and
//! `tests/mcp_modern_era.rs::the_http_subscription_stream_acknowledges_first_and_stays_silent`
//! drives it.
//!
//! **What this file pins that is not ours.** The two server MUSTs on a
//! subscription - the acknowledgment is the first message on the stream, and it
//! carries the subscription id in `_meta` - are implemented inside
//! `SubscriptionContext::establish` (`service/server.rs:337-375`) and inherited
//! rather than written by us. `the_acknowledgment_is_the_first_message_and_names_the_subscription`
//! reads them off the wire, so an upstream change that stopped honouring either
//! fails here instead of in a client's log.
//!
//! **What actually travels on the stream.** One setting moves a list:
//! `github.enabled`, which decides whether the five GitHub collaboration tools
//! are listed at all. A client flips it with `configure`, and so do two
//! callers with no MCP connection of their own - `crystalline config set` over
//! the control socket and Fluid's Connect button through the REST API - which
//! is why the announcement lives on `Engine::configure` and why one test here
//! drives that seam directly. So a subscriber gets
//! `notifications/tools/list_changed` on the flip whoever made it, a peer that
//! opened no stream gets nothing whichever era it speaks, and the resources and
//! prompts categories are accepted and then stay quiet, because the settings
//! behind those two lists are frozen at engine construction.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::engine::ConfigureAction;
use crystalline_service::mcp::McpServer;
use rmcp::model::{
    CallToolRequestParams, ClientInfo, ProtocolVersion, ServerNotification, SubscriptionFilter,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService, Subscription};
use rmcp::{ClientHandler, RoleClient, RoleServer};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

/// A client that records the notifications rmcp delivers outside any
/// subscription stream, which is where an unsolicited server push lands.
#[derive(Clone, Default)]
struct Recorder {
    unsolicited: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Recorder {
    fn seen(&self) -> Vec<String> {
        self.unsolicited.lock().unwrap().clone()
    }
}

impl ClientHandler for Recorder {
    async fn on_tool_list_changed(&self, _context: rmcp::service::NotificationContext<RoleClient>) {
        self.unsolicited
            .lock()
            .unwrap()
            .push("notifications/tools/list_changed".to_string());
    }

    async fn on_prompt_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.unsolicited
            .lock()
            .unwrap()
            .push("notifications/prompts/list_changed".to_string());
    }

    async fn on_resource_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) {
        self.unsolicited
            .lock()
            .unwrap()
            .push("notifications/resources/list_changed".to_string());
    }
}

struct Harness {
    _tmp: tempfile::TempDir,
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
        Harness { _tmp: tmp, engine }
    }

    /// A connection on the modern lifecycle: the client opens with
    /// `server/discover`, which arms rmcp's per-request-metadata latch and puts
    /// every later request on the modern dispatch (see the module doc).
    async fn connect_modern(
        &self,
    ) -> (
        RunningService<RoleClient, Recorder>,
        RunningService<RoleServer, McpServer>,
        Recorder,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let recorder = Recorder::default();
        let client = recorder
            .clone()
            .serve_with_lifecycle(
                client_io,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![newest_served()],
                },
            )
            .await
            .expect("the server answers server/discover");
        let server = server_task.await.unwrap().unwrap();
        (client, server, recorder)
    }

    /// A connection on the legacy `initialize` handshake, which is what every
    /// client we serve today uses.
    async fn connect_legacy(
        &self,
    ) -> (
        RunningService<RoleClient, Recorder>,
        RunningService<RoleServer, McpServer>,
        Recorder,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let recorder = Recorder::default();
        let client = rmcp::serve_client(recorder.clone(), client_io)
            .await
            .unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server, recorder)
    }
}

/// The newest revision this server advertises. Every modern-lifecycle test
/// drives that one, so the file follows the advertised set rather than pinning
/// a revision of its own.
fn newest_served() -> ProtocolVersion {
    crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS
        .last()
        .unwrap()
        .clone()
}

/// Set `github.enabled` through the tool, and prove the write landed.
///
/// This is the call that used to push an unsolicited
/// `notifications/tools/list_changed`. Asserting the snapshot matters: in rmcp
/// 3.1.2 a parameter-deserialization failure comes back as a **tool-level**
/// error, so a malformed `configure` call answers `Ok` with `is_error` set and
/// changes nothing - a silence test would then pass for the wrong reason.
async fn set_github_enabled(peer: &rmcp::service::Peer<RoleClient>, want: bool) {
    let result = peer
        .call_tool(
            CallToolRequestParams::new("configure".to_string()).with_arguments(
                json!({ "set": { "github.enabled": want.to_string() } })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("configure answers rather than failing at the protocol level");
    let body = serde_json::to_value(&result).unwrap();
    let text = body
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let snapshot: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    assert_ne!(
        snapshot["github"]["github_enabled"],
        json!(!want),
        "the setting must actually read {want}, or the silence below proves nothing: {body}"
    );
}

/// Turn `github.enabled` on through the tool, the direction every test here
/// starts from.
async fn flip_github_enabled(peer: &rmcp::service::Peer<RoleClient>) {
    set_github_enabled(peer, true).await;
}

async fn tool_names(peer: &rmcp::service::Peer<RoleClient>) -> Vec<String> {
    peer.list_tools(Default::default())
        .await
        .unwrap()
        .tools
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// Wait briefly for a notification on a subscription stream, returning `None`
/// when the stream stays silent. Silence is the expected answer everywhere in
/// this file, so the wait is short on purpose.
async fn next_within(subscription: &mut Subscription) -> Option<ServerNotification> {
    match tokio::time::timeout(std::time::Duration::from_millis(300), subscription.next()).await {
        Ok(Ok(notification)) => notification,
        Ok(Err(e)) => panic!("the subscription stream failed: {e}"),
        Err(_) => None,
    }
}

// --- the two inherited MUSTs, read off the wire -----------------------------

/// A raw JSON-RPC conversation over the same duplex transport the stdio bridge
/// uses, so the bytes are asserted rather than an rmcp client's interpretation
/// of them.
struct Wire {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
}

impl Wire {
    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    /// The next message the server sends, or `None` if it stays silent.
    async fn recv(&mut self) -> Option<Value> {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            self.lines.next_line(),
        )
        .await
        {
            Ok(Ok(Some(line))) => Some(serde_json::from_str(&line).unwrap()),
            _ => None,
        }
    }
}

/// The `_meta` every request must carry once the metadata latch is armed
/// (`handler/server.rs:76-99`): the two `DRAFT_REQUIRED_KEYS`
/// (`model/meta.rs:400-403`), at an advertised version.
fn required_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": newest_served().as_str(),
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

/// **The two server MUSTs, asserted rather than assumed.**
///
/// `/basic/patterns/subscriptions` requires the acknowledgment
/// (`notifications/subscriptions/acknowledged`) to be the first message on the
/// subscription and to carry the subscription id in `_meta`. Both are
/// implemented by `SubscriptionContext::establish` (rmcp 3.1.2
/// `service/server.rs:337-375`: it builds the sink, attaches a
/// `NotificationMetaObject` with `set_subscription_id(request.id)` at `:353-355`
/// and sends the acknowledgment before returning the context our `listen` is
/// handed). We inherit both; this test is what turns "inherited" into
/// "verified", and it reads the wire because that is where the obligation is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_acknowledgment_is_the_first_message_and_names_the_subscription() {
    let h = Harness::new().await;
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let engine = h.engine.clone();
    let server_task =
        tokio::spawn(async move { rmcp::serve_server(McpServer::new(engine), server_io).await });

    let (read, write) = tokio::io::split(client_io);
    let mut wire = Wire {
        write,
        lines: tokio::io::BufReader::new(read).lines(),
    };

    // Open on the modern lifecycle, which arms the latch (see the module doc).
    wire.send(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": { "_meta": required_meta() },
    }))
    .await;
    let server = server_task.await.unwrap().unwrap();
    let discovered = wire.recv().await.expect("server/discover is answered");
    assert_eq!(discovered["id"], json!(1));
    assert!(
        discovered["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
        "the discover answer is the modern onboarding path: {discovered}"
    );

    wire.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "subscriptions/listen",
        "params": {
            "_meta": required_meta(),
            "notifications": { "toolsListChanged": true },
        },
    }))
    .await;

    let first = wire
        .recv()
        .await
        .expect("a subscription is acknowledged rather than refused");
    assert_eq!(
        first["method"], "notifications/subscriptions/acknowledged",
        "the acknowledgment MUST be the first message on the stream: {first}"
    );
    assert_eq!(
        first["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        json!(2),
        "and it MUST carry the subscription id in _meta: {first}"
    );
    assert_eq!(
        first["params"]["notifications"]["toolsListChanged"],
        json!(true),
        "the accepted filter names what this stream will carry: {first}"
    );

    // What follows it is the announcement, and it names this subscription in
    // the same `_meta` slot the acknowledgment used - `SubscriptionSink::send`
    // re-attaches the id to everything it carries (`:184-257`).
    wire.send(json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "_meta": required_meta(),
            "name": "configure",
            "arguments": { "set": { "github.enabled": "true" } },
        },
    }))
    .await;

    let mut seen = Vec::new();
    while let Some(message) = wire.recv().await {
        let done = message["id"] == json!(3);
        seen.push(message);
        if done {
            break;
        }
    }
    let answered = seen.iter().any(|m| m["id"] == json!(3));
    assert!(answered, "the configure call was answered: {seen:?}");
    let pushed: Vec<&Value> = seen
        .iter()
        .filter(|m| m["method"] == "notifications/tools/list_changed")
        .collect();
    assert_eq!(
        pushed.len(),
        1,
        "the moved list is announced exactly once: {seen:?}"
    );
    assert_eq!(
        pushed[0]["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        json!(2),
        "and it names the subscription it rode in on: {}",
        pushed[0]
    );

    drop(wire);
    drop(server);
}

// --- the subscription path itself -------------------------------------------

/// A modern client can open a subscription, and the acknowledgment names
/// exactly the categories it asked for out of the ones we can deliver.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_client_opens_a_subscription_for_the_categories_it_asks_for() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;

    let subscription = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");

    assert_eq!(subscription.acknowledged().tools_list_changed, Some(true));
    assert_eq!(
        subscription.acknowledged().prompts_list_changed,
        None,
        "a category the client did not ask for is never accepted for it"
    );
    assert_eq!(subscription.acknowledged().resources_list_changed, None);
}

/// A filter naming something this server cannot deliver is narrowed to what it
/// can. Resource subscriptions (`notifications/resources/updated`) are the
/// case: the shipped skills are static for a binary's lifetime, so `get_info`
/// deliberately does not enable `resources.subscribe`, and both our own
/// accepted filter and rmcp's intersection against the advertised capabilities
/// (`handler/server.rs:157-160`) drop it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_category_this_server_cannot_deliver_is_narrowed_away() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;

    let mut requested = SubscriptionFilter::builder()
        .tools_list_changed()
        .prompts_list_changed()
        .resources_list_changed()
        .build();
    requested.resource_subscriptions = Some(vec!["skill://crystalline/SKILL.md".to_string()]);

    let subscription = client
        .peer()
        .listen(requested)
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");

    assert_eq!(subscription.acknowledged().tools_list_changed, Some(true));
    assert_eq!(subscription.acknowledged().prompts_list_changed, Some(true));
    assert_eq!(
        subscription.acknowledged().resources_list_changed,
        Some(true)
    );
    assert_eq!(
        subscription.acknowledged().resource_subscriptions,
        None,
        "a resource subscription is not something this server can deliver"
    );
}

/// **A subscriber is told when the tool list moves, and told only on the
/// stream.**
///
/// `github.enabled` gates the listing of the five collaboration tools, so
/// flipping it is the one thing a client can ask this server to do that
/// changes what `tools/list` returns. The notification rides the subscription
/// - `Recorder` watches the off-stream channel and must stay empty, since from
/// 2026-07-28 an unsolicited push has no channel at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscribed_client_is_told_when_the_tool_list_moves() {
    let h = Harness::new().await;
    let (client, _server, recorder) = h.connect_modern().await;

    let mut subscription = client
        .peer()
        .listen(
            SubscriptionFilter::builder()
                .tools_list_changed()
                .prompts_list_changed()
                .resources_list_changed()
                .build(),
        )
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");

    let before = tool_names(client.peer()).await;
    flip_github_enabled(client.peer()).await;
    let after = tool_names(client.peer()).await;
    assert_eq!(
        after.len(),
        before.len() + 5,
        "the five collaboration tools arrived: {before:?} -> {after:?}"
    );

    let announced = next_within(&mut subscription)
        .await
        .expect("a moved list is announced on the stream");
    assert!(
        matches!(
            &announced,
            ServerNotification::ToolListChangedNotification(_)
        ),
        "and what is announced is the tool list: {announced:?}"
    );
    assert!(
        next_within(&mut subscription).await.is_none(),
        "one flip, one notification: the resources and prompts lists did not move"
    );

    // A configure call that writes the value the setting already had moves no
    // list, so it announces nothing. This is why the gate is read either side
    // of the write rather than parsed out of the request.
    set_github_enabled(client.peer(), true).await;
    assert!(
        next_within(&mut subscription).await.is_none(),
        "a configure call that changed no effective setting announced nothing"
    );

    // And the gate closing is a moved list exactly as the gate opening was.
    set_github_enabled(client.peer(), false).await;
    assert!(
        matches!(
            next_within(&mut subscription).await,
            Some(ServerNotification::ToolListChangedNotification(_))
        ),
        "the off direction is announced too: the five tools left the list"
    );

    assert!(
        recorder.seen().is_empty(),
        "nothing arrives off the stream: an unsolicited push has no channel in this era: {:?}",
        recorder.seen()
    );
}

/// **A flip that never touches the MCP server still reaches its subscribers.**
///
/// `github.enabled` is one shared setting on one shared engine, and three
/// callers write it: the `configure` tool, `crystalline config set` over the
/// control socket (`control.rs`, which calls exactly the engine method below)
/// and Fluid's Connect button through the REST API (`rest::github_settings::
/// ensure_enabled`, which calls it too). All three move every connected peer's
/// tool list, so the announcement lives on `Engine::configure` rather than on
/// the MCP handler - this test drives that seam with no MCP `configure` call
/// anywhere in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flip_from_outside_the_mcp_server_still_reaches_a_subscriber() {
    let h = Harness::new().await;
    let (client, _server, recorder) = h.connect_modern().await;

    let mut subscription = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");

    let before = tool_names(client.peer()).await;
    h.engine
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "true".to_string(),
        })
        .await
        .expect("the control and REST paths write the setting through this method");
    let after = tool_names(client.peer()).await;
    assert_eq!(
        after.len(),
        before.len() + 5,
        "the list moved for this peer: {before:?} -> {after:?}"
    );

    let announced = next_within(&mut subscription)
        .await
        .expect("a list moved by the control or REST path is announced too");
    assert!(
        matches!(
            &announced,
            ServerNotification::ToolListChangedNotification(_)
        ),
        "and what is announced is the tool list: {announced:?}"
    );
    assert!(
        recorder.seen().is_empty(),
        "still only on the stream: {:?}",
        recorder.seen()
    );
}

/// The other half of the same rule: a modern peer that opened no subscription
/// is told nothing, even though its list moved exactly as the subscriber's
/// did. Being on the modern lifecycle is not consent; opening the stream is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modern_client_that_never_subscribed_is_told_nothing() {
    let h = Harness::new().await;
    let (client, _server, recorder) = h.connect_modern().await;

    let before = tool_names(client.peer()).await;
    flip_github_enabled(client.peer()).await;
    let after = tool_names(client.peer()).await;
    assert_eq!(
        after.len(),
        before.len() + 5,
        "the list really did move for this peer too: {before:?} -> {after:?}"
    );

    // The send is fire-and-forget, so give it a moment to be wrong in.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        recorder.seen().is_empty(),
        "a peer that opened no stream has nowhere to be notified: {:?}",
        recorder.seen()
    );
}

/// Polls the registry until it holds exactly `expected` sinks, failing with
/// `what` after 5s. Both edges of a subscription's life are asynchronous with
/// the client's view of them (the acknowledgment precedes registration, and
/// cancellation is a notification), so the tests wait for the state instead
/// of trusting a window.
async fn wait_for_subscribers(h: &Harness, expected: usize, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while h.engine.list_subscribers().len() != expected {
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: expected {expected} registered sink(s) after 5s, found {}",
            h.engine.list_subscribers().len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// A subscription that ended leaves nothing behind that a later flip would
/// try to write to. `SubscriptionSink` holds a `Peer` and a child cancellation
/// token (`service/server.rs:139-144`), so a registry that kept dead entries
/// would pin them for the process's life; the guard in `McpServer::listen`
/// drops the entry when the stream ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_subscription_is_unregistered_rather_than_written_to() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;

    let subscription = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");
    // Registration is not synchronous with the client's return either: rmcp
    // sends the subscription acknowledgment in `SubscriptionContext::establish`
    // BEFORE the handler that registers the sink runs, so `listen` resolving
    // on the client only proves the acknowledgment arrived. Under full-suite
    // load the immediate assertion read 0 (2026-09-02); wait for the state.
    wait_for_subscribers(&h, 1, "the open stream is registered").await;

    drop(subscription);
    // Cancellation travels to the server as a notification, so the
    // unregistration is not synchronous with the drop. Poll for the state
    // instead of trusting one fixed window: under full-suite load a fixed
    // 200ms lost this race (locally and on the windows CI leg, 2026-08-31).
    wait_for_subscribers(&h, 0, "the ended stream left a sink behind").await;

    // And the flip still works with nobody listening.
    flip_github_enabled(client.peer()).await;
    assert!(
        tool_names(client.peer())
            .await
            .contains(&"share_changes".to_string()),
        "the list moved even with no subscriber to tell"
    );
}

/// Dropping the subscription handle cancels the listen request, and the session
/// keeps serving. `SubscriptionSink` holds a `Peer` and a child cancellation
/// token (`service/server.rs:139-144`), so a subscription that ended must leave
/// nothing behind that wedges the connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_subscription_leaves_the_session_serving() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;

    let subscription = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");
    let before = tool_names(client.peer()).await;
    drop(subscription);

    let after = tool_names(client.peer()).await;
    assert_eq!(before, after);
    assert!(!after.is_empty(), "the connection still serves its tools");
}

// --- the legacy era, which keeps its own answer ------------------------------

/// **No unsolicited push reaches a client that never asked for one.**
///
/// This is V3 itself. `configure` used to send `notifications/tools/list_changed`
/// whenever it flipped `github.enabled`, to whoever happened to be connected.
/// The flip does move the tool list again - the five collaboration tools are
/// listed only while the setting is on - but from 2026-07-28 an unsolicited
/// notification has no channel at all, so the announcement rides a
/// subscription or it does not happen. A legacy peer is the strictest case,
/// since it cannot subscribe and therefore can only ever be pushed at.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_is_never_pushed_a_list_change_it_did_not_ask_for() {
    let h = Harness::new().await;
    let (client, _server, recorder) = h.connect_legacy().await;

    let before = tool_names(client.peer()).await;
    flip_github_enabled(client.peer()).await;
    let after = tool_names(client.peer()).await;
    assert_eq!(
        after.len(),
        before.len() + 5,
        "its list moved, and it will only learn that by asking again: {before:?} -> {after:?}"
    );

    // The push was fire-and-forget, so give it a moment to be wrong in.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        recorder.seen().is_empty(),
        "an unsolicited push reached a client that asked for nothing: {:?}",
        recorder.seen()
    );
}

/// A legacy peer asking to subscribe is still answered `method not found`, and
/// that answer is rmcp's rather than ours: `subscriptions/listen` does not exist
/// before 2026-07-28, so the handler branch refuses it whenever the request is
/// on the legacy lifecycle (`handler/server.rs:147-150`).
///
/// **This passes before and after this task**, which is the point: it is the
/// guard that keeps the new hooks from leaking into an era that has no such
/// method.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_peer_has_no_subscriptions_at_all() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_legacy().await;

    let error = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .expect_err("subscriptions/listen does not exist in the legacy era");
    let text = error.to_string();
    assert!(
        text.contains("-32601") || text.to_ascii_lowercase().contains("method not found"),
        "the refusal names the missing method rather than something else: {text}"
    );
}

/// The advertised capabilities and the accepted filter say the same thing, so a
/// client is never offered a subscription for a category we do not advertise or
/// told about a capability it cannot subscribe to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_advertised_list_changed_capabilities_match_what_a_subscription_accepts() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;

    let info = client.peer().peer_info().expect("the peer published info");
    let capabilities = info.capabilities.clone();
    assert!(
        capabilities
            .tools
            .as_ref()
            .and_then(|t| t.list_changed)
            .unwrap_or(false),
        "the tools list-changed capability is advertised"
    );

    let subscription = client
        .peer()
        .listen(
            SubscriptionFilter::builder()
                .tools_list_changed()
                .prompts_list_changed()
                .resources_list_changed()
                .build(),
        )
        .await
        .expect("subscriptions/listen is served on the modern lifecycle");
    let accepted = subscription.acknowledged();

    for (advertised, accepted, label) in [
        (
            capabilities.tools.as_ref().and_then(|c| c.list_changed),
            accepted.tools_list_changed,
            "tools",
        ),
        (
            capabilities.prompts.as_ref().and_then(|c| c.list_changed),
            accepted.prompts_list_changed,
            "prompts",
        ),
        (
            capabilities.resources.as_ref().and_then(|c| c.list_changed),
            accepted.resources_list_changed,
            "resources",
        ),
    ] {
        assert_eq!(
            advertised, accepted,
            "{label}: the capability and the accepted filter must agree"
        );
    }

    assert_eq!(
        capabilities.resources.as_ref().and_then(|c| c.subscribe),
        None,
        "resource subscriptions are not advertised, so none may be accepted"
    );
}

/// Of the three lists, only the tool list moves, and only on the one setting
/// that gates it.
///
/// The resources and prompts lists read `skills.serve` and `harness_onboarded`,
/// both fixed before the first request arrives, so they are accepted on a
/// subscription and then never carry anything. That is the premise behind
/// sending exactly one notification per flip rather than three.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_tool_list_moves_and_only_on_the_setting_that_gates_it() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;
    let peer = client.peer();

    let before = (
        tool_names(peer).await,
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .len(),
    );

    flip_github_enabled(peer).await;
    let _ = peer
        .call_tool(
            CallToolRequestParams::new("add_domain".to_string()).with_arguments(
                json!({ "domain": "second", "virtual": true })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await;

    let after = (
        tool_names(peer).await,
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .len(),
    );
    assert_eq!(
        after.0.len(),
        before.0.len() + 5,
        "the tool list moved by the five collaboration tools: {:?} -> {:?}",
        before.0,
        after.0
    );
    assert_eq!(
        (before.1, before.2),
        (after.1, after.2),
        "and neither of the other two lists moved at all"
    );
}

/// The lifecycle these tests drive really is the modern one, so none of the
/// subscription assertions above can pass on a connection that would never
/// reach the subscription branch at all. **A guard rather than a red:** it
/// passes before and after this task.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_modern_lifecycle_is_what_these_tests_actually_drive() {
    let h = Harness::new().await;
    let (client, _server, _recorder) = h.connect_modern().await;

    // A legacy connection answers `initialize`; a modern one never sends it and
    // learns the same version from `server/discover`.
    let info = client.peer().peer_info().expect("the peer published info");
    assert_eq!(info.protocol_version, newest_served());
    assert!(
        ClientInfo::default().protocol_version >= ProtocolVersion::V_2025_03_26,
        "sanity: the rmcp client default is a real revision"
    );
}
