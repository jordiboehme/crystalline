//! What an unattended session may read: only domains every reader of the
//! domain it works on may read too. A reflect or evolve run that answers a
//! thread on a shared page cannot quote a private domain, because it never
//! sees one: no read tool answers it. An interactive session is unaffected.

use crystalline_core::automation::Loop;
use crystalline_service::{McpServer, Scope};
use serde_json::{Value, json};

use crate::disclosure::{Wire, private_harbor, request};

const LEGACY: &str = "2025-11-25";

/// A legacy session over `server`, opened.
async fn session(server: McpServer) -> Wire {
    let mut wire = Wire::over(server);
    let opened = wire
        .call(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY,
                "capabilities": {},
                "clientInfo": { "name": "read-limit-test", "version": "1.0.0" },
            }),
        ))
        .await;
    assert_eq!(opened["result"]["protocolVersion"], json!(LEGACY));
    wire.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    wire
}

/// Everything a session gets back from the read tools about harbor's
/// engram, as one text.
async fn reads(wire: &mut Wire) -> String {
    let calls = [
        ("search_engrams", json!({ "query": "five people" })),
        (
            "search_engrams",
            json!({ "query": "five people", "domains": ["harbor"] }),
        ),
        (
            "read_engram",
            json!({ "identifier": "crystalline://harbor/team/harbor-team" }),
        ),
        (
            "read_engram",
            json!({ "identifier": "team/harbor-team", "domain": "harbor" }),
        ),
        (
            "build_context",
            json!({ "anchor": "crystalline://harbor/team/harbor-team" }),
        ),
        ("browse_domain", json!({ "domain": "harbor" })),
        ("recent_activity", json!({ "timeframe": "3650d" })),
        ("list_domains", json!({})),
    ];
    let mut out = String::new();
    for (n, (name, arguments)) in calls.into_iter().enumerate() {
        let answer = wire
            .call(request(
                10 + n as u32,
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            ))
            .await;
        out.push_str(&answer.to_string());
        out.push('\n');
    }
    out
}

/// Whether harbor's engram showed up anywhere in `text`.
fn quotes_harbor(text: &str) -> bool {
    text.contains("five people") || text.contains("Harbor team")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interactive_session_reads_every_domain() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = session(McpServer::new(fx.engine.clone())).await;
    assert!(quotes_harbor(&reads(&mut wire).await));
}

/// The reflect loop on the shared eng answers its threads without ever
/// seeing the private harbor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reflect_run_cannot_read_a_domain_fewer_people_may_read() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(McpServer::new(fx.engine.clone())).await;
    let text = reads(&mut wire).await;
    assert!(!quotes_harbor(&text), "{text}");
    // Its own domain stays readable.
    let own: Value = wire
        .call(request(
            30,
            "tools/call",
            json!({ "name": "read_engram", "arguments": { "identifier": "runbooks/deploy", "domain": "eng" } }),
        ))
        .await;
    assert!(own.to_string().contains("deploys run at night"), "{own}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_evolve_run_cannot_read_a_domain_fewer_people_may_read() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .automation_lease_take(Loop::Evolve, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(McpServer::new(fx.engine.clone())).await;
    let text = reads(&mut wire).await;
    assert!(!quotes_harbor(&text), "{text}");
}

/// A session marked unattended reads as a run does, also without a lease:
/// it may write anywhere, so it reads only what every domain's readers may.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattended_session_without_a_lease_is_limited_too() {
    let (fx, _auth) = private_harbor().await;
    let mut wire = session(McpServer::new(fx.engine.clone()).with_unattended(true)).await;
    let text = reads(&mut wire).await;
    assert!(!quotes_harbor(&text), "{text}");
}

/// The run's own domain decides: a run on the private harbor reads harbor,
/// and eng, which all of harbor's readers may read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_on_the_narrow_domain_reads_it() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .automation_lease_take(Loop::Evolve, "harbor", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(McpServer::new(fx.engine.clone())).await;
    assert!(quotes_harbor(&reads(&mut wire).await));
}

/// Switch harbor's Reflection on, so a reflect run may lease it.
async fn harbor_reflects(fx: &crate::fixture::Fx) {
    let manifest = crate::fixture::root_of(fx, "harbor").join("MANIFEST.md");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        text.replacen("---\n", "---\nreflection: on\n", 1),
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
}

fn runner() -> Scope {
    Scope::User {
        account: "runner".into(),
        admin: false,
    }
}

