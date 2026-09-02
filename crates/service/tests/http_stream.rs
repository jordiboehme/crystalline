//! Drives the HTTP endpoint's real router (`crystalline_service::daemon::http_router`,
//! the same construction `run_http` mounts) over a live TCP listener and reads the
//! raw SSE wire bytes, so a regression in the rmcp config can't hide behind a
//! client library that silently tolerates the extra priming frame.
//!
//! AWS Bedrock AgentCore Gateway's strict SSE parser rejects rmcp's SEP-1699
//! priming event (an empty `data:` line followed by `id:`/`retry:`) ahead of the
//! JSON-RPC response; the MCP Python SDK never emits it and single-`data:`-event
//! streams are the ecosystem baseline. This test speaks raw HTTP/1.1 (no new
//! dependency) so the assertions see exactly what a gateway parser sees.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// Build the same kind of engine the other service integration tests use: a
/// real temp-directory domain (files are the source of truth) synced into an
/// in-memory Turso store, response format pinned to plain JSON so assertions
/// don't have to account for TOON framing.
async fn build_engine() -> (tempfile::TempDir, Arc<Engine>) {
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
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

/// Bind `http_router` on an ephemeral loopback port and serve it on a
/// background task for the duration of the test.
async fn spawn_router() -> std::net::SocketAddr {
    spawn_router_counting().await.0
}

/// As [`spawn_router`], handing back the `http_sessions` counter the daemon
/// reports through `ctl status` so a test can read it between requests.
async fn spawn_router_counting() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let (_tmp, engine) = build_engine().await;
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&_tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    let sessions = Arc::new(AtomicUsize::new(0));
    let router = http_router(engine, sessions.clone(), &[], auth, None).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Leak the temp dir's engine/store for the server's lifetime; the test
        // process exits at the end of the test function anyway.
        let _tmp = _tmp;
        // Served the way `run_http` serves it, connect info included: the peer
        // address the first-run setup route reads lives in the extensions this
        // adds, and a plain router would leave it missing.
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, sessions)
}

/// Send one raw HTTP/1.1 POST over a fresh connection and read back whatever
/// arrives within a bounded window. The streamable-HTTP response is chunked
/// SSE with no natural end-of-message the client can wait for (the session
/// stays open for further requests), so this reads for a fixed short window
/// rather than until EOF; assertions below use substring checks so chunk-size
/// framing lines never need to be stripped.
async fn post(addr: std::net::SocketAddr, body: &str, session_id: Option<&str>) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut request = "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Connection: close\r\n"
        .to_string();
    if let Some(id) = session_id {
        request.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    // A write error is a legitimate outcome rather than a test failure: the
    // body limit below answers 413 and closes the connection the moment the
    // accumulated frames pass the cap (rmcp 3.1.2
    // `transport/common/server_side_http.rs:205-250`), which can happen while
    // this side is still writing. Read whatever came back either way.
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
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Pull the `mcp-session-id` response header out of a raw HTTP response's
/// head, case-insensitively (header names are case-insensitive on the wire).
fn extract_session_id(raw: &str) -> String {
    for line in raw.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("mcp-session-id")
        {
            return value.trim().to_string();
        }
    }
    panic!("no mcp-session-id header in response:\n{raw}");
}

/// Assert the properties a strict SSE parser (AWS Bedrock AgentCore Gateway
/// among them) requires: the first event is the JSON-RPC payload itself, with
/// no priming frame ahead of it.
fn assert_no_priming_frame(raw: &str, context: &str) {
    assert!(
        raw.contains("data: {\"jsonrpc\""),
        "{context}: expected the first SSE data line to carry the JSON-RPC response, got:\n{raw}"
    );
    assert!(
        !raw.contains("\nretry:") && !raw.starts_with("retry:"),
        "{context}: found a `retry:` line, which strict SSE parsers reject:\n{raw}"
    );
    assert!(
        !raw.contains("data: \n"),
        "{context}: found the empty-data priming shape `data: \\nid:`:\n{raw}"
    );
}

/// An `initialize` body declaring `version`, with no `_meta`: the shape every
/// client that speaks a revision below 2026-07-28 sends.
fn init_body(version: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{version}","capabilities":{{}},"clientInfo":{{"name":"http-stream-test","version":"0.0.0"}}}}}}"#
    )
}

