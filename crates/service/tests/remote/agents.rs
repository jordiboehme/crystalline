//! The gates of an agent of the loops (0.35.0, spec D2 and D3), over the
//! production router: an agent token works only during a run of its loop.
//!
//! - Outside a live lease of its loop an MCP session reaches the handshake,
//!   the tool list and the `skills` tool, and nothing else.
//! - Inside one the server refuses what a run's prompt forbids.
//! - The session is bound to one loop (the run-loop header, or the agent's
//!   only loop), and a write is noted only in the lease of the loop the call
//!   belongs to.
//! - With `auth.mcp` off an agent token opens nothing and says why.

use crystalline_service::params::{DiscussParams, WriteReflectionParams};
use crystalline_service::rest::{AGENTS_NEED_AUTH_MCP, AgentLoops};
use crystalline_service::{ReflectionCaller, Scope};
use serde_json::{Value, json};

use crate::fixture::{Options, RemoteServer, engram};
use crate::support::raw_post;

/// The revision whose requests carry their own `_meta` and need no session.
const ERA: &str = "2026-07-28";

const PAGE: &str = "# D\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night.\n";

/// Add the agent `name` running `loops` and answer its token.
async fn agent(server: &RemoteServer, name: &str, loops: AgentLoops) -> String {
    let (_, issued) = server
        .auth
        .add_agent(name, None, loops, "boss", "", 30 * 86_400)
        .await
        .unwrap();
    issued.token
}

/// Give `open` work for both loops: a mechanical evolve finding (V103) and a
/// Reflection thread waiting for the agent.
async fn work_for_both_loops(server: &RemoteServer) {
    std::fs::write(
        server.file("open", "summary-doc.md"),
        engram(
            "Summary doc",
            "summary-doc",
            "The distilled version of the migration write-up.\n\n- summarizes [[Full text doc]]\n- [context] written after the migration closed",
        ),
    )
    .unwrap();
    std::fs::write(
        server.file("open", "full-text-doc.md"),
        engram(
            "Full text doc",
            "full-text-doc",
            "The full migration write-up with the shard tables.\n\n- [context] the source the summary was cut from\n- [lesson] keep the tables out of the summary",
        ),
    )
    .unwrap();
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
            &WriteReflectionParams {
                domain: "open".into(),
                path: "d.md".into(),
                markdown: Some(PAGE.into()),
                ..Default::default()
            },
            &ReflectionCaller::agent(Scope::Unrestricted, "claude-code", false),
        )
        .await
        .unwrap();
    server
        .engine
        .discuss(
            &DiscussParams {
                domain: "open".into(),
                action: Some("open".into()),
                path: Some("d.md".into()),
                section: Some("aaaa1111".into()),
                text: Some("Still true?".into()),
                ..Default::default()
            },
            &ReflectionCaller::person(Scope::Unrestricted, "keeper"),
        )
        .await
        .unwrap();
}

/// Take the lease of `lp` on `open` with the agent token `token`, over the
/// ctl route as a runner does: the lease token.
async fn take(server: &RemoteServer, token: &str, lp: &str) -> String {
    let (code, reply) = server
        .ctl(
            Some(token),
            json!({ "v": 1, "cmd": "automation_lease_take", "loop": lp, "domain": "open", "runner": "cron" }),
        )
        .await;
    assert_eq!(code, 200, "{reply}");
    assert_eq!(reply["ok"], true, "{reply}");
    reply["data"]["token"].as_str().unwrap().to_string()
}

/// The paths the lease of `lp` on `open` has noted, sorted.
async fn noted(server: &RemoteServer, lp: &str) -> Vec<String> {
    let store = server.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("open").await.unwrap().unwrap();
    let mut written = store
        .automation_lease(id, lp)
        .await
        .unwrap()
        .expect("the lease is held")
        .written;
    written.sort();
    written
}

