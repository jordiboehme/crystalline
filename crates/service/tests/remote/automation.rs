//! A headless run shares only what its runner wrote over MCP (R4, ruling
//! B): the MCP tool dispatch notes every engram path a write-shaped tool
//! wrote under the account that holds a lease, and nothing else does.

use crystalline_core::automation::Loop;
use crystalline_service::Scope;
use crystalline_service::params::WriteParams;
use serde_json::{Value, json};

use crate::fixture::{Options, RemoteServer, engram};

fn tool(name: &str, args: Value) -> Value {
    json!({ "v": 1, "cmd": "tool", "tool": name, "args": args })
}

fn keeper() -> Scope {
    Scope::User {
        account: "keeper".into(),
        admin: false,
    }
}

/// The paths the evolve lease on `open` has noted, sorted.
async fn noted(server: &RemoteServer) -> Vec<String> {
    let store = server.engine.store();
    let store = store.lock().await;
    let id = store.domain_id("open").await.unwrap().unwrap();
    let mut written = store
        .automation_lease(id, "evolve")
        .await
        .unwrap()
        .expect("the lease is held")
        .written;
    written.sort();
    written
}

#[tokio::test]
async fn only_the_lease_accounts_mcp_writes_are_noted() {
    let server = RemoteServer::start(Options::TOKENS).await;
    server
        .engine
        .automation_lease_take(Loop::Evolve, "open", "k8s", &keeper())
        .await
        .unwrap();
    let token = server.token_for("keeper").await;

    // An MCP write by the lease's account is noted.
    let (_, reply) = server
        .ctl(
            Some(&token),
            tool(
                "write_engram",
                json!({ "domain": "open", "title": "Run Fix", "content": "- [fact] fixed" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(noted(&server).await, vec!["run-fix.md"]);

    // A person's engram links the run's fix by its permalink; written on
    // disk and synced, so it is not noted.
    std::fs::write(
        server.file("open", "linker.md"),
        engram("Linker", "linker", "see [[run-fix]] for the fix"),
    )
    .unwrap();
    server.engine.sync(Some("open")).await.unwrap();
    assert_eq!(noted(&server).await, vec!["run-fix.md"]);

    // A move notes both ends, so the delete side reaches the pull request,
    // and every linker it rewrote.
    let (_, reply) = server
        .ctl(
            Some(&token),
            tool(
                "move_engram",
                json!({ "identifier": "run-fix", "domain": "open", "destination": "fixes/run-fix.md" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(
        reply.to_string().contains("linker"),
        "the move rewrote the linker: {reply}"
    );
    assert_eq!(
        noted(&server).await,
        vec!["fixes/run-fix.md", "linker.md", "run-fix.md"]
    );

    // An MCP write by another account is not the run's.
    let out = server.token_for("out").await;
    let (_, reply) = server
        .ctl(
            Some(&out),
            tool(
                "write_engram",
                json!({ "domain": "open", "title": "Other Note", "content": "- [fact] theirs" }),
            ),
        )
        .await;
    assert_eq!(reply["ok"], true, "{reply}");

    // A write by the same account through the engine (the way REST and
    // Fluid write) and a file edit picked up by a sync are not noted either.
    server
        .engine
        .write_engram_as(
            &WriteParams {
                path: None,
                base64: None,
                domain: "open".into(),
                title: "Typed In Fluid".into(),
                content: Some("- [fact] a person typed this".into()),
                folder: None,
                engram_type: None,
                tags: Vec::new(),
                status: None,
                metadata: None,
                overwrite: false,
                model: None,
                share_link: None,
            },
            None,
            &keeper(),
        )
        .await
        .unwrap();
    std::fs::write(
        server.file("open", "by-hand.md"),
        engram("By Hand", "by-hand", "edited in a text editor"),
    )
    .unwrap();
    server.engine.sync(Some("open")).await.unwrap();

    assert_eq!(
        noted(&server).await,
        vec!["fixes/run-fix.md", "linker.md", "run-fix.md"]
    );
}
