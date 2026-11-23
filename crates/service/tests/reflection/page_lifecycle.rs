//! A Reflection page's life beside its text: an unattended session never
//! deletes a page (it retires it), a page an agent creates for another page
//! is owned by it, a retired page keeps the page that owned it, and a
//! reconcile is recorded beside the page, never in it.

use std::sync::Arc;

use crystalline_core::automation::Loop;
use crystalline_core::reflection as r;
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

    // What it does instead goes through.
    within(
        Some(Limit::default()),
        fx.engine.write_reflection(&act("retire", "guides/a.md"), &agent()),
    )
    .await
    .unwrap();

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

// --- owned pages ------------------------------------------------------------

fn owned(path: &str, owner: &str, text: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        owner: Some(owner.into()),
        ..write(path, text)
    }
}

fn act(action: &str, path: &str) -> WriteReflectionParams {
    WriteReflectionParams {
        domain: "eng".into(),
        action: Some(action.into()),
        path: path.into(),
        ..Default::default()
    }
}

fn record(fx: &Fx, path: &str) -> r::PageRecord {
    r::read_provenance(&file(fx, path).expect("the page is there")).page
}

/// A page an agent creates with `owner` is owned by that page: the record
/// says so, the owner's set is read from the pages, and a hand-written
/// record in the text sets nothing.
#[tokio::test]
async fn a_page_created_with_an_owner_is_owned_by_it() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    let receipt = fx
        .engine
        .write_reflection(&owned("arch/one.md", "arch.md", PAGE), &agent())
        .await
        .unwrap();
    assert_eq!(receipt["owner"], "arch.md", "{receipt}");
    assert_eq!(record(&fx, "arch/one.md").owner.as_deref(), Some("arch.md"));

    // The agent's next write of the page keeps the owner, sent or not.
    fx.engine
        .write_reflection(
            &owned("arch/one.md", "arch.md", "# A\n\n## One\n\nSecond.\n"),
            &agent(),
        )
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("arch/one.md", "# A\n\n## One\n\nThird.\n"), &agent())
        .await
        .unwrap();
    assert_eq!(record(&fx, "arch/one.md").owner.as_deref(), Some("arch.md"));

    // A record written into the text owns nothing: the block is Crystalline's.
    fx.engine
        .write_reflection(
            &write(
                "arch/two.md",
                "---\nreflection:\n  _page:\n    owner: arch.md\n---\n# Two\n",
            ),
            &agent(),
        )
        .await
        .unwrap();
    assert_eq!(record(&fx, "arch/two.md").owner, None);

    let set = fx.engine.reflection_owned_pages("eng").await.unwrap();
    assert_eq!(set.get("arch.md"), Some(&vec!["arch/one.md".to_string()]));
}

/// An owner is set on create only, names an existing page that is not
/// retired and leads nowhere back, and is a curator's to set.
#[tokio::test]
async fn an_owner_is_checked_when_the_page_is_created() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    put(&fx, "notes.md", PAGE).await;
    put(&fx, "old.md", PAGE).await;
    fx.engine
        .write_reflection(&act("retire", "old.md"), &agent())
        .await
        .unwrap();

    for (path, owner, says) in [
        ("arch/one.md", "gone.md", "does not exist"),
        ("arch/one.md", "PACT.md", "names no page"),
        ("arch/one.md", "old.md", "retired"),
        ("arch/one.md", "arch/one.md", "itself"),
        // An existing page takes no new owner.
        ("notes.md", "arch.md", "created"),
    ] {
        let refused = fx
            .engine
            .write_reflection(&owned(path, owner, PAGE), &agent())
            .await
            .unwrap_err();
        assert!(refused.to_string().contains(says), "{owner}: {refused}");
    }
    assert!(file(&fx, "arch/one.md").is_none(), "nothing was written");
    assert_eq!(record(&fx, "notes.md").owner, None);

    // A page under the new path's folder follows it as its folder parent,
    // so it cannot own it.
    put(&fx, "arch/z/child.md", PAGE).await;
    let refused = fx
        .engine
        .write_reflection(&owned("arch/z.md", "arch/z/child.md", PAGE), &agent())
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("leads back"), "{refused}");
    assert!(file(&fx, "arch/z.md").is_none());
}

