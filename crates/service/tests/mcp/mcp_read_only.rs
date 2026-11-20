//! Read-only domains over MCP: every write tool answers the refusal as a
//! tool result, and every route to switching the flag off is refused, the
//! machine owner's stdio session included (spec A-1).

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\n## Part\n\n- [fact] a fact about alpha #eng\n- [fact] a second fact about alpha #eng\n- [fact] a third fact about alpha #eng\n";

/// A7's sentence, which every surface but the switch-off refusal speaks.
const LOCKED: &str =
    "'archive' is read-only; its owner switches read_only off in the domain policies";

/// The MCP switch-off refusal (spec A-1).
const NOT_OVER_MCP: &str =
    "'archive' is read-only. Only a person switches it off, in the domain policies; ask the owner.";

const YAML: &str = "verify:\n  rules:\n    E008: warning\n";

fn manifest(name: &str, read_only: Option<&str>) -> String {
    let line = read_only
        .map(|v| format!("read_only: {v}\n"))
        .unwrap_or_default();
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n{line}---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
    )
}

pub(crate) struct Harness {
    _tmp: tempfile::TempDir,
    pub(crate) root: std::path::PathBuf,
    pub(crate) engine: Arc<Engine>,
}

impl Harness {
    pub(crate) async fn new() -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        for (name, flag) in [("archive", Some("on")), ("eng", None)] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("MANIFEST.md"), manifest(name, flag)).unwrap();
            std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
            cfg.domains.insert(name.to_string(), DomainEntry::file(dir));
        }
        std::fs::write(root.join("archive/.crystalline.yaml"), YAML).unwrap();
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
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

    /// A real rmcp client over a duplex pair: the stdio transport, which is
    /// the machine owner (`Scope::Unrestricted`).
    pub(crate) async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 18);
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let client = rmcp::serve_client((), client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }

    fn bytes(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.root.join(rel)).unwrap()
    }
}

/// Call a tool and return the whole result as JSON. A protocol error panics,
/// which is the point: every refusal here must be a tool result.
pub(crate) async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Value {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    serde_json::to_value(peer.call_tool(params).await.unwrap()).unwrap()
}

/// The text of a tool result's first block.
pub(crate) fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_write_tool_refuses_with_the_sentence_as_a_tool_result() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    let before = (h.bytes("archive/alpha.md"), h.bytes("archive/MANIFEST.md"));
    for (tool, args) in [
        (
            "write_engram",
            json!({ "domain": "archive", "title": "Beta", "content": "- [fact] x #eng" }),
        ),
        (
            "edit_engram",
            json!({ "domain": "archive", "identifier": "alpha", "operation": "append", "content": "- [fact] y #eng" }),
        ),
        (
            "edit_engram",
            json!({ "domain": "archive", "identifier": "alpha", "operation": "set_frontmatter", "key": "status", "value": "draft" }),
        ),
        (
            "split_engram",
            json!({ "domain": "archive", "identifier": "alpha", "title": "Part", "sections": ["## Part"] }),
        ),
        (
            "delete_engram",
            json!({ "domain": "archive", "identifier": "alpha" }),
        ),
        (
            "move_engram",
            json!({ "domain": "archive", "identifier": "alpha", "destination": "x", "destination_domain": "eng" }),
        ),
        (
            "move_engram",
            json!({ "domain": "eng", "identifier": "alpha", "destination": "x", "destination_domain": "archive" }),
        ),
    ] {
        let result = call(peer, tool, args.clone()).await;
        assert_eq!(result["isError"], true, "{tool} {args}: {result}");
        assert_eq!(text(&result), LOCKED, "{tool} {args}: {result}");
    }
    assert_eq!(
        (h.bytes("archive/alpha.md"), h.bytes("archive/MANIFEST.md")),
        before
    );
}

