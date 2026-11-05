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
use rmcp::model::{CallToolRequestParams, GetPromptRequestParams, ReadResourceRequestParams};
use rmcp::service::{Peer, RunningService};
use serde_json::{Value, json};

use crate::fixture::{HOSTED, INVALID_FILE, Options, PIC, Play, RemoteServer, SHOT};
use crate::local::LocalMachine;
use crate::support::mcp_session::ServedSession;

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

/// T4-b: configure on a mounted domain is refused over stdio in the router's
/// words, for the view and for a change alike, and nothing is written on
/// this machine. A local domain still answers its view.
#[tokio::test]
async fn a_stdio_configure_on_a_mounted_domain_says_to_change_it_on_its_server() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    for args in [
        json!({ "domain": "open" }),
        json!({ "domain": "open", "set": { "sharing": "direct" } }),
        json!({ "domain": "open", "unset": ["rules.E007"] }),
    ] {
        let refused = call(client.peer(), "configure", args.clone())
            .await
            .unwrap_err();
        assert!(
            refused.contains(
                "'open' comes from acme; configure changes this machine's own domains, so change 'open' on acme itself"
            ),
            "{args}: {refused}"
        );
    }
    assert!(
        !machine.tmp.path().join("open").exists(),
        "nothing local was created"
    );
    let local = call(client.peer(), "configure", json!({ "domain": "notes" }))
        .await
        .unwrap();
    assert_eq!(local["domain"], "notes", "{local}");
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
    // The discover chooses no lifecycle (rmcp 3.5.1), so the server half is
    // never waited on here: it is kept alive until the text is read.
    let _server = ServedSession::spawn(server, server_io);
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
    // The two mounted rows this test pins; lab and team would only crowd
    // the block toward its count line.
    machine.mount_only(&["open", "platform"]);
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
        crate::local::assert_domain_lines_shown(text);
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

/// 0.26.0 over a stdio session: the Reflection tools reach a mounted
/// domain, answer in the shape the local handlers answer, and carry the
/// server's own refusal for an account below the right a verb needs.
#[tokio::test]
async fn the_reflection_tools_reach_a_mounted_domain_over_stdio_with_the_accounts_right() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let manifest = server.file("open", "MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replace("status: stable\n", "status: stable\nreflection: on\n"),
    )
    .unwrap();
    server.engine.sync(None).await.unwrap();
    server
        .engine
        .write_reflection(
            &crystalline_service::params::WriteReflectionParams {
                domain: "open".into(),
                path: "guides/a.md".into(),
                markdown: Some("# A\n\n## One\n\nText for people.\n".into()),
                ..Default::default()
            },
            &crystalline_service::ReflectionCaller::owner(),
        )
        .await
        .unwrap();

    // keeper is an instance editor: curate on the shared `open`.
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let doc = call(
        client.peer(),
        "read_reflection",
        json!({ "domain": "open", "path": "guides/a.md" }),
    )
    .await
    .unwrap();
    assert!(
        doc["markdown"]
            .as_str()
            .is_some_and(|m| m.contains("Text for people.")),
        "plain JSON, as locally: {doc}"
    );
    // A page stays plain JSON when lists answer TOON, as a local page does:
    // the routed answer renders as the local handler renders it.
    call(
        client.peer(),
        "configure",
        json!({ "set": { "service.response_format": "toon" } }),
    )
    .await
    .unwrap();
    let again = call(
        client.peer(),
        "read_reflection",
        json!({ "domain": "open", "path": "guides/a.md" }),
    )
    .await
    .unwrap();
    assert!(
        again["markdown"]
            .as_str()
            .is_some_and(|m| m.contains("Text for people.")),
        "plain JSON under TOON too: {again}"
    );
    let written = call(
        client.peer(),
        "write_reflection",
        json!({ "domain": "open", "path": "guides/a.md", "markdown": "# A\n\n## One\n\nText for people, edited.\n" }),
    )
    .await
    .unwrap();
    assert!(written.is_object(), "{written}");
    let stored = std::fs::read_to_string(server.file("open", "reflections/guides/a.md")).unwrap();
    assert!(stored.contains("edited"), "{stored}");
    assert!(
        stored.contains("keeper"),
        "changed_by names the account the token belongs to: {stored}"
    );

    // looker is an instance viewer: read and comment on `open`, never edit.
    let viewer = LocalMachine::start(false).await;
    viewer.connect(&server, "acme", "looker").await;
    viewer.mount();
    let (client, _s) = connect(McpServer::new(viewer.engine.clone())).await;
    let opened = call(
        client.peer(),
        "discuss",
        json!({ "domain": "open", "path": "guides/a.md", "section": "One", "text": "question: is this still true?" }),
    )
    .await
    .unwrap();
    assert!(
        opened["id"].as_str().is_some_and(|id| id.starts_with("t-")),
        "{opened}"
    );
    let refused = call(
        client.peer(),
        "write_reflection",
        json!({ "domain": "open", "path": "guides/a.md", "markdown": "# A\n" }),
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("your access to the Reflection of 'open' is read, and edit is needed"),
        "the server's refusal word for word: {refused}"
    );
}