/// Retiring takes a page out of its owner's set and keeps the owner as the
/// page it was owned by; the status says retired, and it owes no feedback.
/// A restore brings it back, standalone or under the page it was owned by.
#[tokio::test]
async fn a_retired_page_keeps_its_former_owner_and_comes_back() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    fx.engine
        .write_reflection(&owned("arch/one.md", "arch.md", PAGE), &agent())
        .await
        .unwrap();

    let receipt = fx
        .engine
        .write_reflection(&act("retire", "arch/one.md"), &agent())
        .await
        .unwrap();
    assert_eq!(receipt["retired"], true, "{receipt}");
    let text = file(&fx, "arch/one.md").unwrap();
    assert!(text.contains("status: archived"), "{text}");
    let page = record(&fx, "arch/one.md");
    assert_eq!(page.owner, None);
    let retired = page.retired.expect("a retired record");
    assert_eq!(retired.was_owned_by.as_deref(), Some("arch.md"));
    assert_eq!(retired.by, "claude-code");
    assert!(!retired.at.is_empty());
    assert!(
        r::feedback_changes(&text, &file(&fx, ".reflected/arch/one.md").unwrap()).is_empty(),
        "a retire owes no feedback"
    );
    assert!(
        fx.engine
            .reflection_owned_pages("eng")
            .await
            .unwrap()
            .is_empty(),
        "no longer in its owner's set"
    );
    let again = fx
        .engine
        .write_reflection(&act("retire", "arch/one.md"), &agent())
        .await
        .unwrap_err();
    assert!(again.to_string().contains("already retired"), "{again}");

    // Back under another page than the former owner: refused.
    let refused = fx
        .engine
        .write_reflection(
            &WriteReflectionParams {
                owner: Some("notes.md".into()),
                ..act("restore", "arch/one.md")
            },
            &person(),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("arch.md"), "{refused}");

    let receipt = fx
        .engine
        .write_reflection(
            &WriteReflectionParams {
                owner: Some("arch.md".into()),
                ..act("restore", "arch/one.md")
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["restored"], true, "{receipt}");
    let text = file(&fx, "arch/one.md").unwrap();
    assert!(!text.contains("status:"), "{text}");
    let page = record(&fx, "arch/one.md");
    assert_eq!(page.owner.as_deref(), Some("arch.md"));
    assert_eq!(page.retired, None);

    // Retired again and restored on its own.
    fx.engine
        .write_reflection(&act("retire", "arch/one.md"), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&act("restore", "arch/one.md"), &person())
        .await
        .unwrap();
    let page = record(&fx, "arch/one.md");
    assert_eq!((page.owner, page.retired), (None, None));
    let refused = fx
        .engine
        .write_reflection(&act("restore", "arch/one.md"), &person())
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("not retired"), "{refused}");
}

/// A restore under a former owner that is gone is refused; on its own it
/// goes through.
#[tokio::test]
async fn a_restore_under_a_gone_owner_is_refused() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    fx.engine
        .write_reflection(&owned("arch/one.md", "arch.md", PAGE), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&act("retire", "arch/one.md"), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&delete("arch.md"), &person())
        .await
        .unwrap();
    let refused = fx
        .engine
        .write_reflection(
            &WriteReflectionParams {
                owner: Some("arch.md".into()),
                ..act("restore", "arch/one.md")
            },
            &person(),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("does not exist"), "{refused}");
    fx.engine
        .write_reflection(&act("restore", "arch/one.md"), &person())
        .await
        .unwrap();
}

/// Only a curator retires, restores or names an owner; PACT.md and page
/// pacts are no pages to retire.
#[tokio::test]
async fn retire_restore_and_owner_are_a_curators() {
    let (fx, ann, _) = crate::page_rights::editor_and_curator().await;
    put(&fx, "notes.md", PAGE).await;
    let refused = fx
        .engine
        .write_reflection(&owned("arch/one.md", "notes.md", PAGE), &ann)
        .await
        .unwrap_err();
    assert!(matches!(refused, EngineError::Forbidden(_)), "{refused}");
    for action in ["retire", "restore"] {
        let refused = fx
            .engine
            .write_reflection(&act(action, "notes.md"), &ann)
            .await
            .unwrap_err();
        assert!(
            matches!(refused, EngineError::Forbidden(_)),
            "{action}: {refused}"
        );
    }
    assert!(file(&fx, "arch/one.md").is_none());
    assert!(!file(&fx, "notes.md").unwrap().contains("status"));

    put(&fx, "arch.md", PAGE).await;
    put(&fx, "arch.pact.md", "# Pact of arch\n\n## Tone\n\nShort.\n").await;
    for path in ["PACT.md", "arch.pact.md"] {
        let refused = fx
            .engine
            .write_reflection(&act("retire", path), &person())
            .await
            .unwrap_err();
        assert!(
            matches!(refused, EngineError::Invalid(_)),
            "{path}: {refused}"
        );
    }
    let missing = fx
        .engine
        .write_reflection(&act("retire", "gone.md"), &person())
        .await
        .unwrap_err();
    assert!(matches!(missing, EngineError::NotFound(_)), "{missing}");
}

/// A move rewrites the owner and the former owner that name the moved page,
/// in the move itself, and the owner's set follows.
#[tokio::test]
async fn a_move_takes_the_owned_pages_along() {
    let fx = reflecting().await;
    put(&fx, "arch.md", PAGE).await;
    fx.engine
        .write_reflection(&owned("arch/one.md", "arch.md", PAGE), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&owned("arch/two.md", "arch.md", PAGE), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&act("retire", "arch/two.md"), &agent())
        .await
        .unwrap();
    let receipt = fx
        .engine
        .write_reflection(
            &WriteReflectionParams {
                to: Some("design.md".into()),
                ..act("move", "arch.md")
            },
            &person(),
        )
        .await
        .unwrap();
    assert_eq!(receipt["owners_rewritten"], 2, "{receipt}");
    assert_eq!(
        record(&fx, "arch/one.md").owner.as_deref(),
        Some("design.md")
    );
    assert_eq!(
        record(&fx, "arch/two.md")
            .retired
            .unwrap()
            .was_owned_by
            .as_deref(),
        Some("design.md")
    );
    let set = fx.engine.reflection_owned_pages("eng").await.unwrap();
    assert_eq!(set.get("design.md"), Some(&vec!["arch/one.md".to_string()]));
    assert!(!set.contains_key("arch.md"), "{set:?}");
    // The index row holds the rewrite too: the set is read from it.
    let one = file(&fx, "arch/one.md").unwrap();
    assert!(
        r::feedback_changes(&one, &file(&fx, ".reflected/arch/one.md").unwrap()).is_empty(),
        "the rewrite owes no feedback"
    );
}
