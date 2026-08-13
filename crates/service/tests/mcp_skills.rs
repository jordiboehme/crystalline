//! In-process rmcp duplex tests for the skill-serving surface: the `skills`
//! tool, the `skill://` resources and the onboarding and connector prompts.
//!
//! Same harness shape as `mcp_tools.rs` (a `tokio::io::duplex` pair driving
//! the real `McpServer` over JSON-RPC), narrowed to what a remote client that
//! never runs the CLI can read, and to the one `skills.serve` gate all three
//! surfaces share. The last three tests cover that gate's `auto` default,
//! which is decided per connection from the client's `initialize` name and
//! this machine's install receipt: `connect_as` drives a handshake under a
//! chosen client name against a stand-in receipt.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams};
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};
use tokio::sync::Mutex;

/// Every skill this binary ships, in served order.
const SKILL_NAMES: [&str; 5] = [
    "crystalline-routing",
    "crystalline-capture",
    "crystalline-schema",
    "crystalline-collaboration",
    "crystalline-intelligence",
];

struct Harness {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
}

impl Harness {
    async fn new(domains: &[&str]) -> Harness {
        Harness::build(domains, false).await
    }

    /// A harness whose engine serves the content API read-only, where the
    /// `skills` tool must stay visible: reading a skill is a read.
    async fn new_read_only(domains: &[&str]) -> Harness {
        Harness::build(domains, true).await
    }

    async fn build(domains: &[&str], read_only: bool) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        for d in domains {
            let dir = root.join(d);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("MANIFEST.md"),
                format!(
                    "---\ntype: manifest\ntitle: {d}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {d}\n\n## Scope\n\n- Everything about {d}\n\n## When to Use\n\n- Route here for {d} questions\n"
                ),
            )
            .unwrap();
            cfg.domains.insert(d.to_string(), DomainEntry::file(dir));
        }
        // Pin JSON so the tool assertions stay on data semantics rather than
        // on the TOON encoding, which has its own tests.
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
                .with_read_only(read_only),
        );
        engine.sync(None).await.unwrap();
        Harness { _tmp: tmp, engine }
    }

    async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, ()>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let server_task =
            tokio::spawn(
                async move { rmcp::serve_server(McpServer::new(engine), server_io).await },
            );
        let client = rmcp::serve_client((), client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }

    /// Open a stdio connection whose client announces itself as `client_name`,
    /// against a server reading this harness's stand-in install receipt. This
    /// is how the per-connection `auto` gate is exercised.
    async fn connect_as(
        &self,
        client_name: &str,
    ) -> (
        RunningService<RoleClient, rmcp::model::ClientInfo>,
        RunningService<rmcp::RoleServer, McpServer>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let engine = self.engine.clone();
        let receipt = self.receipt_path();
        let server_task = tokio::spawn(async move {
            rmcp::serve_server(
                McpServer::new(engine).with_install_receipt(receipt),
                server_io,
            )
            .await
        });
        let mut info = rmcp::model::ClientInfo::default();
        info.client_info = rmcp::model::Implementation::new(client_name, "1.2.3");
        let client = rmcp::serve_client(info, client_io).await.unwrap();
        let server = server_task.await.unwrap().unwrap();
        (client, server)
    }

    /// Where this harness's stand-in install receipt lives.
    fn receipt_path(&self) -> std::path::PathBuf {
        self._tmp.path().join("installs.json")
    }

    /// Write an install receipt in the shape `crystalline install` writes,
    /// recording `harness` with its session hooks wired.
    fn write_hooked_receipt(&self, harness: &str) {
        std::fs::write(
            self.receipt_path(),
            format!(
                r#"{{"format":1,"installs":[{{"harness":"{harness}","scope":"user","version":"0.11.0","parts":{{"mcp":true,"hooks":true,"skills":true}},"skills":[]}}]}}"#
            ),
        )
        .unwrap();
    }
}

