//! A draft's conflict over MCP, in a domain that reviews changes: the share
//! answers `conflicts_pending` with the kind, `origin_status` with `detail`
//! carries both texts an agent merges from, `resolve_conflict` settles the
//! draft, and an HTTP agent with no identity is refused as every draft verb
//! refuses it.
//!
//! The engine runs against `crate::support::MockProvider`, and every folder
//! is under a tempdir, so nothing here reaches a network or the machine's own
//! state. Over stdio the session acts as the machine owner, so the owner's own
//! draft is the one at stake there.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::support::MockProvider;
use crystalline_core::config::{GitHubConfig, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::mcp::McpServer;
use crystalline_service::params::EditParams;
use crystalline_service::{Engine, Scope};
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RunningService};
use rmcp::{RoleClient, RoleServer};
use serde_json::{Value, json};
use tokio::sync::Mutex;

fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

fn plan(first: &str) -> Vec<u8> {
    engram(
        "Plan",
        "plan",
        &format!("{first}\n\nthe middle of the plan\n\nthe last line of the plan"),
    )
}

const START: &str = "the first line of the plan";
const BOB_LINE: &str = "bob's first line";
const HER_LINE: &str = "her own first line";

/// A domain that reviews changes, subscribed from the mock forge, and the
/// tempdir that holds every folder of it.
struct Team {
    _tmp: tempfile::TempDir,
    mock: Arc<MockProvider>,
    eng: Arc<Engine>,
}

impl Team {
    async fn new() -> Team {
        let tmp = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockProvider::new());
        let files: BTreeMap<String, Vec<u8>> = [
            ("MANIFEST.md".to_string(), manifest()),
            ("plan.md".to_string(), plan(START)),
        ]
        .into_iter()
        .collect();
        let commit = mock.add_commit(files);
        mock.set_branch("main", &commit);
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
        let eng = Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(tmp.path().join("config.yaml")),
        )
        .with_origin_provider(mock.clone())
        .with_origins_dir(tmp.path().join("origins"))
        .with_state_dir(tmp.path().to_path_buf());
        eng.origin_add(
            "acme/team",
            Some("team"),
            None,
            None,
            Some(tmp.path().join("team-knowledge").to_str().unwrap()),
            None,
        )
        .await
        .unwrap();
        eng.set_review_mode(
            "team",
            Some(crystalline_core::config::ReviewMode::Overlay),
            crystalline_service::ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
        Team {
            _tmp: tmp,
            mock,
            eng: Arc::new(eng),
        }
    }

    /// The team commits `bytes` at plan.md, and this machine has not pulled.
    fn team_changes_the_plan(&self, bytes: Vec<u8>) {
        let head = self.mock.branch_commit("main").unwrap();
        let mut files: BTreeMap<String, Vec<u8>> = self
            .mock
            .commit_paths(&head)
            .into_iter()
            .map(|path| {
                let bytes = self.mock.commit_file(&head, &path).unwrap();
                (path, bytes)
            })
            .collect();
        files.insert("plan.md".to_string(), bytes);
        let commit = self.mock.add_commit(files);
        self.mock.set_branch("main", &commit);
    }

    /// `scope` replaces the plan's first line with `line`, as a draft.
    async fn replace_first_line(&self, scope: &Scope, line: &str) {
        let params = EditParams {
            identifier: "plan".to_string(),
            domain: "team".to_string(),
            operation: "find_replace".to_string(),
            content: Some(line.to_string()),
            key: None,
            value: None,
            values: None,
            find_text: Some(START.to_string()),
            expected_replacements: None,
            section: None,
            include_subsections: false,
            expected_checksum: None,
            ack_scope: None,
            share_link: None,
            model: None,
        };
        self.eng.edit_engram_as(&params, None, scope).await.unwrap();
    }
}