/// A `reflect` that names a local domain and a mounted one fans out, and a
/// refusal of the local part reads as the local handler answers it: a
/// call-time refusal with the sentence, never a protocol error.
#[tokio::test]
async fn a_routed_reflect_refuses_its_local_part_at_call_time() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let mut params = CallToolRequestParams::new("reflect");
    if let Value::Object(map) = json!({ "domains": ["notes", "open"] }) {
        params = params.with_arguments(map);
    }
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("a refusal, not a protocol error");
    let result = serde_json::to_value(&result).unwrap();
    assert_eq!(result["isError"], true, "{result}");
    let text = result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(text.contains("Reflection is off for 'notes'"), "{text}");
}

// --- files of a mounted domain, both kinds (R85) ------------------------------

/// The resource links of a tool result and its JSON body, for a call that
/// must succeed.
async fn call_with_links(peer: &Peer<RoleClient>, tool: &str, args: Value) -> (Value, Vec<Value>) {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = serde_json::to_value(peer.call_tool(params).await.unwrap()).unwrap();
    assert_ne!(result["isError"], true, "{result}");
    let blocks = result["content"].as_array().unwrap();
    let body: Value = serde_json::from_str(blocks[0]["text"].as_str().unwrap()).unwrap();
    let links = blocks
        .iter()
        .filter(|b| b["type"] == "resource_link")
        .cloned()
        .collect();
    (body, links)
}

/// The bytes of one `resources/read` answer, base64 decoded.
async fn read_bytes(peer: &Peer<RoleClient>, uri: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let read = peer
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .map_err(|e| e.to_string())?;
    let contents = serde_json::to_value(&read.contents[0]).unwrap();
    assert_eq!(contents["uri"], uri, "{contents}");
    assert_eq!(contents["mimeType"], "image/png", "{contents}");
    Ok(base64::engine::general_purpose::STANDARD
        .decode(contents["blob"].as_str().unwrap())
        .unwrap())
}

/// Whether the server was asked for a file through the ctl `file` command.
fn asked_for_a_file(server: &RemoteServer) -> bool {
    server.ctl_requests().iter().any(|r| r["cmd"] == "file")
}

/// R85: a page of a mounted Reflection carries a resource link for each file
/// it shows, as a local page does, and the routed answer keeps no `files`.
#[tokio::test]
async fn a_mounted_read_reflection_carries_resource_links_for_its_assets() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let (page, links) = call_with_links(
        client.peer(),
        "read_reflection",
        json!({ "domain": "open", "path": "guides/a.md" }),
    )
    .await;
    assert!(page.get("files").is_none(), "{page}");
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        links[0]["uri"],
        "crystalline://open/reflections/assets/pic.png"
    );
    assert_eq!(links[0]["name"], "pic.png");
    assert_eq!(links[0]["mimeType"], "image/png");
    assert_eq!(links[0]["size"], PIC.len());
}

/// R85: a mounted engram carries a resource link for each attachment it
/// shows, as a local engram does.
#[tokio::test]
async fn a_mounted_read_engram_carries_resource_links_for_its_attachments() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let (engram, links) = call_with_links(
        client.peer(),
        "read_engram",
        json!({ "identifier": "shot-note", "domain": "open" }),
    )
    .await;
    assert!(engram.get("files").is_none(), "{engram}");
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0]["uri"], "crystalline://open/assets/shot.png");
    assert_eq!(links[0]["name"], "shot.png");
    assert_eq!(links[0]["mimeType"], "image/png");
    assert_eq!(links[0]["size"], SHOT.len());
}