/// Every revision we advertise is served at the HTTP handshake, and an
/// `initialize` always takes the session branch whatever revision it names.
///
/// `is_legacy_request` (rmcp 3.2.0 `tower.rs:359-416`) returns `true` for an
/// `InitializeRequest` before it looks at any version at all, because the
/// handshake exists only in the revisions before 2026-07-28; the session branch
/// is the only site that inserts `Mcp-Session-Id` (`tower.rs:1911`). The answer
/// then comes from `negotiate_protocol_version` (`service/server.rs:479`),
/// which echoes a legacy revision and substitutes the newest legacy one for
/// anything else. A modern peer never sends `initialize` - it carries the
/// SEP-2575 `_meta` on each request and routes statelessly, which
/// `tests/mcp_modern_era.rs` covers. Driven off `SERVED_PROTOCOL_VERSIONS`
/// rather than a literal list, so a revision added without a decision about its
/// session model fails here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_revision_we_serve_is_handshaken_as_legacy_and_gets_a_session() {
    let addr = spawn_router().await;
    let newest_handshake = crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS
        .iter()
        .map(|v| v.as_str())
        .rfind(|v| *v < "2026-07-28")
        .expect("we serve at least one revision with a handshake");
    for version in crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS {
        let version = version.as_str();
        let response = post(addr, &init_body(version), None).await;
        assert!(
            response.starts_with("HTTP/1.1 200 "),
            "{version} initialize is served:\n{response}"
        );
        let answered = if version < "2026-07-28" {
            version
        } else {
            newest_handshake
        };
        assert!(
            response.contains(&format!("\"protocolVersion\":\"{answered}\"")),
            "{version} is answered {answered}:\n{response}"
        );
        assert!(
            !extract_session_id(&response).is_empty(),
            "{version} gets a session id, because an initialize is legacy \
             whatever it names:\n{response}"
        );
    }
}

/// A version outside our advertised set is refused at the HTTP handshake
/// instead of being negotiated down into a wedge.
///
/// The wedge this closes: `use_session` (`tower.rs:1727`) reads the version off
/// the *request* and compares it against 2026-07-28, never against our list, so
/// an `initialize` at or above that revision routes statelessly and gets no
/// `Mcp-Session-Id`. Whatever version the handshake then answers with, every
/// follow-up that does not itself declare 2026-07-28 takes the session branch,
/// has no session id to present, and gets `422 Unprocessable Entity: Unexpected
/// message, expect initialize request` (`tower.rs:1833`/`:1851`) for the rest
/// of its life. Observed on this endpoint before the refusal existed: a 200
/// handshake carrying `"protocolVersion":"2026-07-28"` and no session header,
/// then `HTTP/1.1 422 Unprocessable Entity` on a plain `tools/list`.
///
/// **The trigger class shrank when the era was adopted, and the branch stayed.**
/// 2026-07-28 is served now, so a client naming it is answered it and routes
/// statelessly throughout, which is what `mcp_modern_era.rs` drives. What is
/// left is the case that never depended on any decision of ours: a string
/// sorting at or above 2026-07-28 that no revision matches. `ProtocolVersion`
/// deserializes any string (`model.rs:204-220`) and the comparison is
/// lexicographic, so a future-dated or malformed version takes the stateless
/// route while being a revision nobody implements, and answering it with one of
/// ours would leave the original wedge exactly as it was.
///
/// The refusal's wire shape is read from source, not assumed:
/// `StreamableHttpServerConfig::default()` leaves `json_response` false
/// (`tower.rs:169`) and the plain stateless path answers through
/// `stateless_sse_response` (`tower.rs:2027`) without the status mapper, so
/// this arrives as **HTTP 200 with an SSE-framed JSON-RPC error**, code -32022
/// (`model.rs:546`, `model.rs:601-613`), never a 4xx.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_we_do_not_serve_is_refused_at_the_http_handshake() {
    let addr = spawn_router().await;
    // Both are strings sorting at or above 2026-07-28 that match no revision:
    // one future-dated, one not a date at all. They take the identical path.
    for version in ["2027-01-01", "zzzz"] {
        let response = post(addr, &init_body(version), None).await;
        assert!(
            response.starts_with("HTTP/1.1 200 "),
            "{version} is refused in-band, not by status:\n{response}"
        );
        assert!(
            response.to_ascii_lowercase().contains("text/event-stream"),
            "{version} refusal is SSE framed:\n{response}"
        );
        assert!(
            response.contains("\"code\":-32022"),
            "{version} refusal carries the unsupported-protocol-version code:\n{response}"
        );
        assert!(
            response.contains("\"supported\":[\"2024-11-05\""),
            "{version} refusal names what we do serve:\n{response}"
        );
        assert!(
            !response.contains("\"result\""),
            "{version} gets no handshake result:\n{response}"
        );
    }

    // The refusal does not poison the endpoint: a fresh initialize at a version
    // we serve still gets a session. (A bare follow-up would get the 422 above,
    // which has nothing to do with poisoning.)
    let response = post(addr, &init_body("2025-06-18"), None).await;
    assert!(
        !extract_session_id(&response).is_empty(),
        "a served version still works after a refusal:\n{response}"
    );
}

