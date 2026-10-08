//! The remote `tool` command: the MCP verb core as the token's account, with
//! the agent the connected machine forwards, its own allow-list, live
//! documents and draft links.

use std::time::{Duration, Instant};

use crystalline_service::Scope;
use crystalline_service::join::IDLE_JOIN_LIMIT;
use serde_json::{Value, json};

use crate::fixture::{Options, RemoteServer};
use crate::support::McpTestSession;

fn tool(name: &str, args: Value) -> Value {
    json!({ "v": 1, "cmd": "tool", "tool": name, "args": args })
}

fn tool_as(name: &str, args: Value, client: &str) -> Value {
    json!({ "v": 1, "cmd": "tool", "tool": name, "args": args, "agent": { "client": client } })
}

#[tokio::test]
async fn a_remote_search_answers_what_the_account_may_see() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool("search_engrams", json!({ "query": "vent" })),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let text = reply["data"].to_string();
    assert!(
        text.contains("open-note") && text.contains("lab-note"),
        "{reply}"
    );

    let out = server.token_for("out").await;
    let (_, reply) = server
        .ctl(
            Some(&out),
            tool("search_engrams", json!({ "query": "vent" })),
        )
        .await;
    let text = reply["data"].to_string();
    assert!(text.contains("open-note"), "{reply}");
    assert!(
        !text.contains("lab-note") && !text.contains("\"lab\""),
        "{reply}"
    );
}

/// The write records the agent that asked and the account it asked for, the
/// way an MCP write by that harness does; with no agent named it is the CLI.
#[tokio::test]
async fn a_remote_write_lands_as_the_forwarded_agent_for_the_account() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool_as(
                "write_engram",
                json!({ "domain": "open", "title": "Remote Trace", "content": "- [fact] traced", "model": "claude-opus-5" }),
                "claude-code/2.1.290",
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let written = std::fs::read_to_string(server.file("open", "remote-trace.md")).unwrap();
    assert!(
        written.contains("by: claude-code/2.1.290-for-keeper"),
        "{written}"
    );
    assert!(
        written.contains("model: claude-opus-5"),
        "the model travels in the args: {written}"
    );

    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_engram",
                json!({ "domain": "open", "title": "CLI Trace", "content": "- [fact] typed" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let written = std::fs::read_to_string(server.file("open", "cli-trace.md")).unwrap();
    assert!(written.contains("crystalline-cli-for-keeper"), "{written}");
}

/// A client half cannot name somebody else: the join word is taken out of it,
/// as it is out of `clientInfo`.
#[tokio::test]
async fn a_forwarded_client_cannot_write_in_another_accounts_name() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool_as(
                "write_engram",
                json!({ "domain": "open", "title": "Forged", "content": "- [fact] x" }),
                "x for boss",
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let written = std::fs::read_to_string(server.file("open", "forged.md")).unwrap();
    assert!(written.contains("x-boss-for-keeper"), "{written}");
    assert!(!written.contains("x-for-boss"), "{written}");
}

/// A viewer writing remotely is refused before the engine runs, in the exact
/// sentence an MCP call by the same account gets.
#[tokio::test]
async fn a_viewer_writing_remotely_gets_the_mcp_refusal_word_for_word() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let looker = server.token_for("looker").await;
    let args = json!({ "domain": "open", "title": "Not Mine", "content": "- [fact] no" });
    let (_, reply) = server
        .ctl(Some(&looker), tool("write_engram", args.clone()))
        .await;
    assert_eq!(reply["ok"], false, "{reply}");
    let refusal = reply["error"].as_str().unwrap().to_string();
    assert_eq!(
        refusal,
        "your access to 'open' is viewer, and editor access is required to change it"
    );
    let session = McpTestSession::open(&server.addr, Some(&looker)).await;
    let over_mcp = session.call_tool("write_engram", args).await;
    assert!(
        over_mcp.contains(&refusal),
        "the same words over MCP:\n{over_mcp}"
    );
    assert!(!server.file("open", "not-mine.md").exists());
}

#[tokio::test]
async fn an_outsider_reading_a_private_domain_gets_the_mcp_not_found() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let out = server.token_for("out").await;
    let args = json!({ "identifier": "lab-note", "domain": "lab" });
    let (_, reply) = server
        .ctl(Some(&out), tool("read_engram", args.clone()))
        .await;
    assert_eq!(reply["ok"], false, "{reply}");
    let refusal = reply["error"].as_str().unwrap().to_string();
    let session = McpTestSession::open(&server.addr, Some(&out)).await;
    let over_mcp = session.call_tool("read_engram", args).await;
    assert!(
        over_mcp.contains(&refusal),
        "{refusal}\nversus:\n{over_mcp}"
    );
    assert!(
        !refusal.contains("leaks"),
        "nothing of the engram comes back: {refusal}"
    );
}

/// `configure` and `provision` change what the instance is; neither is
/// reachable through `tool`, not even for an admin. Nor are the domain and
/// collaboration tools.
#[tokio::test]
async fn configure_and_provision_are_refused_through_tool() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let boss = server.token_for("boss").await;
    for name in [
        "configure",
        "provision",
        "add_domain",
        "remove_domain",
        "share_changes",
        "skills",
    ] {
        let (_, reply) = server
            .ctl(
                Some(&boss),
                tool(
                    name,
                    json!({ "action": "set", "key": "search.retired_weight", "value": "0.1" }),
                ),
            )
            .await;
        assert_eq!(reply["ok"], false, "{name}: {reply}");
        assert_eq!(
            reply["error"],
            format!("the tool '{name}' is not available over a remote connection"),
            "{name}"
        );
    }
    let config = std::fs::read_to_string(server.tmp.path().join("config.yaml")).unwrap();
    assert!(
        !config.contains("retired_weight"),
        "nothing was configured: {config}"
    );
}