/// Over MCP nobody switches the flag off, the machine owner included: not
/// with configure set, not with configure unset, not with edit_engram on the
/// MANIFEST in any operation. Each answer is the MCP sentence, and nothing
/// is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_mcp_route_to_off_is_refused_for_the_machine_owner() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    let before = h.bytes("archive/MANIFEST.md");
    for (tool, args) in [
        (
            "configure",
            json!({ "domain": "archive", "set": { "read_only": "off" } }),
        ),
        (
            "configure",
            json!({ "domain": "archive", "unset": ["read_only"] }),
        ),
        (
            "edit_engram",
            json!({ "domain": "archive", "identifier": "manifest", "operation": "find_replace", "find_text": "read_only: on", "content": "read_only: off" }),
        ),
        (
            "edit_engram",
            json!({ "domain": "archive", "identifier": "crystalline://archive/manifest", "operation": "append", "content": "- one more bullet" }),
        ),
        (
            "edit_engram",
            json!({ "domain": "archive", "identifier": "manifest", "operation": "set_frontmatter", "key": "status", "value": "stable" }),
        ),
    ] {
        let result = call(peer, tool, args.clone()).await;
        assert_eq!(result["isError"], true, "{tool} {args}: {result}");
        assert_eq!(text(&result), NOT_OVER_MCP, "{tool} {args}: {result}");
    }
    assert_eq!(h.bytes("archive/MANIFEST.md"), before);
    assert!(h.engine.domain_is_read_only("archive").await);
    // A read still answers.
    let view = call(peer, "configure", json!({ "domain": "archive" })).await;
    assert_ne!(view["isError"], true, "{view}");
}

/// A configure call that mixes the switch-off with a rule override is
/// refused as a whole with the MCP sentence, and neither file moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_mixed_call_on_a_locked_domain_writes_nothing() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    let manifest_before = h.bytes("archive/MANIFEST.md");
    let yaml_before = h.bytes("archive/.crystalline.yaml");
    let result = call(
        peer,
        "configure",
        json!({ "domain": "archive", "set": { "read_only": "off", "rules.E007": "off" } }),
    )
    .await;
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(text(&result), NOT_OVER_MCP, "{result}");
    assert_eq!(h.bytes("archive/MANIFEST.md"), manifest_before);
    assert_eq!(h.bytes("archive/.crystalline.yaml"), yaml_before);

    // A rule override alone is the lock's own refusal.
    let result = call(
        peer,
        "configure",
        json!({ "domain": "archive", "set": { "rules.E007": "off" } }),
    )
    .await;
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(text(&result), LOCKED, "{result}");
    assert_eq!(h.bytes("archive/.crystalline.yaml"), yaml_before);
}

/// The machine owner may still switch a domain ON over MCP when asked (spec
/// A-1): the local agent is the owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_machine_owner_switches_read_only_on_over_mcp() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let result = call(
        client.peer(),
        "configure",
        json!({ "domain": "eng", "set": { "read_only": "on" } }),
    )
    .await;
    assert_ne!(result["isError"], true, "{result}");
    assert!(h.engine.domain_is_read_only("eng").await);
}

/// The read tools carry the mark over MCP too, for the stdio session (the
/// machine owner, so only the lock marks).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_read_tools_mark_the_locked_domain() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    let listed = call(peer, "list_domains", json!({ "include_routing": true })).await;
    let listing: Value = serde_json::from_str(&text(&listed)).unwrap();
    let row = |name: &str| -> Value {
        listing["domains"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {listing}"))
            .clone()
    };
    assert_eq!(row("archive")["read_only"], true, "{listing}");
    assert!(row("eng").get("read_only").is_none(), "{listing}");
    let read = call(
        peer,
        "read_engram",
        json!({ "domain": "archive", "identifier": "alpha" }),
    )
    .await;
    assert_eq!(read["structuredContent"]["read_only"], true, "{read}");
    let open = call(
        peer,
        "read_engram",
        json!({ "domain": "eng", "identifier": "alpha" }),
    )
    .await;
    let open = open["structuredContent"].as_object().unwrap();
    assert!(!open.contains_key("read_only"), "{open:?}");
    let browsed: Value = serde_json::from_str(&text(
        &call(peer, "browse_domain", json!({ "domain": "archive" })).await,
    ))
    .unwrap();
    assert_eq!(browsed["read_only"], true, "{browsed}");
    // The per-folder counts are the REST tree's; the tool keeps its shape.
    assert!(browsed.get("folder_counts").is_none(), "{browsed}");
}

/// B-1 item 2: switching the lock on where it is already on is a no-op that
/// says so. Nothing is written, and the answer is not the switch-off refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setting_read_only_on_where_it_is_on_is_a_no_op() {
    let h = Harness::new().await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();
    let before = h.bytes("archive/MANIFEST.md");
    let answer = call(
        peer,
        "configure",
        json!({ "domain": "archive", "set": { "read_only": "on" } }),
    )
    .await;
    let body = text(&answer);
    assert_ne!(answer["isError"], true, "{answer}");
    assert!(body.contains("'archive' is already read-only"), "{body}");
    assert!(!body.contains("Only a person switches it off"), "{body}");
    assert_eq!(h.bytes("archive/MANIFEST.md"), before);
}