/// R85: `resources/read` of a mounted Reflection's file is forwarded to the
/// server and answers its bytes.
#[tokio::test]
async fn resources_read_of_a_mounted_reflection_asset_returns_the_servers_bytes() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let bytes = read_bytes(
        client.peer(),
        "crystalline://open/reflections/assets/pic.png",
    )
    .await
    .unwrap();
    assert_eq!(bytes, PIC);
    assert!(asked_for_a_file(&server), "{:?}", server.ctl_requests());
}

/// R85: `resources/read` of a mounted engram attachment is forwarded to the
/// server and answers its bytes.
#[tokio::test]
async fn resources_read_of_a_mounted_engram_attachment_returns_the_servers_bytes() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let bytes = read_bytes(client.peer(), "crystalline://open/assets/shot.png")
        .await
        .unwrap();
    assert_eq!(bytes, SHOT);
    assert!(asked_for_a_file(&server), "{:?}", server.ctl_requests());
}

/// R85: the server decides with the account's own rights, by the kind of the
/// file. An account that loses its engram rights on a mounted domain (it
/// keeps reading the Reflection) is refused the engram attachment by the
/// server, and still gets the Reflection's file.
#[tokio::test]
async fn a_mounted_file_read_is_refused_where_the_account_may_not_read() {
    use crystalline_service::rest::{MemberLevel, ReflectionLevel};

    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    // A private `open` that looker reads as a member, engrams included.
    server
        .auth
        .set_domain_visibility("open", true, "boss")
        .await
        .unwrap();
    server
        .auth
        .upsert_domain_member("open", "looker", MemberLevel::Viewer, None, "boss")
        .await
        .unwrap();
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "looker").await;
    let sources = machine.mount();
    assert!(sources.table().mount("open").is_some(), "open is mounted");
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    // A member viewer reads both kinds.
    assert_eq!(
        read_bytes(client.peer(), "crystalline://open/assets/shot.png")
            .await
            .unwrap(),
        SHOT
    );
    server
        .auth
        .upsert_domain_member(
            "open",
            "looker",
            MemberLevel::None,
            Some(ReflectionLevel::Reader),
            "boss",
        )
        .await
        .unwrap();
    server.clear_ctl_log();
    let refused = read_bytes(client.peer(), "crystalline://open/assets/shot.png")
        .await
        .unwrap_err();
    assert!(
        asked_for_a_file(&server),
        "the server refused it, not this machine: {:?}",
        server.ctl_requests()
    );
    assert!(!refused.contains("shot"), "{refused}");
    assert!(refused.contains("open"), "{refused}");
    assert_eq!(
        read_bytes(
            client.peer(),
            "crystalline://open/reflections/assets/pic.png"
        )
        .await
        .unwrap(),
        PIC,
        "the Reflection right reads the Reflection's file"
    );
}

/// R85 against a 0.26 server: a read still succeeds, without links, and a
/// file read names the version the server needs.
#[tokio::test]
async fn a_026_server_reads_without_links_and_refuses_a_file_naming_the_version() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server.answer_as_026();
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let (engram, links) = call_with_links(
        client.peer(),
        "read_engram",
        json!({ "identifier": "shot-note", "domain": "open" }),
    )
    .await;
    assert!(
        engram["content"]
            .as_str()
            .is_some_and(|c| c.contains("assets/shot.png")),
        "{engram}"
    );
    assert!(links.is_empty(), "{links:?}");
    let refused = read_bytes(client.peer(), "crystalline://open/assets/shot.png")
        .await
        .unwrap_err();
    assert!(
        refused.contains("needs Crystalline 0.27 or newer"),
        "{refused}"
    );
    assert!(refused.contains("acme"), "{refused}");
}

