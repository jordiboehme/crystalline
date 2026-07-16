//! In-process rmcp duplex tests for the degraded status server.
//!
//! A `tokio::io::duplex` pair connects a real rmcp client to a
//! [`DegradedServer`] in the same process, driving the actual JSON-RPC
//! initialize handshake, `tools/list` and `tools/call`. `StubStatus`'s fields
//! are public, so the status is built directly with no lock, socket or
//! environment involved; the same duplex pattern as `tests/mcp_instructions.rs`.

use crystalline_service::{DegradedServer, StubStatus};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, RoleServer};
use serde_json::Value;

/// A newer-daemon-on-mcpb status: the upgrade-skew case whose copy points at
/// the releases page, so a single fixture exercises the interesting path.
fn mcpb_skew_status() -> StubStatus {
    StubStatus {
        reason: "cannot run an embedded MCP server: another Crystalline instance owns the index (pid 4242)".to_string(),
        binary_version: crystalline_core::VERSION.to_string(),
        daemon_version: Some("99.0.0".to_string()),
        daemon_pid: Some(4242),
        channel: Some("mcpb".to_string()),
    }
}

/// Open one rmcp connection to a degraded server carrying `status`. The server
/// handshake blocks until the client sends `initialize`, so the two run
/// concurrently, exactly as in the healthy-server harness.
async fn connect(
    status: StubStatus,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, DegradedServer>,
) {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task =
        tokio::spawn(
            async move { rmcp::serve_server(DegradedServer::new(status), server_io).await },
        );
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    (client, server)
}

/// The text of a tool result's first content block.
fn first_text(result: &rmcp::model::CallToolResult) -> String {
    let v = serde_json::to_value(result).unwrap();
    v.pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_succeeds_and_identifies_as_crystalline_with_degraded_copy() {
    let (client, _server) = connect(mcpb_skew_status()).await;
    let info = client
        .peer()
        .peer_info()
        .expect("the server sent its handshake");
    assert_eq!(
        info.server_info.name, "crystalline",
        "identifies as crystalline"
    );
    assert_eq!(
        info.server_info.version,
        crystalline_core::VERSION,
        "at this binary's version"
    );
    let instructions = info
        .instructions
        .clone()
        .expect("degraded instructions present");
    assert!(
        instructions.contains("degraded mode"),
        "instructions announce degraded mode:\n{instructions}"
    );
    assert!(
        instructions.contains("https://github.com/jordiboehme/crystalline/releases"),
        "the mcpb skew points at the releases page:\n{instructions}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_list_exposes_exactly_the_status_tool() {
    let (client, _server) = connect(mcpb_skew_status()).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        ["status"],
        "exactly one tool named status: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calling_status_returns_the_json_payload() {
    let (client, _server) = connect(mcpb_skew_status()).await;
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("status"))
        .await
        .unwrap();
    let payload: Value = serde_json::from_str(&first_text(&result)).expect("payload is JSON");
    assert_eq!(payload["available"], serde_json::json!(false));
    assert_eq!(payload["daemon_version"], serde_json::json!("99.0.0"));
    assert_eq!(payload["daemon_pid"], serde_json::json!(4242));
    assert_eq!(payload["channel"], serde_json::json!("mcpb"));
    assert!(
        payload["fix"].as_str().is_some_and(|f| !f.is_empty()),
        "fix present: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_real_tool_call_is_a_tool_error_carrying_the_fix() {
    let (client, _server) = connect(mcpb_skew_status()).await;
    // A client replaying a cached tool list from a healthy session calls a real
    // tool by name; it must learn why it failed, not get an opaque protocol
    // error, so the call succeeds at the protocol level with is_error set.
    let result = client
        .peer()
        .call_tool(CallToolRequestParams::new("search_engrams"))
        .await
        .expect("a stale tool call is a tool-level error, not a protocol error");
    assert_eq!(
        result.is_error,
        Some(true),
        "the result is flagged an error"
    );
    let text = first_text(&result);
    assert!(
        text.contains("install it over the current extension"),
        "the error carries the fix to relay:\n{text}"
    );
}
