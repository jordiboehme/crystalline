//! A local stdio MCP session with a connected server: the tool list is the
//! same, a mounted page open in the server's editor is live and shows this
//! agent, a draft link works, and an HTTP session on this daemon never
//! borrows the owner's sign-in.

use std::time::{Duration, Instant};

use crystalline_service::Scope;
use crystalline_service::join::IDLE_JOIN_LIMIT;
use crystalline_service::mcp::McpServer;
use rmcp::RoleClient;
use rmcp::RoleServer;
use rmcp::model::{CallToolRequestParams, GetPromptRequestParams};
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};

use crate::fixture::{Options, RemoteServer};
use crate::local::LocalMachine;

async fn connect(
    server: McpServer,
) -> (
    RunningService<RoleClient, ()>,
    RunningService<RoleServer, McpServer>,
) {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let task = tokio::spawn(async move { rmcp::serve_server(server, server_io).await });
    let client = rmcp::serve_client((), client_io).await.unwrap();
    (client, task.await.unwrap().unwrap())
}

async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<Value, String> {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = peer.call_tool(params).await.map_err(|e| e.to_string())?;
    let v = serde_json::to_value(&result).unwrap();
    let text = v
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if v["isError"] == true {
        return Err(text);
    }
    let payload = text
        .split_once("\n\n---\n")
        .map_or(text.as_str(), |(head, _)| head)
        .to_string();
    Ok(serde_json::from_str(&payload).unwrap_or(Value::String(payload)))
}

async fn tool_names(peer: &Peer<RoleClient>) -> Vec<String> {
    let mut names: Vec<String> = peer
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    names
}

fn this_client() -> String {
    let info = rmcp::model::ClientConfig::default().client_info;
    format!("{}/{}", info.name, info.version)
}

/// Spec A4 Testing and V1: adding a source does not change the tool list.
#[tokio::test]
async fn the_mcp_tool_list_does_not_change_when_a_source_is_added() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let before = LocalMachine::start(false).await;
    let (client, _s) = connect(McpServer::new(before.engine.clone())).await;
    let unmounted = tool_names(client.peer()).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    assert_eq!(tool_names(client.peer()).await, unmounted);
}

/// Spec A4 Testing: a remote read of a page open in the server's editor
/// returns the live text and shows the forwarded agent in the strip.
#[tokio::test]
async fn a_stdio_read_of_a_mounted_page_open_in_the_servers_editor_is_live_and_draws_this_agent() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let room = server
        .person_typing("open", "open-note", "Ada", "a person typed this")
        .await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let read = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "open-note", "domain": "open" }),
    )
    .await
    .unwrap();
    assert_eq!(read["live"], true, "{read}");
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("a person typed this")
    );
    assert_eq!(read["domain"], "open");
    let present = room.present().await;
    let expected = format!("keeper (agent: {})", this_client());
    assert!(
        present.contains(&expected),
        "{present:?} should name {expected}"
    );
}

/// Spec A4 Testing: a draft link works on a mounted domain and lapses 30
/// minutes after the last call.
#[tokio::test]
async fn a_draft_link_works_on_a_mounted_domain_and_lapses_after_the_last_call() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server
        .engine
        .write_engram_as(
            &serde_json::from_value(json!({ "domain": "team", "title": "Plan", "content": "- [fact] keeper drafted this" })).unwrap(),
            None,
            &Scope::User { account: "keeper".into(), admin: false },
        )
        .await
        .unwrap();
    let link = server
        .auth
        .mint_overlay_grant("team", "plan.md", "keeper", None)
        .await
        .unwrap()
        .token;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "out").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let read = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "plan", "domain": "team", "share_link": link }),
    )
    .await
    .unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("keeper drafted this"),
        "{read}"
    );
    call(
        client.peer(),
        "edit_engram",
        json!({ "identifier": "plan", "domain": "team", "operation": "append", "content": "- [fact] out was here" }),
    )
    .await
    .unwrap();
    let draft = server
        .engine
        .overlay_draft_at("team", "keeper", "plan.md")
        .await
        .unwrap()
        .unwrap();
    assert!(draft.content.contains("out was here"));
    server
        .engine
        .joins()
        .expire_idle(Instant::now() + IDLE_JOIN_LIMIT + Duration::from_secs(1));
    let lapsed = call(
        client.peer(),
        "edit_engram",
        json!({ "identifier": "plan", "domain": "team", "operation": "append", "content": "- [fact] too late" }),
    )
    .await;
    let lapsed = lapsed.unwrap_err();
    assert!(
        lapsed.starts_with(
            "'plan' is keeper's draft, shared with you to read: writing into it is a second step"
        ),
        "the lapsed join is refused as unjoined: {lapsed}"
    );
    let draft = server
        .engine
        .overlay_draft_at("team", "keeper", "plan.md")
        .await
        .unwrap()
        .unwrap();
    assert!(!draft.content.contains("too late"), "{}", draft.content);
}