/// R85 under a local name of its own: a mount renamed here links its files
/// under this machine's name, and reading a link asks the server under its
/// own name.
#[tokio::test]
async fn a_renamed_mount_links_and_reads_its_files_under_the_local_name() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount().rename_local("open", "acme-open").unwrap();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let (_, links) = call_with_links(
        client.peer(),
        "read_engram",
        json!({ "identifier": "shot-note", "domain": "acme-open" }),
    )
    .await;
    let engram_uri = "crystalline://acme-open/assets/shot.png";
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0]["uri"], engram_uri);
    let (_, links) = call_with_links(
        client.peer(),
        "read_reflection",
        json!({ "domain": "acme-open", "path": "guides/a.md" }),
    )
    .await;
    let page_uri = "crystalline://acme-open/reflections/assets/pic.png";
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0]["uri"], page_uri);
    assert_eq!(read_bytes(client.peer(), engram_uri).await.unwrap(), SHOT);
    assert_eq!(read_bytes(client.peer(), page_uri).await.unwrap(), PIC);
    assert!(
        server
            .ctl_requests()
            .iter()
            .filter(|r| r["cmd"] == "file")
            .all(|r| r["domain"] == "open"),
        "the server is asked under its own name: {:?}",
        server.ctl_requests()
    );
}

/// A hostile server does not decide how this machine serves a file: the
/// mime comes from the file name here, whatever the server names, and a
/// listed file whose path no domain may hold makes no link.
#[tokio::test]
async fn a_servers_mime_and_file_paths_are_not_trusted() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server.play(Play::BadFiles);
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    for (tool, args, uri) in [
        (
            "read_engram",
            json!({ "identifier": "shot-note", "domain": "open" }),
            "crystalline://open/assets/shot.png",
        ),
        (
            "read_reflection",
            json!({ "domain": "open", "path": "guides/a.md" }),
            "crystalline://open/reflections/assets/pic.png",
        ),
    ] {
        let (_, links) = call_with_links(client.peer(), tool, args).await;
        assert_eq!(links.len(), 1, "{tool}: {links:?}");
        assert_eq!(links[0]["uri"], uri, "{tool}");
        assert_eq!(links[0]["mimeType"], "image/png", "{tool}");
    }
    // read_bytes asserts the mime is image/png and the bytes came as a blob.
    assert_eq!(
        read_bytes(client.peer(), "crystalline://open/assets/shot.png")
            .await
            .unwrap(),
        SHOT
    );
}

/// The links of a routed read name the domain the call was routed for,
/// never a domain the server's answer names.
#[tokio::test]
async fn a_routed_reads_links_name_the_mounted_domain_whatever_the_server_says() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server.play(Play::ForeignDomain);
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let (engram, links) = call_with_links(
        client.peer(),
        "read_engram",
        json!({ "identifier": "shot-note", "domain": "open" }),
    )
    .await;
    assert_eq!(engram["domain"], "notes", "the tap played: {engram}");
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0]["uri"], "crystalline://open/assets/shot.png");
    let (_, links) = call_with_links(
        client.peer(),
        "read_reflection",
        json!({ "domain": "open", "path": "guides/a.md" }),
    )
    .await;
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        links[0]["uri"],
        "crystalline://open/reflections/assets/pic.png"
    );
}

/// A Reflection served by another instance on the server's shared database:
/// the server's refusal reaches the agent word for word as an input error,
/// after exactly one request, and this machine never follows it.
#[tokio::test]
async fn a_hosted_elsewhere_refusal_of_a_file_arrives_word_for_word() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server.play(Play::HostedElsewhere);
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    server.clear_ctl_log();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let refused = read_bytes(
        client.peer(),
        "crystalline://open/reflections/assets/pic.png",
    )
    .await
    .unwrap_err();
    assert_eq!(refused, format!("Mcp error: -32602: {HOSTED}"));
    let asked = server
        .ctl_requests()
        .iter()
        .filter(|r| r["cmd"] == "file")
        .count();
    assert_eq!(asked, 1, "{:?}", server.ctl_requests());
}

/// How a call ended, by class: `Ok`, a tool error (`isError`) or a protocol
/// error with its code.
#[derive(Debug, PartialEq)]
enum Ending {
    Answered,
    ToolError,
    Protocol(i32),
}