/// A page somebody has open in the server's web editor: the read answers
/// their unsaved text, says who is in there, and the agent joins the strip
/// under its own name for the account.
#[tokio::test]
async fn a_remote_read_of_a_page_open_in_the_editor_answers_the_live_text_and_joins_the_strip() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let room = server
        .person_typing("open", "open-note", "Ada", "a person typed this")
        .await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool_as(
                "read_engram",
                json!({ "identifier": "open-note", "domain": "open" }),
                "claude-code/2.1.290",
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let data = &reply["data"];
    assert_eq!(data["live"], true, "{data}");
    assert!(
        data["content"]
            .as_str()
            .unwrap()
            .contains("a person typed this"),
        "{data}"
    );
    assert!(data["present"].to_string().contains("Ada"), "{data}");
    let present = room.present().await;
    assert!(
        present
            .iter()
            .any(|p| p == "keeper (agent: claude-code/2.1.290)"),
        "the strip names the forwarded agent: {present:?}"
    );
}

#[tokio::test]
async fn a_remote_edit_lands_in_the_open_room_and_an_overwrite_is_refused() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let room = server
        .person_typing("open", "open-note", "Ada", "a person typed this")
        .await;
    let keeper = server.token_for("keeper").await;
    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool_as(
                "edit_engram",
                json!({ "identifier": "open-note", "domain": "open", "operation": "append", "content": "- [fact] and the agent added that" }),
                "claude-code/2.1.290",
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["data"]["landed"], "live", "{reply}");
    let live = room.text().await;
    assert!(
        live.contains("a person typed this") && live.contains("and the agent added that"),
        "{live}"
    );
    let on_disk = std::fs::read_to_string(server.file("open", "open-note.md")).unwrap();
    assert!(
        !on_disk.contains("and the agent added that"),
        "the room saves it, not the edit"
    );

    let (_, reply) = server
        .ctl(
            Some(&keeper),
            tool(
                "write_engram",
                json!({ "domain": "open", "title": "Open Note", "content": "- [fact] replaced", "overwrite": true }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], false, "{reply}");
    let refusal = reply["error"].as_str().unwrap();
    assert!(
        refusal.starts_with("The engram is open in the editor right now")
            && refusal.contains("Ada")
            && refusal.contains("a full replace was refused"),
        "the same refusal an MCP call gets: {refusal}"
    );
}

/// A draft share link on a domain that reviews changes: it opens keeper's
/// draft for `out`'s account, an edit without the link then lands in that
/// draft, and the join lapses 30 minutes after the last call (the rule a
/// sessionless HTTP connection already has).
#[tokio::test]
async fn a_share_link_opens_the_draft_for_the_account_and_lapses_after_thirty_idle_minutes() {
    let server = RemoteServer::start(Options::TOKENS).await;
    let keeper = Scope::User {
        account: "keeper".to_string(),
        admin: false,
    };
    server
        .engine
        .write_engram_as(
            &serde_json::from_value(json!({ "domain": "team", "title": "Plan", "content": "- [fact] keeper drafted this" }))
                .unwrap(),
            None,
            &keeper,
        )
        .await
        .unwrap();
    let link = server
        .auth
        .mint_overlay_grant("team", "plan.md", "keeper", None)
        .await
        .unwrap()
        .token;
    let out = server.token_for("out").await;
    let (_, reply) = server
        .ctl(
            Some(&out),
            tool_as(
                "read_engram",
                json!({ "identifier": "plan", "domain": "team", "share_link": link }),
                "claude-code/2.1.290",
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(
        reply["data"]["content"]
            .as_str()
            .unwrap()
            .contains("keeper drafted this"),
        "{reply}"
    );

    let edit = |line: &str| {
        tool_as(
            "edit_engram",
            json!({ "identifier": "plan", "domain": "team", "operation": "append", "content": line }),
            "claude-code/2.1.290",
        )
    };
    let (_, reply) = server
        .ctl(Some(&out), edit("- [fact] out added this inside"))
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    let draft = server
        .engine
        .overlay_draft_at("team", "keeper", "plan.md")
        .await
        .unwrap()
        .unwrap();
    assert!(
        draft.content.contains("out added this inside"),
        "the edit landed in keeper's draft"
    );

    let lapsed = server
        .engine
        .joins()
        .expire_idle(Instant::now() + IDLE_JOIN_LIMIT + Duration::from_secs(1));
    assert_eq!(
        lapsed, 1,
        "one join, held by out's account, ended by idleness"
    );
    let _ = server
        .ctl(Some(&out), edit("- [fact] out added this after the lapse"))
        .await;
    let draft = server
        .engine
        .overlay_draft_at("team", "keeper", "plan.md")
        .await
        .unwrap()
        .unwrap();
    assert!(
        !draft.content.contains("after the lapse"),
        "nothing lands in the draft once the join lapsed: {}",
        draft.content
    );
}