/// An `initialize` body padded with a repeated character to exactly
/// `total_len` bytes, so a test can sit on the limit rather than near it. The
/// padding rides in `clientInfo.name`, which is a plain JSON string with no
/// escaping in it, so the byte count is the character count.
fn init_body_of_exactly(total_len: usize) -> String {
    let prefix = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":""#;
    let suffix = r#"","version":"0.0.0"}}}"#;
    let padding = total_len
        .checked_sub(prefix.len() + suffix.len())
        .expect("asked for a body smaller than the JSON-RPC envelope itself");
    format!("{prefix}{}{suffix}", "x".repeat(padding))
}

/// A request body exactly at the cap is served, and one byte past it is
/// refused with `413`.
///
/// **This boundary is new, not inherited.** rmcp 2.2.0 read the whole body
/// with no cap at all (`rmcp-2.2.0/src/transport/common/server_side_http.rs:171-201`,
/// `expect_json(body)` with no limit argument), so every body size that
/// arrives today is accepted. rmcp 3.1.2 takes a cap
/// (`server_side_http.rs:205-250`) and defaults it to 4 MiB
/// (`DEFAULT_MAX_REQUEST_BODY_BYTES`, `tower.rs:55`), which would start
/// refusing engram writes this project's own corpus contains. We therefore
/// choose the number, and we choose the one the REST API already enforces:
/// `crystalline_service::rest::MAX_BODY_BYTES`, 10 MiB.
///
/// The boundary is inclusive at both ends by rmcp's own arithmetic: the check
/// is `data.remaining() > max_bytes.saturating_sub(collected.len())`, so a
/// body of exactly `max_bytes` passes whatever the frame boundaries are, and
/// `max_bytes + 1` cannot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_at_the_limit_is_served_and_one_byte_over_is_refused() {
    let addr = spawn_router().await;
    let limit = crystalline_service::rest::MAX_BODY_BYTES;

    let at_the_limit = post(addr, &init_body_of_exactly(limit), None).await;
    assert!(
        at_the_limit.starts_with("HTTP/1.1 200 "),
        "a body of exactly {limit} bytes is served:\n{}",
        head_of(&at_the_limit)
    );
    assert!(
        at_the_limit.contains("\"protocolVersion\":\"2025-06-18\""),
        "and it reaches the handshake rather than being truncated:\n{}",
        head_of(&at_the_limit)
    );

    let one_over = post(addr, &init_body_of_exactly(limit + 1), None).await;
    assert!(
        one_over.starts_with("HTTP/1.1 413 Payload Too Large"),
        "one byte past {limit} is refused by status:\n{}",
        head_of(&one_over)
    );
    // rmcp's own words, formatted with the cap we set
    // (`server_side_http.rs:224-231`), so the operator reading the response
    // sees the number they can change.
    assert!(
        one_over.contains(&format!(
            "Payload Too Large: request body exceeds {limit} bytes"
        )),
        "and the refusal names our cap rather than rmcp's 4 MiB default:\n{}",
        head_of(&one_over)
    );

    // The refusal is the transport's, not the session's: the endpoint still
    // works for the next client.
    let after = post(addr, &init_body("2025-06-18"), None).await;
    assert!(
        !extract_session_id(&after).is_empty(),
        "a normal handshake still works after an oversized body:\n{after}"
    );
}