/// One modern-era MCP request with the agent token, and with the run-loop
/// header when `run_loop` names one: the raw HTTP answer.
async fn mcp(
    server: &RemoteServer,
    method: &str,
    mut params: Value,
    token: &str,
    run_loop: Option<&str>,
) -> String {
    let name = params
        .get("name")
        .or_else(|| params.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": ERA,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "agent-gates-test", "version": "0.0.0" },
    });
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let mut headers: Vec<(&str, &str)> =
        vec![("MCP-Protocol-Version", ERA), ("Mcp-Method", method)];
    if let Some(name) = name.as_deref() {
        headers.push(("Mcp-Name", name));
    }
    if let Some(lp) = run_loop {
        headers.push(("X-Crystalline-Run-Loop", lp));
    }
    raw_post(&server.addr, &body, &headers, Some(token)).await
}

/// The JSON-RPC payload of a raw answer.
fn payload(raw: &str) -> Value {
    let line = raw
        .lines()
        .find(|l| l.starts_with("data: "))
        .unwrap_or_else(|| panic!("no SSE data line in:\n{raw}"));
    serde_json::from_str(line.trim_start_matches("data: ")).unwrap()
}

/// A tool call: `Ok` with its first text block, or `Err` with the refusal
/// (a tool error or a JSON-RPC error).
async fn call(
    server: &RemoteServer,
    token: &str,
    tool: &str,
    args: Value,
    run_loop: Option<&str>,
) -> Result<String, String> {
    let raw = mcp(
        server,
        "tools/call",
        json!({ "name": tool, "arguments": args }),
        token,
        run_loop,
    )
    .await;
    let payload = payload(&raw);
    if let Some(message) = payload["error"]["message"].as_str() {
        return Err(message.to_string());
    }
    let text = payload["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if payload["result"]["isError"] == true {
        return Err(text);
    }
    Ok(text)
}

/// **Outside a run an agent reaches the handshake, the tool list and its
/// skills, and nothing else.** A tool call, an attachment and a prompt are
/// refused with a sentence that says why; a skill resource is read. Once its
/// runner takes the lease the same calls go through, and once the lease is
/// given back they are refused again: decided on every call.
#[tokio::test]
async fn outside_a_run_an_agent_reaches_the_handshake_the_tool_list_and_its_skills() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = agent(&server, "writer", AgentLoops::Evolve).await;

    // The legacy handshake opens a session.
    let opened = server
        .http
        .post(format!("http://{}/", server.addr))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .bearer_auth(&token)
        .body(crate::support::initialize_body_as("agent-gates-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), 200);

    let listed = payload(&mcp(&server, "tools/list", json!({}), &token, None).await);
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|t| !t.is_empty()),
        "{listed}"
    );
    let skills = call(&server, &token, "skills", json!({}), None).await;
    assert!(skills.is_ok(), "{skills:?}");
    let skill = payload(
        &mcp(
            &server,
            "resources/read",
            json!({ "uri": "skill://crystalline-intelligence/SKILL.md" }),
            &token,
            None,
        )
        .await,
    );
    assert!(skill["result"]["contents"].is_array(), "{skill}");

    let read = call(
        &server,
        &token,
        "read_engram",
        json!({ "domain": "open", "identifier": "open-note" }),
        None,
    )
    .await
    .unwrap_err();
    assert!(read.contains("An agent works only during a run"), "{read}");
    let asset = payload(
        &mcp(
            &server,
            "resources/read",
            json!({ "uri": "crystalline://open/assets/shot.png" }),
            &token,
            None,
        )
        .await,
    );
    assert!(
        asset["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("An agent reads only during a run")),
        "{asset}"
    );
    let prompt = payload(
        &mcp(
            &server,
            "prompts/get",
            json!({ "name": "crystalline-onboarding" }),
            &token,
            None,
        )
        .await,
    );
    assert!(
        prompt["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("An agent reads only during a run")),
        "{prompt}"
    );

    // During the run the read goes through.
    work_for_both_loops(&server).await;
    let lease = take(&server, &token, "evolve").await;
    let read = call(
        &server,
        &token,
        "read_engram",
        json!({ "domain": "open", "identifier": "open-note" }),
        None,
    )
    .await;
    assert!(read.is_ok_and(|text| text.contains("vent driver")));

    // The lease given back, the agent is outside again.
    let (_, released) = server
        .ctl(
            Some(&token),
            json!({ "v": 1, "cmd": "automation_lease_release", "loop": "evolve", "domain": "open", "token": lease }),
        )
        .await;
    assert_eq!(released["ok"], true, "{released}");
    let read = call(
        &server,
        &token,
        "read_engram",
        json!({ "domain": "open", "identifier": "open-note" }),
        None,
    )
    .await
    .unwrap_err();
    assert!(read.contains("An agent works only during a run"), "{read}");
}

/// **Inside a run the server refuses what a run's prompt forbids**: every
/// tool of the prompt's `disallowed_tools`, the `write_reflection` action
/// `delete_asset` and any `overwrite: true`, while the run's own work goes
/// through and is noted in its lease.
#[tokio::test]
async fn inside_a_run_the_server_refuses_what_a_runs_prompt_forbids() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = agent(&server, "writer", AgentLoops::Evolve).await;
    work_for_both_loops(&server).await;
    take(&server, &token, "evolve").await;

    let written = call(
        &server,
        &token,
        "write_engram",
        json!({ "domain": "open", "title": "Run Fix", "content": "- [fact] fixed" }),
        None,
    )
    .await;
    assert!(written.is_ok(), "{written:?}");
    assert_eq!(noted(&server, "evolve").await, ["run-fix.md"]);

    for tool in crystalline_core::automation::HEADLESS_DISALLOWED_TOOLS {
        let refused = call(&server, &token, tool, json!({ "domain": "open" }), None)
            .await
            .unwrap_err();
        assert!(
            refused.contains(&format!("A run may not call {tool}")),
            "{tool}: {refused}"
        );
    }
    let refused = call(
        &server,
        &token,
        "write_engram",
        json!({ "domain": "open", "title": "Run Fix", "content": "- [fact] again", "overwrite": true }),
        None,
    )
    .await
    .unwrap_err();
    assert!(refused.contains("overwrite: true"), "{refused}");
    let refused = call(
        &server,
        &token,
        "write_reflection",
        json!({ "domain": "open", "action": "delete_asset", "path": "assets/pic.png" }),
        None,
    )
    .await
    .unwrap_err();
    assert!(refused.contains("delete_asset"), "{refused}");
    // Nothing a refused call names was touched.
    assert!(server.file("open", "open-note.md").exists());
    assert_eq!(noted(&server, "evolve").await, ["run-fix.md"]);
}