/// Call a tool, returning the raw text of its first content block on success.
async fn call_text(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<String, String> {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match peer.call_tool(params).await {
        Ok(result) => {
            let v = serde_json::to_value(&result).unwrap();
            Ok(v.pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

async fn tool_names(peer: &Peer<RoleClient>) -> Vec<String> {
    peer.list_tools(Default::default())
        .await
        .unwrap()
        .tools
        .iter()
        .map(|t| t.name.to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_skills_tool_indexes_every_shipped_skill_and_serves_one_in_full() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let index: Value = serde_json::from_str(&call_text(peer, "skills", json!({})).await.unwrap())
        .expect("the index is JSON");
    let listed = index["skills"].as_array().expect("skills array");
    let names: Vec<&str> = listed
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, SKILL_NAMES, "every shipped skill is indexed");
    for entry in listed {
        let description = entry["description"].as_str().unwrap_or_default();
        assert!(
            !description.is_empty(),
            "a skill without a description cannot be routed to: {entry}"
        );
    }

    // Fetching one by name returns the playbook verbatim, not JSON-wrapped.
    let routing = call_text(peer, "skills", json!({ "name": "crystalline-routing" }))
        .await
        .unwrap();
    assert_eq!(
        routing,
        crystalline_core::skill("crystalline-routing")
            .unwrap()
            .content
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_skill_name_names_the_ones_that_ship() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;

    let err = call_text(
        client.peer(),
        "skills",
        json!({ "name": "crystalline-nonesuch" }),
    )
    .await
    .unwrap_err();
    for name in SKILL_NAMES {
        assert!(err.contains(name), "{name} missing from: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resources_list_the_skills_and_read_back_their_playbooks() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let resources = peer.list_resources(Default::default()).await.unwrap();
    let uris: Vec<&str> = resources
        .resources
        .iter()
        .map(|r| r.uri.as_str())
        .collect::<Vec<_>>();
    let expected: Vec<String> = SKILL_NAMES
        .iter()
        .map(|n| format!("skill://{n}/SKILL.md"))
        .collect();
    assert_eq!(
        uris,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    for r in &resources.resources {
        assert_eq!(r.mime_type.as_deref(), Some("text/markdown"), "{}", r.uri);
        assert!(
            r.description.as_deref().is_some_and(|d| !d.is_empty()),
            "{} carries its skill's description",
            r.uri
        );
    }

    let read = peer
        .read_resource(ReadResourceRequestParams::new(
            "skill://crystalline-capture/SKILL.md",
        ))
        .await
        .unwrap();
    let contents = serde_json::to_value(&read.contents[0]).unwrap();
    assert_eq!(
        contents["text"].as_str().unwrap(),
        crystalline_core::skill("crystalline-capture")
            .unwrap()
            .content
    );
    assert_eq!(contents["mimeType"].as_str(), Some("text/markdown"));

    let err = peer
        .read_resource(ReadResourceRequestParams::new("skill://nonesuch/SKILL.md"))
        .await
        .unwrap_err()
        .to_string();
    for uri in &expected {
        assert!(err.contains(uri), "{uri} missing from: {err}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_two_prompts_carry_the_live_routing_block_and_the_static_snippet() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    let listed = peer.list_prompts(Default::default()).await.unwrap();
    let mut names: Vec<&str> = listed.prompts.iter().map(|p| p.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["connector", "onboarding"]);
    for p in &listed.prompts {
        assert!(
            p.description.as_deref().is_some_and(|d| !d.is_empty()),
            "{} needs a description a client can show",
            p.name
        );
    }

    let onboarding = peer
        .get_prompt(GetPromptRequestParams::new("onboarding"))
        .await
        .unwrap();
    let text = prompt_text(&onboarding);
    assert!(
        text.contains("CRYSTALLINE KNOWLEDGE ROUTING"),
        "the live routing block: {text}"
    );
    assert!(text.contains("eng"), "the registered domain: {text}");

    let connector = peer
        .get_prompt(GetPromptRequestParams::new("connector"))
        .await
        .unwrap();
    assert_eq!(prompt_text(&connector), crystalline_core::CONNECTOR_SNIPPET);
}

/// The text of a prompt result's single message.
fn prompt_text(result: &rmcp::model::GetPromptResult) -> String {
    let v = serde_json::to_value(result).unwrap();
    v.pointer("/messages/0/content/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_skills_serve_off_empties_the_lists_while_direct_reads_answer() {
    let h = Harness::new(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    assert!(tool_names(peer).await.contains(&"skills".to_string()));

    call_text(
        peer,
        "configure",
        json!({ "set": { "skills.serve": "false" } }),
    )
    .await
    .unwrap();

    assert!(
        !tool_names(peer).await.contains(&"skills".to_string()),
        "the tool is hidden while the gate is off"
    );
    assert!(
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .is_empty(),
        "no skill resources are advertised while the gate is off"
    );
    assert!(
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .is_empty(),
        "no prompts are advertised while the gate is off"
    );

    // Hidden, not disabled: a client holding a name or a uri from an earlier
    // listing still gets the bytes, exactly like every other gated tool.
    let routing = call_text(peer, "skills", json!({ "name": "crystalline-routing" }))
        .await
        .unwrap();
    assert_eq!(
        routing,
        crystalline_core::skill("crystalline-routing")
            .unwrap()
            .content
    );
    let read = peer
        .read_resource(ReadResourceRequestParams::new(
            "skill://crystalline-routing/SKILL.md",
        ))
        .await
        .expect("a direct resource read answers while the gate is off");
    assert!(!read.contents.is_empty());
    let connector = peer
        .get_prompt(GetPromptRequestParams::new("connector"))
        .await
        .expect("a direct prompt read answers while the gate is off");
    assert_eq!(prompt_text(&connector), crystalline_core::CONNECTOR_SNIPPET);

    // Unsetting the key restores the default, which is on.
    call_text(peer, "configure", json!({ "unset": ["skills.serve"] }))
        .await
        .unwrap();
    assert!(tool_names(peer).await.contains(&"skills".to_string()));
    assert_eq!(
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        SKILL_NAMES.len()
    );
    assert_eq!(
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .len(),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_serving_still_teaches_the_skills() {
    let h = Harness::new_read_only(&["eng"]).await;
    let (client, _server) = h.connect().await;
    let peer = client.peer();

    assert!(
        tool_names(peer).await.contains(&"skills".to_string()),
        "reading a skill is a read"
    );
    assert_eq!(
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        SKILL_NAMES.len()
    );
}

/// The default `auto` gate, decided per connection: a stdio client the install
/// receipt knows as an onboarded harness with hooks already has these five
/// skills as files, so its three lists come back empty while every direct read
/// still answers - the same hidden-not-disabled doctrine `false` follows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_receipt_matched_connection_is_served_no_skill_surface() {
    let h = Harness::new(&["eng"]).await;
    h.write_hooked_receipt("claude-code");

    let (client, _server) = h.connect_as("claude-code").await;
    let peer = client.peer();

    assert!(
        !tool_names(peer).await.contains(&"skills".to_string()),
        "the tool is hidden for a harness that carries the skills already"
    );
    assert!(
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .is_empty(),
        "no skill resources are advertised"
    );
    assert!(
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .is_empty(),
        "no prompts are advertised"
    );

    // Hidden, not disabled.
    let routing = call_text(peer, "skills", json!({ "name": "crystalline-routing" }))
        .await
        .unwrap();
    assert_eq!(
        routing,
        crystalline_core::skill("crystalline-routing")
            .unwrap()
            .content
    );
    let read = peer
        .read_resource(ReadResourceRequestParams::new(
            "skill://crystalline-routing/SKILL.md",
        ))
        .await
        .expect("a direct resource read answers for a matched connection");
    assert!(!read.contents.is_empty());
    let connector = peer
        .get_prompt(GetPromptRequestParams::new("connector"))
        .await
        .expect("a direct prompt read answers for a matched connection");
    assert_eq!(prompt_text(&connector), crystalline_core::CONNECTOR_SNIPPET);
}

/// The decision is per connection, not per server: an unrecognized client on
/// the same machine, with the same receipt, keeps the whole surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmatched_connection_keeps_the_whole_surface() {
    let h = Harness::new(&["eng"]).await;
    h.write_hooked_receipt("claude-code");

    let (client, _server) = h.connect_as("mcp-inspector").await;
    let peer = client.peer();

    assert!(tool_names(peer).await.contains(&"skills".to_string()));
    assert_eq!(
        peer.list_resources(Default::default())
            .await
            .unwrap()
            .resources
            .len(),
        SKILL_NAMES.len()
    );
    assert_eq!(
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .len(),
        2
    );
}

/// `skills.serve` forces the decision either way, mid-session: `true` restores
/// the surface for a matched connection, `false` takes it from everyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_setting_overrides_the_receipt_match_mid_session() {
    let h = Harness::new(&["eng"]).await;
    h.write_hooked_receipt("claude-code");

    let (client, _server) = h.connect_as("claude-code").await;
    let peer = client.peer();
    assert!(!tool_names(peer).await.contains(&"skills".to_string()));

    call_text(
        peer,
        "configure",
        json!({ "set": { "skills.serve": "true" } }),
    )
    .await
    .unwrap();
    assert!(
        tool_names(peer).await.contains(&"skills".to_string()),
        "true serves an installed harness too, on purpose"
    );
    assert_eq!(
        peer.list_prompts(Default::default())
            .await
            .unwrap()
            .prompts
            .len(),
        2
    );

    call_text(
        peer,
        "configure",
        json!({ "set": { "skills.serve": "false" } }),
    )
    .await
    .unwrap();
    assert!(!tool_names(peer).await.contains(&"skills".to_string()));

    // Back to the default: the receipt decides again, and it says hide.
    call_text(peer, "configure", json!({ "unset": ["skills.serve"] }))
        .await
        .unwrap();
    assert!(!tool_names(peer).await.contains(&"skills".to_string()));
}