/// Trim a raw response down to its head plus the first line of body, so an
/// assertion message about a 10 MiB request does not print a 10 MiB response.
fn head_of(raw: &str) -> String {
    raw.chars().take(600).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_post_responses_carry_no_sse_priming_frame() {
    let addr = spawn_router().await;

    let init_body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"http-stream-test","version":"0.0.0"}}}"#;
    let init_response = post(addr, init_body, None).await;
    assert_no_priming_frame(&init_response, "initialize response");
    let session_id = extract_session_id(&init_response);

    let initialized_body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let _ = post(addr, initialized_body, Some(&session_id)).await;

    let tools_list_body = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let tools_list_response = post(addr, tools_list_body, Some(&session_id)).await;
    assert_no_priming_frame(&tools_list_response, "tools/list response");
}

// ---------------------------------------------------------------------------
// The wire-format baseline.
// ---------------------------------------------------------------------------

/// Send one raw HTTP/1.1 POST carrying the SEP-2243 standard headers, which is
/// how a client that speaks the 2026-07-28 era addresses this endpoint.
///
/// `MCP-Protocol-Version` at or above 2026-07-28 is what arms rmcp's
/// `validate_standard_headers` (`tower.rs:673-700`); below that it early-returns
/// and the other two headers are not looked at. `Mcp-Method` must equal the
/// body's method or the request is refused with `-32020` before any handler
/// runs, and `Mcp-Name` is required for the methods that name a thing
/// (`tools/call`, `resources/read`, ...), compared literally unless it is
/// wrapped in the base64 markers (`mcp_headers.rs:194-206`).
async fn post_with_standard_headers(
    addr: std::net::SocketAddr,
    body: &str,
    version: &str,
    method: &str,
) -> String {
    post_with_named_headers(addr, body, version, method, None).await
}

/// [`post_with_standard_headers`] plus the `Mcp-Name` header the methods that
/// name a thing require.
async fn post_with_named_headers(
    addr: std::net::SocketAddr,
    body: &str,
    version: &str,
    method: &str,
    name: Option<&str>,
) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let named = match name {
        Some(name) => format!("Mcp-Name: {name}\r\n"),
        None => String::new(),
    };
    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         MCP-Protocol-Version: {version}\r\n\
         Mcp-Method: {method}\r\n\
         {named}\
         Connection: close\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
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

/// Pull the JSON-RPC payload out of an SSE-framed or plain-JSON response and
/// parse it, so a count can be taken rather than a substring guessed at.
fn payload(raw: &str) -> serde_json::Value {
    let line = raw
        .lines()
        .find_map(|line| {
            line.strip_prefix("data: ")
                .or_else(|| line.starts_with("{\"jsonrpc\"").then_some(line))
        })
        .unwrap_or_else(|| panic!("no JSON-RPC payload in:\n{}", head_of(raw)));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("payload is not JSON ({e}):\n{line}"))
}