/// The ending of one call, by class, for comparing a local and a routed one.
async fn ending(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Ending {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    match peer.call_tool(params).await {
        Ok(result) if result.is_error == Some(true) => Ending::ToolError,
        Ok(_) => Ending::Answered,
        Err(rmcp::ServiceError::McpError(data)) => Ending::Protocol(data.code.0),
        Err(other) => panic!("not an MCP answer: {other}"),
    }
}

const INVALID_PARAMS: i32 = -32602;

/// R86: an argument error from another instance is the protocol error a local
/// one is. The class is compared, never the text.
#[tokio::test]
async fn a_mounted_argument_error_is_the_protocol_error_a_local_one_is() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    for domain in ["notes", "open"] {
        assert_eq!(
            ending(
                client.peer(),
                "read_engram",
                json!({ "identifier": "no-such-page", "domain": domain })
            )
            .await,
            Ending::Protocol(INVALID_PARAMS),
            "read_engram of a missing page in {domain}"
        );
        assert_eq!(
            ending(
                client.peer(),
                "read_reflection",
                json!({ "domain": domain, "path": "guides/a.md", "query": "text" })
            )
            .await,
            Ending::Protocol(INVALID_PARAMS),
            "read_reflection with path and query in {domain}"
        );
    }
}

/// R86 for the search: a filter the engine cannot parse is an argument error
/// on the server, and the protocol error here.
#[tokio::test]
async fn a_mounted_search_argument_error_is_a_protocol_error() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    for domain in ["notes", "open"] {
        assert_eq!(
            ending(
                client.peer(),
                "search_engrams",
                json!({ "query": "vent", "domains": [domain], "metadata_filters": "not an object" })
            )
            .await,
            Ending::Protocol(INVALID_PARAMS),
            "search in {domain}"
        );
    }
}

/// An argument error says nothing about the source: it is not a failure to
/// answer, so it never opens the down window, and the next call is asked.
#[tokio::test]
async fn an_argument_error_does_not_open_the_down_window() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    assert_eq!(
        ending(
            client.peer(),
            "read_engram",
            json!({ "identifier": "no-such-page", "domain": "open" })
        )
        .await,
        Ending::Protocol(INVALID_PARAMS)
    );
    let page = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "open-note", "domain": "open" }),
    )
    .await
    .unwrap();
    assert!(page.to_string().contains("open-note"), "{page}");
}

/// A routed file read the server finds invalid is an input error with the
/// server's words, and the source stays up: the next read is asked.
#[tokio::test]
async fn an_invalid_file_argument_is_an_input_error_and_keeps_the_source_up() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server.play(Play::InvalidFile);
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let refused = read_bytes(client.peer(), "crystalline://open/assets/shot.png")
        .await
        .unwrap_err();
    assert_eq!(refused, format!("Mcp error: -32602: {INVALID_FILE}"));
    server.play(Play::Honest);
    assert_eq!(
        read_bytes(client.peer(), "crystalline://open/assets/shot.png")
            .await
            .unwrap(),
        SHOT
    );
}

/// Every content block of one tool call, as JSON, and whether it was an error.
async fn call_blocks(peer: &Peer<RoleClient>, tool: &str, args: Value) -> (Vec<Value>, bool) {
    let mut params = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    let result = serde_json::to_value(peer.call_tool(params).await.unwrap()).unwrap();
    (
        result["content"].as_array().cloned().unwrap_or_default(),
        result["isError"] == true,
    )
}

fn used_tool(server: &RemoteServer, tool: &str) -> bool {
    server
        .ctl_requests()
        .iter()
        .any(|r| r["cmd"] == "tool" && r["tool"] == tool)
}

