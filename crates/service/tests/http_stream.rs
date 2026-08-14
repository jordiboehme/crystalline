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
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();

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
