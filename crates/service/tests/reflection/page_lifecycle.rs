//! A Reflection page's life beside its text: an unattended session never
//! deletes a page (it retires it), a page an agent creates for another page
//! is owned by it, a retired page keeps the page that owned it, and a
//! reconcile is recorded beside the page, never in it.

use std::sync::Arc;

use crystalline_core::automation::Loop;
use crystalline_service::params::{DiscussParams, WriteReflectionParams};
use crystalline_service::read_limit::{Limit, within};
use crystalline_service::rest::{AgentLoops, AuthStore};
use crystalline_service::{DomainAccess, EngineError, ReflectionCaller, Scope};

use crate::docs::{agent, person, reflecting, write};
use crate::fixture::Fx;

const PAGE: &str = "# A\n\n## One\n\nFirst.\n";

fn file(fx: &Fx, rel: &str) -> Option<String> {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).ok()
}

async fn put(fx: &Fx, path: &str, text: &str) {
    fx.engine
        .write_reflection(&write(path, text), &person())
        .await
        .unwrap();
}

fn delete(path: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: "eng".into(),
        action: Some("delete".into()),
        path: path.into(),
        ..Default::default()
    }
}

/// An agent of the reflect loop, as its token resolves, with an accounts
/// store on `fx` that knows it.
async fn curator_agent(fx: &Fx) -> ReflectionCaller {
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_agent("curator", None, AgentLoops::Reflect, "cli", "", 30 * 86_400)
        .await
        .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let scope = Scope::Agent {
        name: "curator".to_string(),
        loops: AgentLoops::Reflect,
        token_id: 1,
        token_expires_at: "2099-01-01T00:00:00Z".to_string(),
    };
    ReflectionCaller::agent(scope, "curator", false)
}

fn refused_as_unattended(e: &EngineError, path: &str) {
    assert!(matches!(e, EngineError::Forbidden(_)), "{e}");
    let text = e.to_string();
    assert!(
        text.contains("unattended") && text.contains("retire") && text.contains(path),
        "{text}"
    );
}

/// A session marked unattended (a run, or a registration that says so) may
/// not delete a page: the refusal says to retire it instead, and the page
/// stays. The same caller attended deletes it.
#[tokio::test]
async fn an_unattended_session_retires_instead_of_deleting() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;

    let refused = within(
        Some(Limit::default()),
        fx.engine.write_reflection(&delete("guides/a.md"), &agent()),
    )
    .await
    .unwrap_err();
    refused_as_unattended(&refused, "guides/a.md");
    assert!(file(&fx, "guides/a.md").is_some(), "the page stays");

    // The engine verb itself refuses, whichever surface calls it.
    let refused = within(
        Some(Limit::default()),
        fx.engine.delete_reflection("eng", "guides/a.md", &person()),
    )
    .await
    .unwrap_err();
    refused_as_unattended(&refused, "guides/a.md");

    fx.engine
        .write_reflection(&delete("guides/a.md"), &person())
        .await
        .unwrap();
    assert!(file(&fx, "guides/a.md").is_none(), "attended, it goes");
}

/// An agent of the loops is never anybody's interactive session: it may
/// not delete a page even with no read limit around the call.
#[tokio::test]
async fn an_agent_of_the_loops_cannot_delete_a_page() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    let curator = curator_agent(&fx).await;

    let refused = fx
        .engine
        .write_reflection(&delete("guides/a.md"), &curator)
        .await
        .unwrap_err();
    refused_as_unattended(&refused, "guides/a.md");
    assert!(file(&fx, "guides/a.md").is_some(), "the page stays");
}

/// A page pact is not deleted by an unattended session either.
#[tokio::test]
async fn an_unattended_session_cannot_delete_a_page_pact() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    put(
        &fx,
        "guides/a.pact.md",
        "# Pact of A\n\n## Tone\n\nShort.\n",
    )
    .await;

    let refused = within(
        Some(Limit::default()),
        fx.engine
            .delete_reflection("eng", "guides/a.pact.md", &person()),
    )
    .await
    .unwrap_err();
    assert!(matches!(refused, EngineError::Forbidden(_)), "{refused}");
    assert!(refused.to_string().contains("unattended"), "{refused}");
    assert!(file(&fx, "guides/a.pact.md").is_some());
}

/// A caller that holds a live loop lease is the run at work: its delete is
/// refused while the lease lives, with no read limit around the call.
#[tokio::test]
async fn a_lease_holder_cannot_delete_a_page() {
    let fx = reflecting().await;
    put(&fx, "guides/a.md", PAGE).await;
    // Work for the reflect loop, so a lease can be taken.
    fx.engine
        .discuss(
            &DiscussParams {
                domain: "eng".into(),
                action: Some("open".into()),
                path: Some("guides/a.md".into()),
                section: Some("One".into()),
                text: Some("Still true?".into()),
                ..Default::default()
            },
            &person(),
        )
        .await
        .unwrap();
    fx.engine
        .automation_lease_take(Loop::Reflect, "eng", "cron", &Scope::Unrestricted)
        .await
        .unwrap();

    let refused = fx
        .engine
        .write_reflection(&delete("guides/a.md"), &person())
        .await
        .unwrap_err();
    refused_as_unattended(&refused, "guides/a.md");
    assert!(file(&fx, "guides/a.md").is_some(), "the page stays");
}