/// A server that predates agent files ignores `path` and `base64` on a write.
/// With a title beside them it would write an engram instead of a file, so the
/// call is refused here, before anything is sent, naming the server and the
/// release it needs. A download by path is refused the same way: such a server
/// would look for an engram of that name.
#[tokio::test]
async fn a_server_before_agent_files_is_refused_before_anything_is_sent() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    server.play(Play::BeforeAgentFiles);
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    server.clear_ctl_log();
    let upload = "runs an older Crystalline that cannot take files from agents yet; \
                  update it to 0.31.0 or later.";
    let download = "runs an older Crystalline that cannot hand files to agents yet; \
                    update it to 0.31.0 or later.";
    let png = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(PIC)
    };
    for (tool, args, expected) in [
        (
            "write_engram",
            json!({ "domain": "open", "path": "assets/a.png", "base64": png, "title": "Odd" }),
            upload,
        ),
        (
            "write_reflection",
            json!({ "domain": "open", "action": "write_asset", "path": "assets/a.png", "base64": png }),
            upload,
        ),
        (
            "read_engram",
            json!({ "domain": "open", "identifier": "assets/shot.png" }),
            download,
        ),
        (
            "read_reflection",
            json!({ "domain": "open", "path": "assets/pic.png" }),
            download,
        ),
    ] {
        let refused = call(client.peer(), tool, args).await.unwrap_err();
        assert!(
            refused.starts_with("The server at http"),
            "{tool}: {refused}"
        );
        assert!(refused.ends_with(expected), "{tool}: {refused}");
        assert!(
            !used_tool(&server, tool),
            "{tool} was sent: {:?}",
            server.ctl_requests()
        );
    }
    assert!(!server.file("open", "odd.md").exists());
    assert!(!server.file("open", "assets/a.png").exists());

    // An ordinary write and an ordinary read are not held back.
    call(
        client.peer(),
        "write_engram",
        json!({ "domain": "open", "title": "Plain Old", "content": "- [fact] fine" }),
    )
    .await
    .unwrap();
    assert!(server.file("open", "plain-old.md").exists());
}

/// A guarded write is sent only to a server that keeps the guard. One older
/// than 0.32.0, or one that names no version, would ignore
/// `expected_checksum` and write last-write-wins, so the call is refused here
/// before anything is sent. A new server gets it as sent and checks it
/// itself. A write without the field goes through as before, whatever the
/// server's version.
#[tokio::test]
async fn a_guarded_write_is_sent_only_to_a_server_that_keeps_the_guard() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let write = |content: &str, checksum: Option<&str>| {
        let mut args = json!({
            "domain": "open",
            "title": "Guarded",
            "content": content,
            "overwrite": true,
        });
        if let Some(checksum) = checksum {
            args["expected_checksum"] = json!(checksum);
        }
        args
    };
    call(client.peer(), "write_engram", write("- [fact] first", None))
        .await
        .unwrap();
    let path = server.file("open", "guarded.md");
    let token = crate::support::sha256_hex(&std::fs::read(&path).unwrap());

    for (play, says) in [
        (
            Play::Version("0.31.0"),
            "runs Crystalline 0.31.0, which cannot guard",
        ),
        (
            Play::NoVersion,
            "runs a Crystalline that does not name its version, which cannot guard",
        ),
    ] {
        server.play(play);
        server.clear_ctl_log();
        let refused = call(
            client.peer(),
            "write_engram",
            write("- [fact] guarded", Some(&token)),
        )
        .await
        .unwrap_err();
        assert!(refused.starts_with("The server at http"), "{refused}");
        assert!(refused.contains(says), "{refused}");
        assert!(
            refused.ends_with(
                "it needs 0.32 or newer. Leave expected_checksum out to write without the guard."
            ),
            "{refused}"
        );
        assert!(
            !used_tool(&server, "write_engram"),
            "the guarded write was sent: {:?}",
            server.ctl_requests()
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("first"));
    }

    // Without the field nothing changes, even on an old server.
    server.play(Play::Version("0.31.0"));
    call(
        client.peer(),
        "write_engram",
        write("- [fact] second", None),
    )
    .await
    .unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("second"));

    // A new server gets the field and keeps the guard: a stale token is
    // refused there, the current one lands.
    server.play(Play::Version("0.32.0"));
    let stale = call(
        client.peer(),
        "write_engram",
        write("- [fact] stale", Some(&token)),
    )
    .await
    .unwrap_err();
    assert!(stale.contains("re-read"), "{stale}");
    assert!(std::fs::read_to_string(&path).unwrap().contains("second"));
    let current = crate::support::sha256_hex(&std::fs::read(&path).unwrap());
    server.clear_ctl_log();
    call(
        client.peer(),
        "write_engram",
        write("- [fact] third", Some(&current)),
    )
    .await
    .unwrap();
    assert!(
        server.ctl_requests().iter().any(
            |r| r["tool"] == "write_engram" && r["args"]["expected_checksum"] == json!(current)
        ),
        "the field was forwarded as sent: {:?}",
        server.ctl_requests()
    );
    assert!(std::fs::read_to_string(&path).unwrap().contains("third"));
}

