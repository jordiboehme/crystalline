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
    let (_tmp, engine) = build_engine().await;
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&_tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    let router = http_router(engine, Arc::new(AtomicUsize::new(0)), &[], auth, None).unwrap();
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
    addr
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

/// Every revision we advertise still gets the legacy HTTP treatment: a session
/// id and its own version echoed. `is_legacy_request` (rmcp 3.1.2
/// `tower.rs:358-408`) routes anything below 2026-07-28 through the session
/// branch, which is the only site that inserts `Mcp-Session-Id`
/// (`tower.rs:1911`), so this pins the transport behaviour our advertised set
/// keeps working rather than merely the negotiated string.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_revision_we_serve_gets_a_session_and_its_version_back() {
    let addr = spawn_router().await;
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
        let response = post(addr, &init_body(version), None).await;
        assert!(
            response.starts_with("HTTP/1.1 200 "),
            "{version} initialize is served:\n{response}"
        );
        let session_id = extract_session_id(&response);
        assert!(
            !session_id.is_empty(),
            "{version} gets a session id:\n{response}"
        );
        assert!(
            response.contains(&format!("\"protocolVersion\":\"{version}\"")),
            "{version} is echoed back:\n{response}"
        );
    }
}

/// A version outside our advertised set is refused at the HTTP handshake
/// instead of being negotiated down into a wedge.
///
/// The wedge this closes: `use_session` (`tower.rs:1727`) reads the version off
/// the *request* and validates it against rmcp's crate-wide `KNOWN_VERSIONS`,
/// never against our narrowed list, so an `initialize` at 2026-07-28 routes
/// statelessly and gets no `Mcp-Session-Id`. Whatever version the handshake
/// then answers with, every follow-up that does not itself declare 2026-07-28
/// takes the session branch, has no session id to present, and gets
/// `422 Unprocessable Entity: Unexpected message, expect initialize request`
/// (`tower.rs:1833`/`:1851`) for the rest of its life. Observed on this
/// endpoint before the refusal existed: a 200 handshake carrying
/// `"protocolVersion":"2026-07-28"` and no session header, then
/// `HTTP/1.1 422 Unprocessable Entity` on a plain `tools/list`.
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
    // 2026-07-28 is a real revision rmcp knows and we do not yet honour;
    // 2027-01-01 is any string sorting at or above it, which `ProtocolVersion`
    // deserializes happily (`model.rs:204-220`) and which takes the identical
    // path.
    for version in ["2026-07-28", "2027-01-01"] {
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
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST / HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         MCP-Protocol-Version: {version}\r\n\
         Mcp-Method: {method}\r\n\
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
        "all three list-changed capabilities are advertised (mcp.rs:1143-1152)"
    );

    let _ = post(
        addr,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        Some(&session_id),
    )
    .await;

    // --- the three list endpoints, per connection and uncached ---
    // Counts as served to a client that is not receipt-matched, with GitHub
    // off, nothing provisioned and the instance writable. Task 4 makes these
    // lists invariant (the tool count moves as gates come off the listing) and
    // Task 5 decides the skills surface at construction instead. The
    // `tools/list` payload measured 32288 bytes on the day this was recorded,
    // which is the before-number for Task 4's byte-delta measurement.
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
        17,
        "17 tools today, the `skills` surface among them: {names:?}"
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
    assert_eq!(
        templates["result"]["resourceTemplates"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "rmcp's default list_resource_templates, which Task 7 has to override for \
         its cache hints (handler/server.rs:383-393)"
    );
    let prompts = list(5, "prompts/list").await;
    assert_eq!(
        prompts["result"]["prompts"].as_array().unwrap().len(),
        2,
        "connector and onboarding"
    );

    // No caching hints anywhere yet. The 2026-07-28 revision makes them a MUST
    // on six operations; Task 7 adds them and every one of these four flips.
    for (label, result) in [
        ("tools/list", &tools),
        ("resources/list", &resources),
        ("resources/templates/list", &templates),
        ("prompts/list", &prompts),
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
        17
    );

    // The same shape at the era we do not yet serve is refused, and this is the
    // **second** wire shape of the same refusal: `jsonrpc_http_status`
    // (`tower.rs:617-630`) maps -32022 to HTTP 400 on the
    // `serve_negotiated_request_directly` path (`tower.rs:1255`), and the body
    // is plain `application/json` rather than an SSE frame. A plain
    // `initialize` carrying neither `_meta` nor the headers takes the other
    // path and keeps its 200 SSE shape, which
    // `a_version_we_do_not_serve_is_refused_at_the_http_handshake` pins.
    // Task 9 turns both of these into served responses.
    let refused = post_with_standard_headers(
        addr,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}"#,
        "2026-07-28",
        "tools/list",
    )
    .await;
    assert!(
        refused.starts_with("HTTP/1.1 400 Bad Request"),
        "the negotiated-direct path maps the refusal to a status:\n{}",
        head_of(&refused)
    );
    assert!(
        refused.to_ascii_lowercase().contains("application/json"),
        "and answers plain JSON, not SSE:\n{}",
        head_of(&refused)
    );
    assert_eq!(payload(&refused)["error"]["code"], -32022);

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