/// One runner account holds reflect leases on eng and on the private harbor
/// at once. Each run's session is bound to the domain it was started for:
/// the eng session cannot read harbor, the harbor session reads what
/// harbor's readers may read. A marked session that names no domain gets
/// the intersection over its leases; a name it holds no lease on counts for
/// nothing.
#[tokio::test]
async fn each_run_reads_by_the_domain_its_session_was_started_for() {
    let (fx, auth) = private_harbor().await;
    auth.add_user(
        "runner",
        "runner",
        None,
        crystalline_service::rest::Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    auth.upsert_domain_member(
        "harbor",
        "runner",
        crystalline_service::rest::MemberLevel::Editor,
        None,
        "keeper",
    )
    .await
    .unwrap();
    harbor_reflects(&fx).await;
    for domain in ["eng", "harbor"] {
        fx.engine
            .automation_lease_take(Loop::Reflect, domain, "cli", &runner())
            .await
            .unwrap();
    }
    let eng = fx
        .engine
        .read_limit_for(&runner(), true, Some("eng"))
        .await
        .unwrap()
        .hidden;
    assert!(eng.contains("harbor"), "{eng:?}");
    let harbor = fx
        .engine
        .read_limit_for(&runner(), true, Some("harbor"))
        .await
        .unwrap()
        .hidden;
    assert!(harbor.is_empty(), "{harbor:?}");
    let both = fx
        .engine
        .read_limit_for(&runner(), true, None)
        .await
        .unwrap()
        .hidden;
    assert!(both.contains("harbor"), "{both:?}");
    // A domain it holds no lease on cannot widen what it reads.
    let named = fx
        .engine
        .read_limit_for(&user("sam"), true, Some("harbor"))
        .await
        .unwrap()
        .hidden;
    assert!(named.contains("harbor"), "{named:?}");
    // An account with no lease and no mark is attended.
    assert!(
        fx.engine
            .read_limit_for(&user("sam"), false, None)
            .await
            .is_none()
    );
}

fn user(name: &str) -> Scope {
    Scope::User {
        account: name.into(),
        admin: false,
    }
}

/// The same over MCP, for the machine owner's own two runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_owner_runs_each_read_by_their_own_domain() {
    let (fx, _auth) = private_harbor().await;
    harbor_reflects(&fx).await;
    for domain in ["eng", "harbor"] {
        fx.engine
            .automation_lease_take(Loop::Reflect, domain, "cli", &Scope::Unrestricted)
            .await
            .unwrap();
    }
    let mut on_eng = session(
        McpServer::new(fx.engine.clone())
            .with_unattended(true)
            .with_run_domain("eng"),
    )
    .await;
    assert!(!quotes_harbor(&reads(&mut on_eng).await));
    let mut on_harbor = session(
        McpServer::new(fx.engine.clone())
            .with_unattended(true)
            .with_run_domain("harbor"),
    )
    .await;
    assert!(quotes_harbor(&reads(&mut on_harbor).await));
}

/// A reflect run answering a thread: its answer lands, and in the same
/// session it cannot read the private domain it would have to quote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_answers_a_thread_without_seeing_a_private_domain() {
    let (fx, _auth) = private_harbor().await;
    let owner = crystalline_service::ReflectionCaller::owner();
    fx.engine
        .write_reflection(
            &crate::docs::write("overview.md", "# Overview\n\n## Team\n\nA team.\n"),
            &owner,
        )
        .await
        .unwrap();
    let opened = fx
        .engine
        .discuss(
            &crystalline_service::params::DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("overview.md".into()),
                section: Some("Team".into()),
                text: Some("question: what does harbor know about the team?".into()),
                ..Default::default()
            },
            &crystalline_service::ReflectionCaller::person(
                Scope::User {
                    account: "ann".into(),
                    admin: false,
                },
                "ann",
            ),
        )
        .await
        .unwrap();
    let thread = opened["thread"]["id"]
        .as_str()
        .or(opened["id"].as_str())
        .expect("the thread's id")
        .to_string();
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(
        McpServer::new(fx.engine.clone())
            .with_unattended(true)
            .with_run_domain("eng"),
    )
    .await;
    let read = wire
        .call(request(
            40,
            "tools/call",
            json!({ "name": "read_engram", "arguments": { "identifier": "crystalline://harbor/team/harbor-team" } }),
        ))
        .await;
    assert!(!quotes_harbor(&read.to_string()), "{read}");
    let answered = wire
        .call(request(
            41,
            "tools/call",
            json!({ "name": "discuss", "arguments": {
                "domain": "eng", "action": "answer", "path": "overview.md", "thread": thread,
                "text": "note: I cannot see anything about that here."
            } }),
        ))
        .await;
    assert!(
        answered["error"].is_null() && answered["result"]["isError"] != json!(true),
        "{answered}"
    );
}

/// A tool call's answer as text.
async fn tool(wire: &mut Wire, id: u32, name: &str, arguments: Value) -> Value {
    wire.call(request(
        id,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    ))
    .await
}