/// Through a mount, `include_images` decides a picture's shape as it does
/// locally: by default an image block and no base64, with `false` base64 and
/// no image block. A copy from an engram attachment into the Reflection with
/// that base64 lands byte for byte.
#[tokio::test]
async fn a_mounted_picture_comes_as_base64_with_include_images_false_and_copies() {
    use base64::Engine as _;
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let png = base64::engine::general_purpose::STANDARD.encode(SHOT);
    for (tool, args, bytes) in [
        (
            "read_engram",
            json!({ "domain": "open", "identifier": "assets/shot.png" }),
            SHOT,
        ),
        (
            "read_reflection",
            json!({ "domain": "open", "path": "assets/pic.png" }),
            PIC,
        ),
    ] {
        let (blocks, is_error) = call_blocks(client.peer(), tool, args.clone()).await;
        assert!(!is_error, "{tool}: {blocks:?}");
        let body: Value = serde_json::from_str(blocks[0]["text"].as_str().unwrap()).unwrap();
        assert!(body.get("base64").is_none(), "{tool}: {body}");
        assert_eq!(
            blocks.iter().filter(|b| b["type"] == "image").count(),
            1,
            "{tool}: {blocks:?}"
        );
        let mut raw = args;
        raw["include_images"] = json!(false);
        let (blocks, is_error) = call_blocks(client.peer(), tool, raw).await;
        assert!(!is_error, "{tool}: {blocks:?}");
        let body: Value = serde_json::from_str(blocks[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            body["base64"].as_str(),
            Some(
                base64::engine::general_purpose::STANDARD
                    .encode(bytes)
                    .as_str()
            ),
            "{tool}: {body}"
        );
        assert!(
            blocks.iter().all(|b| b["type"] != "image"),
            "{tool}: {blocks:?}"
        );
    }
    let (blocks, _) = call_blocks(
        client.peer(),
        "read_engram",
        json!({ "domain": "open", "identifier": "assets/shot.png", "include_images": false }),
    )
    .await;
    let body: Value = serde_json::from_str(blocks[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["base64"].as_str(), Some(png.as_str()));
    call(
        client.peer(),
        "write_reflection",
        json!({ "domain": "open", "action": "write_asset", "path": "assets/copy.png", "base64": body["base64"] }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(server.file("open", "reflections/assets/copy.png")).unwrap(),
        SHOT
    );
}

/// A routed file transfer has the long ctl limit, not the ten seconds of a
/// one-domain call: a full-size file over a slow uplink takes longer than
/// that, and the proxy in front of a server allows two minutes.
#[tokio::test]
async fn a_routed_upload_slower_than_a_one_domain_call_still_lands() {
    use base64::Engine as _;
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    server.play(Play::SlowTool(
        crystalline_remote::ONE_DOMAIN_LIMIT + Duration::from_secs(1),
    ));
    let png = base64::engine::general_purpose::STANDARD.encode(PIC);
    let up = call(
        client.peer(),
        "write_engram",
        json!({ "domain": "open", "path": "assets/slow.png", "base64": png }),
    )
    .await
    .unwrap();
    assert_eq!(up["path"], "assets/slow.png", "{up}");
    assert_eq!(
        std::fs::read(server.file("open", "assets/slow.png")).unwrap(),
        PIC
    );
}

/// The capability question before a file transfer has a short cut of its
/// own. When it runs out the transfer is refused and nothing is sent, but the
/// source is not marked down: the next call is asked.
#[tokio::test]
async fn a_status_timeout_refuses_the_upload_and_keeps_the_source_up() {
    use base64::Engine as _;
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    server.play(Play::SlowStatus(
        crystalline_remote::ONE_DOMAIN_LIMIT + Duration::from_secs(2),
    ));
    server.clear_ctl_log();
    let png = base64::engine::general_purpose::STANDARD.encode(PIC);
    let started = std::time::Instant::now();
    let refused = call(
        client.peer(),
        "write_engram",
        json!({ "domain": "open", "path": "assets/late.png", "base64": png }),
    )
    .await
    .unwrap_err();
    assert!(
        started.elapsed() < crystalline_remote::ONE_DOMAIN_LIMIT + Duration::from_secs(2),
        "the status question had its own short cut"
    );
    assert!(refused.contains("cannot be reached right now"), "{refused}");
    assert!(
        !used_tool(&server, "write_engram"),
        "{:?}",
        server.ctl_requests()
    );
    assert!(!server.file("open", "assets/late.png").exists());
    server.play(Play::Honest);
    let page = call(
        client.peer(),
        "read_engram",
        json!({ "identifier": "open-note", "domain": "open" }),
    )
    .await
    .unwrap();
    assert!(page.to_string().contains("open-note"), "{page}");
}

/// A retry through a mount of an upload the server already took succeeds on
/// the server as unchanged, for an engram file and a Reflection file. Other
/// bytes are still refused.
#[tokio::test]
async fn a_mounted_upload_sent_again_with_the_same_bytes_is_unchanged() {
    use base64::Engine as _;
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let png = base64::engine::general_purpose::STANDARD.encode(PIC);
    for (tool, args, on_server) in [
        (
            "write_engram",
            json!({ "domain": "open", "path": "assets/again.png", "base64": png }),
            "assets/again.png",
        ),
        (
            "write_reflection",
            json!({ "domain": "open", "action": "write_asset", "path": "assets/again.png", "base64": png }),
            "reflections/assets/again.png",
        ),
    ] {
        let first = call(client.peer(), tool, args.clone()).await.unwrap();
        assert!(first.get("unchanged").is_none(), "{tool}: {first}");
        let again = call(client.peer(), tool, args.clone()).await.unwrap();
        assert_eq!(again["unchanged"], true, "{tool}: {again}");
        assert_eq!(again["created"], false, "{tool}: {again}");
        assert_eq!(again["sha256"], first["sha256"], "{tool}: {again}");
        assert_eq!(
            std::fs::read(server.file("open", on_server)).unwrap(),
            PIC,
            "{tool}"
        );
        let mut other = args.clone();
        other["base64"] = json!(base64::engine::general_purpose::STANDARD.encode(b"other"));
        let refused = call(client.peer(), tool, other).await.unwrap_err();
        assert!(
            refused.contains(crystalline_service::engine::ASSET_EXISTS),
            "{tool}: {refused}"
        );
    }
}

/// A server that has agent files takes the upload through a mount, and the
/// routed answers for a file render as the local ones do: plain JSON (also
/// under a TOON configuration) and, for a picture, one image block with no
/// base64 beside it in the JSON.
#[tokio::test]
async fn a_mounted_upload_and_download_render_as_local_files_do() {
    use base64::Engine as _;
    let server = RemoteServer::start(Options::TOKENS).await;
    server.with_files_in_open().await;
    let machine = LocalMachine::start(false).await;
    machine.connect(&server, "acme", "keeper").await;
    machine.mount();
    let (client, _s) = connect(McpServer::new(machine.engine.clone())).await;
    let png = base64::engine::general_purpose::STANDARD.encode(PIC);
    let up = call(
        client.peer(),
        "write_engram",
        json!({ "domain": "open", "path": "assets/up.png", "base64": png }),
    )
    .await
    .unwrap();
    assert_eq!(up["path"], "assets/up.png", "{up}");
    assert_eq!(
        std::fs::read(server.file("open", "assets/up.png")).unwrap(),
        PIC
    );
    call(
        client.peer(),
        "configure",
        json!({ "set": { "service.response_format": "toon" } }),
    )
    .await
    .unwrap();
    for (tool, args) in [
        (
            "read_engram",
            json!({ "domain": "open", "identifier": "assets/up.png" }),
        ),
        (
            "read_reflection",
            json!({ "domain": "open", "path": "assets/pic.png" }),
        ),
    ] {
        let (blocks, is_error) = call_blocks(client.peer(), tool, args).await;
        assert!(!is_error, "{tool}: {blocks:?}");
        let body: Value = serde_json::from_str(blocks[0]["text"].as_str().unwrap())
            .unwrap_or_else(|_| panic!("{tool} answered JSON: {blocks:?}"));
        assert_eq!(body["mime"], "image/png", "{tool}: {body}");
        assert!(body.get("base64").is_none(), "{tool}: {body}");
        assert!(body["note"].is_string(), "{tool}: {body}");
        let images: Vec<&Value> = blocks.iter().filter(|b| b["type"] == "image").collect();
        assert_eq!(images.len(), 1, "{tool}: {blocks:?}");
        assert_eq!(images[0]["data"], png, "{tool}");
    }
}
