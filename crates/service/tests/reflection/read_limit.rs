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
        .unwrap();
    assert!(eng.contains("harbor"), "{eng:?}");
    let harbor = fx
        .engine
        .read_limit_for(&runner(), true, Some("harbor"))
        .await
        .unwrap();
    assert!(harbor.is_empty(), "{harbor:?}");
    let both = fx
        .engine
        .read_limit_for(&runner(), true, None)
        .await
        .unwrap();
    assert!(both.contains("harbor"), "{both:?}");
    // A domain it holds no lease on cannot widen what it reads.
    let named = fx
        .engine
        .read_limit_for(&user("sam"), true, Some("harbor"))
        .await
        .unwrap();
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