/// Decision D8: an HTTP session on this daemon is somebody who can reach the
/// port, not the owner. It never reaches a mounted domain.
#[tokio::test]
async fn an_http_session_on_this_daemon_never_borrows_the_owners_sign_in() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let room = server
        .person_typing("open", "open-note", "Ada", "typed")
        .await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new_http(machine.engine.clone())).await;
    let refused = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "open-note", "domain": "open" }),
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
    let swept = call(client.peer(), "search_engrams", json!({ "query": "vent" }))
        .await
        .unwrap();
    assert!(
        swept.get("parts").is_none() && !swept.to_string().contains("\"open\""),
        "{swept}"
    );
    assert_eq!(
        room.present().await,
        vec!["Ada".to_string()],
        "nobody reached the server"
    );
}

/// Decision D21: the collaboration tools and the machine-local tools refuse
/// a mounted domain in one sentence each.
#[tokio::test]
async fn collaboration_and_machine_local_tools_refuse_a_mounted_domain() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let share = call(client.peer(), "share_changes", json!({ "domain": "open" }))
        .await
        .unwrap_err();
    assert_eq!(
        share,
        "'open' comes from acme, which shares this domain; share changes there"
    );
    let remove = call(client.peer(), "remove_domain", json!({ "domain": "open" }))
        .await
        .unwrap_err();
    assert_eq!(
        remove,
        "'open' comes from acme; remove_domain acts on this machine's own domains. Disconnect the source with crystalline disconnect acme"
    );
}

/// The write receipt keeps the server's page address, never this machine's.
#[tokio::test]
async fn a_mounted_write_keeps_the_servers_page_address() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let receipt = call(
        client.peer(),
        "write_engram",
        json!({ "domain": "open", "title": "Via MCP", "content": "- [fact] x" }),
    )
    .await
    .unwrap();
    let web_url = receipt["web_url"].as_str().unwrap_or_default();
    assert!(web_url.starts_with(&server.origin()), "{receipt}");
    assert!(server.file("open", "via-mcp.md").exists());
}

/// Spec A8 through MCP: a one-domain call for a source that cannot be
/// reached answers at once, in this machine's words, and never waits past
/// the call's limit.
#[tokio::test]
async fn a_stdio_read_of_a_domain_whose_server_is_down_answers_the_a8_sentence() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    server.stop().await;
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let started = Instant::now();
    let refused = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "open-note", "domain": "open" }),
    )
    .await
    .unwrap_err();
    assert!(
        started.elapsed() < crystalline_remote::ONE_DOMAIN_LIMIT,
        "{:?}",
        started.elapsed()
    );
    assert!(
        refused.starts_with(
            "acme cannot be reached right now (check the VPN or the network); its domains are unavailable"
        ),
        "{refused}"
    );
}

/// The instructions a stdio peer reads out of its `server/discover` answer.
async fn discover_instructions(server: McpServer) -> String {
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let task = tokio::spawn(async move { rmcp::serve_server(server, server_io).await });
    let newest = crystalline_service::mcp::SERVED_PROTOCOL_VERSIONS
        .last()
        .unwrap()
        .clone();
    let client = rmcp::model::ClientConfig::default()
        .serve_with_lifecycle(
            client_io,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![newest],
            },
        )
        .await
        .expect("the server answered server/discover");
    let _server = task.await.unwrap().unwrap();
    client
        .peer()
        .peer_info()
        .as_ref()
        .and_then(|i| i.instructions.clone())
        .unwrap_or_default()
}

/// The text of the `onboarding` prompt.
async fn onboarding_text(server: McpServer) -> String {
    let (client, _s) = connect(server).await;
    let prompt = client
        .peer()
        .get_prompt(GetPromptRequestParams::new("onboarding"))
        .await
        .unwrap();
    serde_json::to_value(&prompt.messages).unwrap()[0]["content"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Decision D25 on every onboarding channel of a stdio session: the
/// `initialize` instructions, `server/discover` and the `onboarding` prompt
/// all list the mounted domains after the local ones, and none of them
/// names the hidden local copy.
#[tokio::test]
async fn every_stdio_onboarding_channel_lists_the_mounted_domains() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(true).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let initialized = client
        .peer()
        .peer_info()
        .as_ref()
        .and_then(|i| i.instructions.clone())
        .unwrap_or_default();
    let discovered = discover_instructions(McpServer::new(machine.engine.clone())).await;
    let prompted = onboarding_text(McpServer::new(machine.engine.clone())).await;
    assert_eq!(
        initialized, discovered,
        "both arrival paths hand out the same bytes"
    );
    // The re-fetch the block points at lists the mounts too, and never the
    // hidden copy.
    let listed = call(
        client.peer(),
        "list_domains",
        json!({ "include_routing": true }),
    )
    .await
    .unwrap();
    let rows = listed["domains"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["name"] == "open" && r["source"] == "acme"),
        "{listed}"
    );
    assert!(
        !rows
            .iter()
            .any(|r| r["name"] == "platform" && r.get("source").is_none()),
        "the hidden copy: {listed}"
    );
    for (channel, text) in [
        ("initialize", &initialized),
        ("server/discover", &discovered),
        ("onboarding", &prompted),
    ] {
        let local = text
            .find("Route here for local notes questions")
            .unwrap_or_else(|| panic!("{channel} names the local domain: {text}"));
        let mounted = text
            .find("Route here for shared questions")
            .unwrap_or_else(|| panic!("{channel} names acme's open: {text}"));
        assert!(local < mounted, "{channel}: mounts after the local rows");
        assert!(
            !text.contains("Route here for local platform questions"),
            "{channel} names the hidden copy: {text}"
        );
    }
}