fn refused(answer: &Value) -> String {
    if let Some(message) = answer["error"]["message"].as_str() {
        return message.to_string();
    }
    assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn landed(answer: &Value) {
    assert!(
        answer["error"].is_null() && answer["result"]["isError"] != json!(true),
        "{answer}"
    );
}

/// A run on the private harbor reads harbor, so it may not write into the
/// shared eng: not its engrams, not its Reflection, not its threads. It
/// writes into harbor as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_writes_only_into_the_domain_it_works_on() {
    let (fx, _auth) = private_harbor().await;
    harbor_reflects(&fx).await;
    let owner = crystalline_service::ReflectionCaller::owner();
    fx.engine
        .write_reflection(
            &crate::docs::write("overview.md", "# Overview\n\n## Team\n\nA team.\n"),
            &owner,
        )
        .await
        .unwrap();
    fx.engine
        .automation_lease_take(Loop::Reflect, "harbor", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(
        McpServer::new(fx.engine.clone())
            .with_unattended(true)
            .with_run_domain("harbor"),
    )
    .await;
    let refusal = "this run works on harbor and cannot write to eng";
    let engram = tool(
        &mut wire,
        50,
        "write_engram",
        json!({
            "domain": "eng", "title": "Leak", "content": "five people",
        }),
    )
    .await;
    assert!(refused(&engram).contains(refusal), "{engram}");
    let edit = tool(
        &mut wire,
        51,
        "edit_engram",
        json!({
            "domain": "eng", "identifier": "runbooks/deploy",
            "operation": "append", "content": "five people",
        }),
    )
    .await;
    assert!(refused(&edit).contains(refusal), "{edit}");
    let page = tool(
        &mut wire,
        52,
        "write_reflection",
        json!({
            "domain": "eng", "path": "leak.md", "markdown": "# Leak\n\nfive people\n",
        }),
    )
    .await;
    assert!(refused(&page).contains(refusal), "{page}");
    let thread = tool(
        &mut wire,
        53,
        "discuss",
        json!({
            "domain": "eng", "action": "open", "path": "overview.md",
            "section": "Team", "text": "note: five people",
        }),
    )
    .await;
    assert!(refused(&thread).contains(refusal), "{thread}");
    assert!(!fx.root.join("leak.md").exists());
    assert!(!fx.root.join("reflections/leak.md").exists());
    // Its own domain takes the write.
    landed(
        &tool(
            &mut wire,
            54,
            "write_engram",
            json!({
                "domain": "harbor", "title": "Rota note", "content": "weekly",
            }),
        )
        .await,
    );
}

/// A limited session's onboarding names only the domains it may read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_limited_session_is_routed_only_to_what_it_may_read() {
    let (fx, _auth) = private_harbor().await;
    let mut open = Wire::over(McpServer::new(fx.engine.clone()));
    let first = open
        .call(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY, "capabilities": {},
                "clientInfo": { "name": "t", "version": "1" },
            }),
        ))
        .await;
    assert!(first.to_string().contains("harbor"), "{first}");
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut limited = Wire::over(McpServer::new(fx.engine.clone()));
    let opened = limited
        .call(request(
            1,
            "initialize",
            json!({
                "protocolVersion": LEGACY, "capabilities": {},
                "clientInfo": { "name": "t", "version": "1" },
            }),
        ))
        .await;
    assert!(!opened.to_string().contains("harbor"), "{opened}");
    limited
        .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    let prompt = limited
        .call(request(2, "prompts/get", json!({ "name": "onboarding" })))
        .await;
    assert!(!prompt.to_string().contains("harbor"), "{prompt}");
    let mut modern = Wire::over(McpServer::new(fx.engine.clone()));
    let discovered = modern
        .call(request(
            1,
            "server/discover",
            json!({
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                }
            }),
        ))
        .await;
    assert!(discovered["result"].is_object(), "{discovered}");
    assert!(!discovered.to_string().contains("harbor"), "{discovered}");
}

/// The Reflection and threads of a hidden domain are not there for a
/// limited session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_limited_session_cannot_read_a_hidden_reflection_or_its_threads() {
    let (fx, _auth) = private_harbor().await;
    harbor_reflects(&fx).await;
    let owner = crystalline_service::ReflectionCaller::owner();
    let mut page = crate::docs::write("secret.md", "# Secret\n\n## Team\n\nfive people\n");
    page.domain = "harbor".into();
    fx.engine.write_reflection(&page, &owner).await.unwrap();
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(
        McpServer::new(fx.engine.clone())
            .with_unattended(true)
            .with_run_domain("eng"),
    )
    .await;
    let read = tool(
        &mut wire,
        60,
        "read_reflection",
        json!({ "domain": "harbor", "path": "secret.md" }),
    )
    .await;
    assert!(!read.to_string().contains("five people"), "{read}");
    refused(&read);
    let list = tool(
        &mut wire,
        61,
        "discuss",
        json!({ "domain": "harbor", "action": "list" }),
    )
    .await;
    refused(&list);
}

/// An interactive owner session limited by its own run says why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interactive_session_limited_by_a_run_says_so() {
    let (fx, _auth) = private_harbor().await;
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cli", &Scope::Unrestricted)
        .await
        .unwrap();
    let mut wire = session(McpServer::new(fx.engine.clone())).await;
    let answer = tool(&mut wire, 70, "list_domains", json!({})).await;
    assert!(
        answer.to_string().contains(
            "a reflect run on eng is going; until it ends this session reads and writes like that run"
        ),
        "{answer}"
    );
    // A run's own session is not told.
    let mut run = session(
        McpServer::new(fx.engine.clone())
            .with_unattended(true)
            .with_run_domain("eng"),
    )
    .await;
    let answer = tool(&mut run, 71, "list_domains", json!({})).await;
    assert!(!answer.to_string().contains("is going"), "{answer}");
}
