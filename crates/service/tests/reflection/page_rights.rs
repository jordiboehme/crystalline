//! The page settings (`freshness`, `follows`, `status`) and the page pact
//! files (`<page>.pact.md`) are a curator's: a page pact is written, deleted
//! and opened in a room by curators only, and moves and goes with its page;
//! a write below curate keeps the settings as stored.

use std::sync::Arc;

use crystalline_service::rest::{AuthStore, MemberLevel, ReflectionLevel, Role};
use crystalline_service::{DomainAccess, ReflectionCaller, Scope};

use crate::docs::{person, reflecting, write};
use crate::fixture::Fx;

const PAGE: &str = "# A\n\n## One\n\nFirst.\n";
const PACT: &str = "# Pact of A\n\n## Tone\n\nShort.\n";

/// `eng` with its Reflection on, private to `keeper`; `ann` a member who
/// edits the Reflection (contributor) and does not curate it. The machine
/// owner (`person()`) curates.
async fn editor_and_curator() -> (Fx, ReflectionCaller, ReflectionCaller) {
    let fx = reflecting().await;
    let auth = Arc::new(
        AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["keeper", "ann"] {
        auth.add_user(name, name, None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    auth.upsert_domain_member(
        "eng",
        "ann",
        MemberLevel::None,
        Some(ReflectionLevel::Contributor),
        "keeper",
    )
    .await
    .unwrap();
    fx.engine
        .set_domain_access(Arc::new(DomainAccess::new(auth)));
    let ann = ReflectionCaller::person(
        Scope::User {
            account: "ann".into(),
            admin: false,
        },
        "ann",
    );
    (fx, ann, person())
}

fn file(fx: &Fx, rel: &str) -> Option<String> {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).ok()
}

async fn put(
    fx: &Fx,
    path: &str,
    text: &str,
    who: &ReflectionCaller,
) -> Result<serde_json::Value, String> {
    fx.engine
        .write_reflection(&write(path, text), who)
        .await
        .map_err(|e| e.to_string())
}

/// A page pact is written by curators and read by everyone who reads the
/// page; its room takes curate too.
#[tokio::test]
async fn a_page_pact_is_written_only_by_a_curator() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &ann).await.unwrap();
    let err = put(&fx, "guides/a.pact.md", PACT, &ann).await.unwrap_err();
    assert!(err.contains("curate is needed"), "{err}");
    assert!(file(&fx, "guides/a.pact.md").is_none());
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    let err = put(&fx, "guides/a.pact.md", "# Mine\n", &ann)
        .await
        .unwrap_err();
    assert!(err.contains("curate is needed"), "{err}");
    let read = fx
        .engine
        .read_reflection("eng", "guides/a.pact.md", &ann)
        .await
        .unwrap();
    assert!(read.to_string().contains("Short."), "{read}");
    assert!(
        fx.engine
            .require_reflection_edit("eng", "guides/a.pact.md", &ann)
            .await
            .is_err(),
        "the pact's room takes curate"
    );
    fx.engine
        .require_reflection_edit("eng", "guides/a.md", &ann)
        .await
        .unwrap();
    fx.engine
        .require_reflection_edit("eng", "guides/a.pact.md", &curator)
        .await
        .unwrap();
}

/// A page pact alone is deleted by a curator only; its page stays.
#[tokio::test]
async fn a_page_pact_alone_is_deleted_only_by_a_curator() {
    let (fx, ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    let err = fx
        .engine
        .delete_reflection("eng", "guides/a.pact.md", &ann)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("curate is needed"), "{err}");
    assert!(file(&fx, "guides/a.pact.md").is_some());
    fx.engine
        .delete_reflection("eng", "guides/a.pact.md", &curator)
        .await
        .unwrap();
    assert!(file(&fx, "guides/a.pact.md").is_none());
    assert!(file(&fx, "guides/a.md").is_some());
}

/// A page pact moves with its page and never on its own, and no page moves
/// onto a pact file's path.
#[tokio::test]
async fn a_page_pact_is_never_moved_on_its_own() {
    let (fx, _ann, curator) = editor_and_curator().await;
    put(&fx, "guides/a.md", PAGE, &curator).await.unwrap();
    put(&fx, "guides/a.pact.md", PACT, &curator).await.unwrap();
    put(&fx, "guides/b.md", PAGE, &curator).await.unwrap();
    for (from, to) in [
        ("guides/a.pact.md", "guides/c.pact.md"),
        ("guides/b.md", "guides/c.pact.md"),
    ] {
        let err = fx
            .engine
            .move_reflection("eng", from, to, &curator)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("moves with its page"), "{from} -> {to}: {err}");
    }
    assert!(file(&fx, "guides/a.pact.md").is_some());
    assert!(file(&fx, "guides/b.md").is_some());
}
