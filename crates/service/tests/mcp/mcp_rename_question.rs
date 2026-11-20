//! **A pending rename question is named to agents once per session.**
//!
//! When a sync saw an explicit domain's MANIFEST `domain_name` change, this
//! instance holds a question for a person: finish the rename, or keep the
//! links as they are. No MCP tool answers it, so an agent is only told about
//! it: one guidance line on the first tool answer about that domain in a
//! session, never again in the same session, and again in the next one.

use std::sync::Arc;

use crystalline_core::config::{
    DomainEntry, GlobalConfig, NameOrigin, RenameQuestion, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use serde_json::{Value, json};
use tokio::sync::Mutex;

const LINE: &str = "This domain was renamed from eng to platform in its MANIFEST. Tell the person they can finish the rename in Fluid (the banner on the domain) or with `crystalline doctor --fix`; if nobody is there to ask, leave it.";

const NOTE: &str = "---\ntype: engram\ntitle: Note\npermalink: note\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Note\n\nA note.\n";

struct Harness {
    _tmp: tempfile::TempDir,
    engine: Arc<Engine>,
}

impl Harness {
    /// An explicit domain `eng` whose MANIFEST now declares `platform`, with
    /// the question the adoption recorded for it, and a plain domain `ops`.
    async fn new() -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut cfg = GlobalConfig::default();
        for (name, declared) in [("eng", "platform"), ("ops", "ops")] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("MANIFEST.md"),
                crystalline_core::manifest_template(declared, "2026-01-01"),
            )
            .unwrap();
            std::fs::write(dir.join("note.md"), NOTE).unwrap();
            std::fs::create_dir_all(dir.join("assets")).unwrap();
            std::fs::write(dir.join("assets/legacy.txt"), "A text attachment.").unwrap();
            let mut entry = DomainEntry::file(dir).with_name_origin(NameOrigin::Explicit);
            entry.canonical_seen = Some(declared.to_string());
            if name == "eng" {
                entry.rename_question = Some(RenameQuestion {
                    from: "eng".to_string(),
                    to: "platform".to_string(),
                    seen_at: "2026-01-01T00:00:00+00:00".to_string(),
                    answered: None,
                });
            }
            cfg.domains.insert(name.to_string(), entry);
        }
        // Tokens for the HTTP door, which the gated test signs in through.
        cfg.auth = Some(crystalline_core::config::AuthConfig {
            mcp: Some(true),
            oauth: Some(false),
            ..crystalline_core::config::AuthConfig::default()
        });
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
        Harness { _tmp: tmp, engine }
    }

    /// One stdio session: a real rmcp client over a duplex pair.
    async fn connect(
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
}

async fn read(client: &RunningService<RoleClient, ()>, domain: &str, identifier: &str) -> Value {
    let args = json!({ "identifier": identifier, "domain": domain });
    let params =
        CallToolRequestParams::new("read_engram").with_arguments(args.as_object().unwrap().clone());
    serde_json::to_value(client.call_tool(params).await.unwrap()).unwrap()
}

#[tokio::test]
async fn the_rename_guidance_appears_once_per_session_and_domain() {
    let harness = Harness::new().await;
    let (first, _server) = harness.connect().await;

    let answer = read(&first, "eng", "note").await;
    assert_eq!(answer["structuredContent"]["domain"], "eng");
    assert_eq!(answer["structuredContent"]["content"], NOTE);
    super::assert_engram_text_matches_structure(&answer);
    assert_eq!(
        answer["content"].as_array().unwrap()[1..],
        [json!({ "type": "text", "text": LINE })]
    );

    let again = read(&first, "eng", "note").await;
    assert_eq!(again["structuredContent"], answer["structuredContent"]);
    assert_eq!(
        again["content"].as_array().unwrap(),
        &answer["content"].as_array().unwrap()[..1]
    );

    let other = read(&first, "ops", "note").await;
    assert_eq!(other["structuredContent"]["domain"], "ops");
    super::assert_engram_text_matches_structure(&other);
    assert_eq!(other["content"].as_array().unwrap().len(), 1);

    let (second, _server) = harness.connect().await;
    let fresh = read(&second, "eng", "note").await;
    assert_eq!(fresh["content"], answer["content"]);
}

#[tokio::test]
async fn a_legacy_text_result_keeps_its_rename_trailer() {
    let harness = Harness::new().await;
    let (first, _server) = harness.connect().await;

    let answer = read(&first, "eng", "assets/legacy.txt").await;
    let text = answer["content"][0]["text"].as_str().unwrap();
    let (receipt, trailer) = text.split_once("\n\n---\n").unwrap();
    assert_eq!(trailer, LINE);
    let receipt: Value = serde_json::from_str(receipt).unwrap();
    assert_eq!(receipt["domain"], "eng");
    assert_eq!(receipt["text"], "A text attachment.");
    assert!(answer.get("structuredContent").is_none());

    let again = read(&first, "eng", "assets/legacy.txt").await;
    let again = again["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(again).unwrap(), receipt);

    let structured = read(&first, "eng", "note").await;
    assert_eq!(structured["structuredContent"]["domain"], "eng");
    super::assert_engram_text_matches_structure(&structured);
    assert_eq!(structured["content"].as_array().unwrap().len(), 1);
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

/// The line goes only to a caller who may rename the domain, the right
/// `POST /rename` needs: on a shared domain an instance admin is told, and
/// an editor who reads and writes it but may not rename it is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_rename_guidance_goes_only_to_callers_who_may_rename() {
    let harness = Harness::new().await;
    let auth_dir = harness._tmp.path().join("web-auth.db");
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&auth_dir)
            .await
            .unwrap(),
    );
    auth.add_user(
        "root",
        "Root",
        None,
        crystalline_service::rest::Role::Admin,
        "pw12345678",
    )
    .await
    .unwrap();
    auth.add_user(
        "eddy",
        "Eddy",
        None,
        crystalline_service::rest::Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    let router = crystalline_service::daemon::http_router(
        harness.engine.clone(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        &[],
        auth.clone(),
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

    let args = json!({ "identifier": "note", "domain": "eng" });
    let eddy = crate::support::McpTestSession::open(
        &addr,
        Some(&auth.issue_mcp_token("eddy", "t").await.unwrap().token),
    )
    .await;
    let read = payload(&eddy.call_tool("read_engram", args.clone()).await);
    let result = &read["result"];
    assert_eq!(result["structuredContent"]["domain"], "eng");
    super::assert_engram_text_matches_structure(result);
    assert_eq!(
        result["content"].as_array().unwrap().len(),
        1,
        "an editor is not told: {read}"
    );

    let root = crate::support::McpTestSession::open(
        &addr,
        Some(&auth.issue_mcp_token("root", "t").await.unwrap().token),
    )
    .await;
    let read = payload(&root.call_tool("read_engram", args).await);
    let result = &read["result"];
    assert_eq!(result["structuredContent"]["domain"], "eng");
    super::assert_engram_text_matches_structure(result);
    assert_eq!(
        result["content"].as_array().unwrap()[1..],
        [json!({ "type": "text", "text": LINE })],
        "an admin is told: {read}"
    );
}