/// **A session is bound to one loop, and a write is noted only in that
/// loop's lease.** An agent with both loops holds both leases on one domain:
/// without the header each write lands in the lease of the loop it belongs
/// to (an engram in evolve's, a Reflection file in reflect's), with the
/// header the session has that loop's rights only and notes into that lease
/// only. A header naming no loop, or a loop the agent does not run, is
/// refused.
#[tokio::test]
async fn a_write_is_noted_only_in_the_lease_of_its_loop() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let nightly = agent(&server, "nightly", AgentLoops::Both).await;
    let curator = agent(&server, "curator", AgentLoops::Reflect).await;
    work_for_both_loops(&server).await;
    take(&server, &nightly, "evolve").await;
    take(&server, &nightly, "reflect").await;

    // Unbound: each write in its own loop's lease.
    call(
        &server,
        &nightly,
        "write_engram",
        json!({ "domain": "open", "title": "Unbound Fix", "content": "- [fact] fixed" }),
        None,
    )
    .await
    .unwrap();
    call(
        &server,
        &nightly,
        "write_reflection",
        json!({ "domain": "open", "action": "write_asset", "path": "assets/flow.svg", "content": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>" }),
        None,
    )
    .await
    .unwrap();
    assert_eq!(noted(&server, "evolve").await, ["unbound-fix.md"]);
    assert_eq!(
        noted(&server, "reflect").await,
        ["reflections/assets/flow.svg"]
    );

    // Bound to reflect: the reflect rights only, so no engram write.
    let refused = call(
        &server,
        &nightly,
        "write_engram",
        json!({ "domain": "open", "title": "Reflect Fix", "content": "- [fact] no" }),
        Some("reflect"),
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
    // Bound to evolve: noted in evolve's lease only.
    call(
        &server,
        &nightly,
        "write_engram",
        json!({ "domain": "open", "title": "Evolve Fix", "content": "- [fact] fixed" }),
        Some("evolve"),
    )
    .await
    .unwrap();
    assert_eq!(
        noted(&server, "evolve").await,
        ["evolve-fix.md", "unbound-fix.md"]
    );
    assert_eq!(
        noted(&server, "reflect").await,
        ["reflections/assets/flow.svg"]
    );

    let refused = call(&server, &nightly, "skills", json!({}), Some("both"))
        .await
        .unwrap_err();
    assert!(refused.contains("'both' is neither"), "{refused}");
    let refused = call(&server, &curator, "skills", json!({}), Some("evolve"))
        .await
        .unwrap_err();
    assert!(
        refused.contains("does not run the evolve loop"),
        "{refused}"
    );
    // The reflect-only agent is bound to reflect without the header, and it
    // holds no lease of it: outside its run.
    let refused = call(
        &server,
        &curator,
        "read_engram",
        json!({ "domain": "open", "identifier": "open-note" }),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("An agent works only during a run"),
        "{refused}"
    );
}

/// **A run of one loop does not open the other.** A session bound to
/// reflect, by the header, is outside its run while the agent holds only the
/// evolve lease.
#[tokio::test]
async fn a_lease_of_one_loop_does_not_open_a_session_bound_to_the_other() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let nightly = agent(&server, "nightly", AgentLoops::Both).await;
    work_for_both_loops(&server).await;
    take(&server, &nightly, "evolve").await;
    let args = json!({ "domain": "open", "identifier": "open-note" });
    assert!(
        call(
            &server,
            &nightly,
            "read_engram",
            args.clone(),
            Some("evolve")
        )
        .await
        .is_ok()
    );
    assert!(
        call(&server, &nightly, "read_engram", args.clone(), None)
            .await
            .is_ok()
    );
    let refused = call(&server, &nightly, "read_engram", args, Some("reflect"))
        .await
        .unwrap_err();
    assert!(
        refused.contains("An agent works only during a run"),
        "{refused}"
    );
}

/// **An agent's take refuses what is not due**, over the ctl route as over
/// the engine: nothing due for its loop on the domain, and `force` does not
/// change that.
#[tokio::test]
async fn an_agent_takes_no_lease_where_nothing_is_due() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let token = agent(&server, "writer", AgentLoops::Evolve).await;
    for force in [false, true] {
        let (code, reply) = server
            .ctl(
                Some(&token),
                json!({ "v": 1, "cmd": "automation_lease_take", "loop": "evolve", "domain": "open", "runner": "cron", "force": force }),
            )
            .await;
        assert_eq!(code, 200, "{reply}");
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(
            reply["error"]
                .as_str()
                .is_some_and(|e| e.contains("Nothing is due for the evolve loop on 'open'")),
            "{reply}"
        );
    }
    // A loop the agent does not run is refused outright.
    let (_, reply) = server
        .ctl(
            Some(&token),
            json!({ "v": 1, "cmd": "automation_lease_take", "loop": "reflect", "domain": "open", "runner": "cron" }),
        )
        .await;
    assert_eq!(reply["ok"], false, "{reply}");
}