async fn serve(
    server: McpServer,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, McpServer>,
) {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task = tokio::spawn(async move { rmcp::serve_server(server, server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    let server = server_task.await.unwrap().unwrap();
    (client, server)
}

/// One tool call: whether it is an error result, and its text, parsed as JSON
/// where it is JSON.
async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> (bool, Value) {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = peer
        .call_tool(params)
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    let v = serde_json::to_value(&result).unwrap();
    let text = v
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = text.split_once("\n\n---\n").map_or(text, |(head, _)| head);
    let is_error = v["isError"].as_bool().unwrap_or(false);
    (
        is_error,
        serde_json::from_str(payload).unwrap_or(Value::String(payload.to_string())),
    )
}

#[tokio::test]
async fn share_changes_over_mcp_answers_conflicts_pending_with_the_kind() {
    let t = Team::new().await;
    t.replace_first_line(&Scope::Unrestricted, HER_LINE).await;
    t.team_changes_the_plan(plan(BOB_LINE));
    let (client, _server) = serve(McpServer::new(t.eng.clone())).await;

    let (is_error, answer) = call(&client, "share_changes", json!({ "domain": "team" })).await;
    assert!(!is_error, "{answer}");
    assert_eq!(answer["outcome"], "conflicts_pending", "{answer}");
    assert_eq!(answer["conflicts"][0]["path"], "plan.md", "{answer}");
    assert_eq!(answer["conflicts"][0]["kind"], "EditEdit", "{answer}");
    assert_eq!(answer["conflicts"][0]["draft"], true, "{answer}");
    // The forge keeps the team's text.
    let head = t.mock.branch_commit("main").unwrap();
    assert_eq!(
        t.mock.commit_file(&head, "plan.md").unwrap(),
        plan(BOB_LINE)
    );
}

#[tokio::test]
async fn an_agent_settles_with_resolve_conflict_merged_from_the_status_sides() {
    let t = Team::new().await;
    t.replace_first_line(&Scope::Unrestricted, HER_LINE).await;
    t.team_changes_the_plan(plan(BOB_LINE));
    let (client, _server) = serve(McpServer::new(t.eng.clone())).await;

    let (_, update) = call(&client, "update_domain", json!({ "domain": "team" })).await;
    let entry = &update["domains"][0]["conflicts"][0];
    assert_eq!(entry["draft"], true, "{update}");
    assert_eq!(entry["kind"], "EditEdit", "{update}");

    let (_, status) = call(
        &client,
        "origin_status",
        json!({ "domain": "team", "detail": true }),
    )
    .await;
    let conflict = &status["domains"][0]["conflicts"][0];
    assert_eq!(conflict["draft"], true, "{status}");
    let local = conflict["local"]
        .as_str()
        .unwrap_or_else(|| panic!("{status}"));
    let upstream = conflict["upstream"]
        .as_str()
        .unwrap_or_else(|| panic!("{status}"));
    assert!(
        conflict["base"].as_str().unwrap().contains(START),
        "{status}"
    );
    assert!(local.contains(HER_LINE), "{status}");
    assert!(upstream.contains(BOB_LINE), "{status}");

    // Both first lines, one after the other: the agent's merge of the two.
    let merged = upstream.replace(BOB_LINE, &format!("{BOB_LINE}\n\n{HER_LINE}"));
    let (is_error, receipt) = call(
        &client,
        "resolve_conflict",
        json!({
            "domain": "team",
            "path": "plan.md",
            "resolution": "merged",
            "content": merged,
        }),
    )
    .await;
    assert!(!is_error, "{receipt}");
    assert_eq!(receipt["kept"], "merged", "{receipt}");
    assert_eq!(receipt["draft"], true, "{receipt}");
    assert_eq!(receipt["remaining"], 0, "{receipt}");

    let (_, status) = call(&client, "origin_status", json!({ "domain": "team" })).await;
    assert_eq!(status["domains"][0]["conflicts"], json!([]), "{status}");
    let (_, read) = call(
        &client,
        "read_engram",
        json!({ "identifier": "plan", "domain": "team" }),
    )
    .await;
    let read = read.to_string();
    assert!(read.contains(BOB_LINE) && read.contains(HER_LINE), "{read}");
}

#[tokio::test]
async fn an_http_agent_with_no_identity_is_refused_as_every_draft_verb_is() {
    let t = Team::new().await;
    let alice = Scope::User {
        account: "alice".to_string(),
        admin: false,
    };
    t.replace_first_line(&alice, HER_LINE).await;
    t.team_changes_the_plan(plan(BOB_LINE));
    t.eng.origin_update(Some("team"), &alice).await.unwrap();
    assert_eq!(
        t.eng.draft_conflicts("team", "alice").await.unwrap().len(),
        1
    );
    let (client, _server) = serve(McpServer::new_http(t.eng.clone())).await;

    // Status names nobody's draft to an agent with no identity.
    let (_, status) = call(
        &client,
        "origin_status",
        json!({ "domain": "team", "detail": true }),
    )
    .await;
    let domain = &status["domains"][0];
    assert_eq!(domain["conflicts"], json!([]), "{status}");
    assert_eq!(domain["drafts_merged"], json!([]), "{status}");
    assert!(!status.to_string().contains(HER_LINE), "{status}");

    for resolution in [
        json!({ "resolution": "merged", "content": "anything" }),
        json!({ "resolution": "theirs" }),
        json!({ "resolution": "mine" }),
    ] {
        let mut args = json!({ "domain": "team", "path": "plan.md" });
        for (key, value) in resolution.as_object().unwrap() {
            args[key] = value.clone();
        }
        let (is_error, said) = call(&client, "resolve_conflict", args).await;
        assert!(is_error, "{said}");
        assert!(
            said.to_string()
                .contains(crystalline_service::OVERLAY_NEEDS_IDENTITY),
            "{said}"
        );
    }
    let (is_error, said) = call(&client, "share_changes", json!({ "domain": "team" })).await;
    assert!(is_error, "{said}");
    assert!(
        said.to_string()
            .contains(crystalline_service::OVERLAY_NEEDS_IDENTITY),
        "{said}"
    );
    // Her draft and her conflict stand as they were.
    assert_eq!(
        t.eng.draft_conflicts("team", "alice").await.unwrap().len(),
        1
    );
}