/// Decision D8 on the onboarding channels: an HTTP session's discover and
/// onboarding prompt never name a mounted domain.
#[tokio::test]
async fn an_http_sessions_onboarding_never_lists_the_mounted_domains() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let discovered = discover_instructions(McpServer::new_http(machine.engine.clone())).await;
    let prompted = onboarding_text(McpServer::new_http(machine.engine.clone())).await;
    for (channel, text) in [("server/discover", &discovered), ("onboarding", &prompted)] {
        assert!(
            text.contains("Route here for local notes questions"),
            "{channel} still onboards the local domain: {text}"
        );
        assert!(
            !text.contains("Route here for shared questions"),
            "{channel} names a mounted domain: {text}"
        );
    }
}

/// The local half of a sweep resolves a domain's old name as a machine
/// without sources does: a rename leaves the old name answering.
#[tokio::test]
async fn a_sweep_naming_a_renamed_local_domain_by_its_old_name_still_finds_it() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine
        .engine
        .rename_domain("notes", "journal", false, &Scope::Unrestricted)
        .await
        .unwrap();
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let found = call(
        client.peer(),
        "search_engrams",
        json!({ "query": "vent", "domains": ["notes", "open"] }),
    )
    .await
    .unwrap();
    let text = found.to_string();
    assert!(text.contains("the local notes vent is green"), "{found}");
    assert!(text.contains("the vent driver retries"), "{found}");
    for tool in ["recent_activity", "evolve_engrams"] {
        let swept = call(client.peer(), tool, json!({ "domains": ["notes", "open"] })).await;
        let swept = swept.unwrap_or_else(|e| panic!("{tool}: {e}"));
        assert!(
            swept
                .get("missing")
                .is_none_or(|m| m.as_array().is_some_and(|a| a.is_empty())),
            "{tool}: {swept}"
        );
    }
}

/// Decision D21 for the rest of the handler lines: `provision` with a
/// domain and `discard_changes` refuse a mounted domain, and `add_domain`
/// under a mounted name acts on this machine and says what it was named.
#[tokio::test]
async fn provision_discard_and_add_domain_meet_a_mounted_domain() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let provision = call(
        client.peer(),
        "provision",
        json!({ "action": "allow", "domain": "open" }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        provision,
        "'open' comes from acme; provision acts on this machine's own domains. Disconnect the source with crystalline disconnect acme"
    );
    let discard = call(
        client.peer(),
        "discard_changes",
        json!({ "domain": "open", "paths": ["open-note.md"] }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        discard,
        "'open' comes from acme, which shares this domain; share changes there"
    );
    let folder = machine.tmp.path().join("my-open");
    let added = call(
        client.peer(),
        "add_domain",
        json!({ "domain": "open", "folder": folder.to_string_lossy() }),
    )
    .await
    .unwrap();
    assert_eq!(added["domain"], "open-local", "{added}");
    assert_eq!(
        added["note"],
        "'open' is a domain from acme on this machine, so this local domain is registered as 'open-local'"
    );
    assert!(folder.join("MANIFEST.md").exists(), "acted on this machine");
}

/// A write on a read-only instance is refused in one shape whichever domain
/// it names: the protocol error a local write gets, with the same words.
#[tokio::test]
async fn a_read_only_refusal_has_the_same_shape_for_a_mounted_domain() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start_read_only(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let mut shapes = Vec::new();
    for domain in ["notes", "open"] {
        let mut params = CallToolRequestParams::new("write_engram");
        if let Value::Object(map) =
            json!({ "domain": domain, "title": "Not Here", "content": "- [fact] no" })
        {
            params = params.with_arguments(map);
        }
        let answer = client.peer().call_tool(params).await;
        shapes.push(
            answer
                .map(|r| serde_json::to_value(r).unwrap())
                .map_err(|e| e.to_string()),
        );
    }
    assert_eq!(shapes[0], shapes[1]);
    let refusal = shapes[1].clone().unwrap_err();
    assert!(
        refusal.contains("this instance is read-only; content mutations are disabled"),
        "{refusal}"
    );
    assert!(!server.file("open", "not-here.md").exists());
}