/// **With `auth.mcp` off an agent token opens nothing, and says why**, at
/// the MCP door and at the ctl route alike: a `403` with the teaching
/// sentence and no challenge. The token is never looked up, so its last use
/// stays empty.
#[tokio::test]
async fn with_auth_mcp_off_an_agent_token_is_told_agents_need_it() {
    let server = RemoteServer::start(Options::OPEN).await;
    let token = agent(&server, "nightly", AgentLoops::Both).await;
    let (code, reply) = server
        .ctl(
            Some(&token),
            json!({ "v": 1, "cmd": "automation_status", "loop": "evolve" }),
        )
        .await;
    assert_eq!(code, 403, "{reply}");
    assert_eq!(reply["error"], AGENTS_NEED_AUTH_MCP);

    let door = server
        .http
        .post(format!("http://{}/", server.addr))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .bearer_auth(&token)
        .body(crate::support::initialize_body_as("agent-gates-test"))
        .send()
        .await
        .unwrap();
    assert_eq!(door.status(), 403);
    assert!(door.headers().get("www-authenticate").is_none());
    let body: Value = door.json().await.unwrap();
    assert_eq!(body["error"], AGENTS_NEED_AUTH_MCP);

    let agent = server.auth.agent("nightly").await.unwrap().unwrap();
    assert_eq!(agent.tokens[0].last_used, None);
}