/// **The wire-format baseline: what this server puts on the wire today.**
///
/// Recorded before any of the conformance work in this program runs, so a
/// later task can prove it changed exactly what it meant to change. Several of
/// the shapes pinned here are the violations that work fixes; the assertion is
/// not that they are right, it is that they are *known*. Every one names the
/// task expected to move it, and moving one is a deliberate edit to this test
/// rather than a surprise.
///
/// Recorded on rmcp 3.1.2 with `SERVED_PROTOCOL_VERSIONS` at the four
/// revisions below 2026-07-28.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wire_format_baseline_the_conformance_tasks_measure_against() {
    let addr = spawn_router().await;

    // --- the legacy handshake, which is what every client we serve uses today ---
    let init = post(addr, &init_body("2025-06-18"), None).await;
    assert!(init.starts_with("HTTP/1.1 200 OK"));
    assert!(init.to_ascii_lowercase().contains("text/event-stream"));
    let session_id = extract_session_id(&init);
    let handshake = payload(&init);
    assert_eq!(handshake["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(handshake["result"]["serverInfo"]["name"], "crystalline");
    // The routing block rides the handshake result. Task 5 moves this channel
    // to `server/discover`; when it does, this line is what proves the block
    // did not simply vanish.
    assert!(
        handshake["result"]["instructions"]
            .as_str()
            .is_some_and(|s| s.starts_with("CRYSTALLINE KNOWLEDGE ROUTING")),
        "instructions arrive on the initialize result today"
    );
    assert_eq!(
        handshake["result"]["capabilities"]["tools"]["listChanged"], true,
        "all three list-changed capabilities are advertised (`get_info` in mcp.rs)"
    );
    // **Task 6 kept all three deliberately, and this line records the
    // decision.** It removed the unsolicited pushes and gave the capability the
    // only delivery channel the 2026-07-28 era has: a client may open a
    // `subscriptions/listen` stream for exactly these three categories
    // (`tests/mcp_subscriptions.rs`). Nothing is ever sent on it, because after
    // Tasks 4 and 5 no request can move a list; retracting the capability would
    // have been a user-visible change to every client we serve today for no
    // behavioural gain, so it stays and the stream stays silent.

    let _ = post(
        addr,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        Some(&session_id),
    )
    .await;

    // --- the three list endpoints, per connection and uncached ---
    // Counts as served with GitHub off, nothing provisioned and the instance
    // writable, which is the default install. `provisioning_declared` gates
    // the call rather than the listing - `add_domain` and `update_domain` can
    // create a declaration mid-call, and there is no one setting to point at -
    // so `provision` is on every list. `github.enabled` is one shared setting
    // and does gate the listing, so the five collaboration tools that need it
    // are absent here and appear together when it is turned on. Task 5 decides
    // the skills surface at construction instead. The assertion below is the
    // count, read off a failing run rather than computed here.
    let list = |id: u8, method: &'static str| {
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{}}}}"#);
        let session_id = session_id.clone();
        async move { payload(&post(addr, &body, Some(&session_id)).await) }
    };

    let tools = list(2, "tools/list").await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names.len(),
        18,
        "a default install's tools, the `skills` surface among them: {names:?}"
    );
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(
        names, sorted,
        "rmcp's ToolRouter::list_all sorts by name (router/tool.rs:588), which is \
         the spec's deterministic-ordering SHOULD satisfied for free"
    );

    let resources = list(3, "resources/list").await;
    assert_eq!(
        resources["result"]["resources"].as_array().unwrap().len(),
        5,
        "the five served skills"
    );
    let templates = list(4, "resources/templates/list").await;
    let listed = templates["result"]["resourceTemplates"].as_array().unwrap();
    assert_eq!(
        listed.len(),
        1,
        "the one template: every attachment a domain carries. The endpoint also \
         has to be overridden rather than inherited because rmcp's default \
         answers one of the six cache-hint operations with no hints \
         (handler/server.rs:387-395)"
    );
    assert_eq!(
        listed[0]["uriTemplate"].as_str(),
        Some("crystalline://{domain}/assets/{+path}"),
        "the reserved expansion keeps a nested path's separators: {listed:?}"
    );
    assert_eq!(listed[0]["name"].as_str(), Some("attachment"));
    let prompts = list(5, "prompts/list").await;
    assert_eq!(
        prompts["result"]["prompts"].as_array().unwrap().len(),
        2,
        "connector and onboarding"
    );

    let read = payload(
        &post(
            addr,
            r#"{"jsonrpc":"2.0","id":11,"method":"resources/read","params":{"uri":"skill://crystalline-routing/SKILL.md"}}"#,
            Some(&session_id),
        )
        .await,
    );
    assert!(
        read["result"]["contents"][0]["text"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "a skill reads back: {read}"
    );

    // **No caching hints on this session, now or ever.** An earlier version of
    // this comment predicted that Task 7 would flip all four; it does not, and
    // the reason is the assertion rather than an omission. SEP-2549's hints
    // (`ttlMs`, `cacheScope`) and SEP-2322's `resultType` are 2026-07-28 wire
    // shape, and this is a 2025-06-18 session: rmcp strips `resultType` for a
    // peer below the era (`handler/server.rs:246-260`) and our own
    // `CacheHinted` gate withholds the other two on the same test
    // (`RequestContext::protocol_version()`). Emitting them here would be
    // inventing wire shape for a revision that has none, so these five stay
    // absent for the whole program. Advertising the era added a **new**
    // modern-peer leg below rather than moving these lines, which is what that
    // prediction should have said; `tests/mcp_modern_era.rs` carries the rest
    // of the modern surface.
    for (label, result) in [
        ("tools/list", &tools),
        ("resources/list", &resources),
        ("resources/templates/list", &templates),
        ("prompts/list", &prompts),
        ("resources/read", &read),
    ] {
        let object = result["result"].as_object().unwrap();
        for hint in ["resultType", "ttlMs", "cacheScope"] {
            assert!(
                !object.contains_key(hint),
                "{label} carries no {hint} today: {object:?}"
            );
        }
    }

    // --- the modern request shape, which is live already for a served version ---
    // A request carrying per-request `_meta` plus the standard headers routes
    // statelessly: no session id is presented and none is needed. This is the
    // path Task 9 puts every 2026-07-28 client on.
    let modern = post_with_standard_headers(
        addr,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        "2025-11-25",
        "tools/list",
    )
    .await;
    assert!(
        modern.starts_with("HTTP/1.1 200 OK"),
        "a sessionless modern-shaped request at a served version works today:\n{}",
        head_of(&modern)
    );
    assert_eq!(
        payload(&modern)["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        18
    );

    // **Task 9 moved this assertion, deliberately.** The same shape at
    // 2026-07-28 used to be refused `-32022` in the second of the refusal's two
    // wire shapes (`jsonrpc_http_status`, `tower.rs:617-630`, maps it to HTTP
    // 400 with `application/json` on the `serve_negotiated_request_directly`
    // path, `tower.rs:1255`). That refusal now applies only to strings no
    // revision matches, which
    // `a_version_we_do_not_serve_is_refused_at_the_http_handshake` pins in both
    // of its shapes. Here the era is served: statelessly, with its caching
    // hints, and with the same 18 tools every other client of this instance is
    // served (GitHub is off here, so the five collaboration tools are withheld
    // from every era alike).
    // `tests/mcp_modern_era.rs` is where the rest of that surface lives.
    let era = post_with_standard_headers(
        addr,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        "2026-07-28",
        "tools/list",
    )
    .await;
    assert!(
        era.starts_with("HTTP/1.1 200 OK"),
        "the era is served on this endpoint:\n{}",
        head_of(&era)
    );
    assert!(
        !era.split("\r\n")
            .any(|line| line.to_ascii_lowercase().starts_with("mcp-session-id")),
        "and it is sessionless:\n{}",
        head_of(&era)
    );
    let era_result = &payload(&era)["result"];
    assert_eq!(era_result["tools"].as_array().unwrap().len(), 18);
    assert_eq!(era_result["resultType"], "complete");
    assert_eq!(era_result["ttlMs"], 0);
    assert_eq!(era_result["cacheScope"], "public");

    // --- the standard-header rules, inherited from rmcp and already enforced ---
    // Only a client declaring 2026-07-28 or above reaches them, so this bites
    // today regardless of what we advertise. Task 7 owns the consequences.
    let no_header = post(
        addr,
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        None,
    )
    .await;
    assert_eq!(payload(&no_header)["error"]["code"], -32020);
    assert_eq!(
        payload(&no_header)["error"]["message"],
        "request _meta protocolVersion requires MCP-Protocol-Version header"
    );

    let wrong_method_header = post_with_standard_headers(
        addr,
        r#"{"jsonrpc":"2.0","id":9,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        "2026-07-28",
        "prompts/list",
    )
    .await;
    assert_eq!(
        payload(&wrong_method_header)["error"]["message"],
        "Mcp-Method header `prompts/list` does not match body method `tools/list`"
    );

    // --- the bare discover probe, which HTTP does NOT answer natively ---
    // A `server/discover` sent as the first message with no `_meta` and no
    // session is treated as a legacy request, takes the session branch and is
    // answered `422 Unexpected message, expect initialize request`
    // (`server_side_http.rs:196-203`). Task 8 owns the stdio interception and
    // should not assume the HTTP side already works: it does not.
    let bare_probe = post(
        addr,
        r#"{"jsonrpc":"2.0","id":10,"method":"server/discover"}"#,
        None,
    )
    .await;
    assert!(
        bare_probe.starts_with("HTTP/1.1 422 Unprocessable Entity"),
        "a bare discover probe over HTTP is not answered with a DiscoverResult:\n{}",
        head_of(&bare_probe)
    );
    assert!(bare_probe.contains("Unexpected message, expect initialize request"));

    // --- the body limit, the one boundary this task introduced ---
    // Kept in the baseline too so the number is visible beside the shapes it
    // guards; the both-directions proof is its own test above.
    let over = post(
        addr,
        &init_body_of_exactly(crystalline_service::rest::MAX_BODY_BYTES + 1),
        None,
    )
    .await;
    assert!(over.starts_with("HTTP/1.1 413 Payload Too Large"));
}

/// **`http_sessions` counts sessions, not service constructions.**
///
/// `Shared::http_session_count` (`daemon.rs`) is reported as `http_sessions` by
/// the daemon's `ctl status`, and its only writer used to be the service
/// factory `StreamableHttpService::new` is handed. That factory is
/// `get_service()`, which rmcp 3.1.2 calls at five sites
/// (`tower.rs:1280`, `:1426`, `:1822`, `:1855`, `:1948`) and only one of them
/// - `:1855` - creates a session. The other four are the SEP-2243 tool-schema
/// cache, the external-store restore replay, the stateless `server/discover`
/// branch and **every stateless POST**, so the counter was reading a number
/// with no name.
///
/// Two of the four are reachable on the tree as it stands, which is why this is
/// a genuine red rather than a note for the task that advertises the era:
///
/// - a sessionless modern-shaped POST at an advertised revision is served
///   statelessly today (the baseline above pins that it works), and took the
///   `:1948` construction with it;
/// - `validate_standard_headers` arms on the **client's**
///   `MCP-Protocol-Version` header rather than on our advertised set
///   (`tower.rs:678-684`), so a `tools/call` whose header names 2026-07-28
///   reaches the tool-schema cache at `:1280` before the request is refused
///   `-32022` for a version we do not serve. That one POST used to move the
///   counter twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_sessions_counts_sessions_rather_than_service_constructions() {
    let (addr, sessions) = spawn_router_counting().await;
    let count = || sessions.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(count(), 0, "nothing has connected yet");

    // A legacy handshake is a session, and is the one thing that should count.
    let init = post(addr, &init_body("2025-06-18"), None).await;
    let session_id = extract_session_id(&init);
    assert_eq!(count(), 1, "the legacy handshake created one session");

    // Further requests on that session reuse the session worker.
    let _ = post(
        addr,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        Some(&session_id),
    )
    .await;
    let _ = post(
        addr,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        Some(&session_id),
    )
    .await;
    assert_eq!(count(), 1, "requests on a live session create no session");

    // A sessionless modern-shaped request at a revision we serve. It is
    // answered (the baseline pins the 200 and the tool count), it is by
    // definition not a session - no `Mcp-Session-Id` is presented and none is
    // returned - and it must not move the counter.
    let stateless = post_with_standard_headers(
        addr,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2025-11-25","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        "2025-11-25",
        "tools/list",
    )
    .await;
    assert!(
        stateless.starts_with("HTTP/1.1 200 OK"),
        "the stateless request is served:\n{}",
        head_of(&stateless)
    );
    assert_eq!(
        count(),
        1,
        "a stateless POST is not a session:\n{}",
        head_of(&stateless)
    );

    // A `tools/call` whose header declares the era reaches the SEP-2243
    // tool-schema cache during header validation, which builds a service of its
    // own, and then dispatches. **Task 9 moved this leg**: the same request used
    // to be refused `-32022` for a revision we did not serve, and the counter
    // had to stay put through a refusal; now it has to stay put through a
    // served call, which is the stronger version of the same assertion - two
    // service constructions, one request, no session.
    let schema_probe = post_with_named_headers(
        addr,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_engrams","arguments":{"query":"anything"},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        "2026-07-28",
        "tools/call",
        Some("search_engrams"),
    )
    .await;
    assert_eq!(
        payload(&schema_probe)["result"]["isError"],
        false,
        "the era is served, so the call runs:\n{}",
        head_of(&schema_probe)
    );
    assert_eq!(
        count(),
        1,
        "neither the schema cache nor the stateless dispatch is a session:\n{}",
        head_of(&schema_probe)
    );
}
