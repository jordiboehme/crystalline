//! The reconcile state of a Reflection page, as the engine reads and keeps
//! it: in the index beside the page row, never in the file. A page with no
//! state reads as reconciled when its row last changed, so a new home or a
//! wiped index does not make every page due at once. The state moves with
//! its page, goes with it, and stays through a domain rename.

use std::sync::Arc;

use crystalline_index::{ReflectionReconcile, Store, TursoStore};
use crystalline_service::engine::{PageReconcile, ReconcileOutcome};
use tokio::sync::Mutex;

use crate::docs::{person, write};
use crate::fixture::{Fx, engram, fixture_on, manifest};

const PAGE: &str = "# A\n\n## One\n\nFirst.\n";

/// `eng` with its Reflection on, over a store the test keeps a handle to.
async fn reflecting_on() -> (Fx, Arc<Mutex<dyn Store>>) {
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let fx = fixture_on(
        store.clone(),
        &[
            ("MANIFEST.md", &manifest("reflection: on\n")),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        |_, _| {},
        None,
        None,
        None,
    )
    .await;
    (fx, store)
}

async fn put(fx: &Fx, path: &str, text: &str) {
    fx.engine
        .write_reflection(&write(path, text), &person())
        .await
        .unwrap();
}

fn file(fx: &Fx, rel: &str) -> Option<String> {
    std::fs::read_to_string(fx.root.join("reflections").join(rel)).ok()
}

/// The modified time of the page's index row.
async fn row_modified(store: &Arc<Mutex<dyn Store>>, path: &str) -> String {
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    store
        .get_reflection(id, path)
        .await
        .unwrap()
        .expect("the page has a row")
        .modified
}

async fn record(fx: &Fx, path: &str, outcome: ReconcileOutcome, version: &str) -> PageReconcile {
    fx.engine
        .record_reflection_reconcile_for_tests("eng", path, outcome, "chain-hash", version)
        .await
        .unwrap()
}

/// A page with no state reads as reconciled at its row's modified time,
/// with no outcome; `PACT.md` and page pacts are no pages and have none.
#[tokio::test]
async fn a_page_with_no_state_reads_as_reconciled_when_its_row_changed() {
    let (fx, store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    put(
        &fx,
        "guides/a.pact.md",
        "# Pact of A\n\n## Tone\n\nShort.\n",
    )
    .await;
    put(&fx, "PACT.md", "# Pact\n\n## Tone\n\nPlain.\n").await;

    let seeded = PageReconcile {
        last_reconciled: row_modified(&store, "guides/a.md").await,
        outcome: None,
        pact_hash: None,
        version: None,
    };
    assert_eq!(
        fx.engine
            .reflection_reconcile("eng", "guides/a.md")
            .await
            .unwrap(),
        Some(seeded.clone())
    );
    let all = fx.engine.reflection_reconciles("eng").await.unwrap();
    assert_eq!(
        all.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["guides/a.md"],
        "pages only: {all:?}"
    );
    assert_eq!(all["guides/a.md"], seeded);
    for not_a_page in ["guides/missing.md", "guides/a.pact.md", "PACT.md"] {
        assert_eq!(
            fx.engine
                .reflection_reconcile("eng", not_a_page)
                .await
                .unwrap(),
            None,
            "{not_a_page}"
        );
    }
    assert!(
        fx.engine
            .reflection_reconciles("nowhere")
            .await
            .unwrap()
            .is_empty()
    );
}

/// A recorded reconcile is read back as recorded, and the page file is not
/// touched.
#[tokio::test]
async fn a_recorded_reconcile_is_kept_out_of_the_file() {
    let (fx, _store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    let before = file(&fx, "guides/a.md").unwrap();

    let first = record(&fx, "guides/a.md", ReconcileOutcome::Updated, "v1").await;
    assert_eq!(first.outcome, Some(ReconcileOutcome::Updated));
    assert_eq!(first.pact_hash.as_deref(), Some("chain-hash"));
    assert_eq!(first.version.as_deref(), Some("v1"));
    crystalline_core::time::parse_rfc3339(&first.last_reconciled)
        .expect("last_reconciled is RFC 3339");
    assert_eq!(
        fx.engine
            .reflection_reconcile("eng", "guides/a.md")
            .await
            .unwrap(),
        Some(first)
    );

    let second = record(&fx, "guides/a.md", ReconcileOutcome::Unchanged, "v2").await;
    assert_eq!(
        fx.engine.reflection_reconciles("eng").await.unwrap()["guides/a.md"],
        second
    );
    assert_eq!(
        file(&fx, "guides/a.md").unwrap(),
        before,
        "never in the file"
    );
}

/// The state moves with its page, in the move's own commit.
#[tokio::test]
async fn the_state_moves_with_its_page() {
    let (fx, _store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    let kept = record(&fx, "guides/a.md", ReconcileOutcome::Updated, "v1").await;
    fx.engine
        .move_reflection("eng", "guides/a.md", "guides/b.md", &person())
        .await
        .unwrap();
    assert_eq!(
        fx.engine
            .reflection_reconcile("eng", "guides/b.md")
            .await
            .unwrap(),
        Some(kept)
    );
    assert_eq!(
        fx.engine
            .reflection_reconcile("eng", "guides/a.md")
            .await
            .unwrap(),
        None
    );
}

/// The state goes with its page: a new page at the same path starts from
/// its own row, not from the old page's state.
#[tokio::test]
async fn the_state_goes_with_its_page() {
    let (fx, store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    record(&fx, "guides/a.md", ReconcileOutcome::Updated, "v1").await;
    fx.engine
        .delete_reflection("eng", "guides/a.md", &person())
        .await
        .unwrap();
    put(&fx, "guides/a.md", PAGE).await;
    assert_eq!(
        fx.engine
            .reflection_reconcile("eng", "guides/a.md")
            .await
            .unwrap(),
        Some(PageReconcile {
            last_reconciled: row_modified(&store, "guides/a.md").await,
            outcome: None,
            pact_hash: None,
            version: None,
        })
    );
}

/// A page deleted on disk loses its state with its row at the next sync.
#[tokio::test]
async fn a_page_removed_on_disk_takes_its_state() {
    let (fx, store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    record(&fx, "guides/a.md", ReconcileOutcome::Updated, "v1").await;
    std::fs::remove_file(fx.root.join("reflections/guides/a.md")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    assert_eq!(
        store.reflection_reconcile(id, "guides/a.md").await.unwrap(),
        None
    );
}

/// A domain rename keeps the state: it belongs to the domain's index row.
#[tokio::test]
async fn a_domain_rename_keeps_the_state() {
    let (fx, _store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    let kept = record(&fx, "guides/a.md", ReconcileOutcome::Unchanged, "v1").await;
    fx.engine
        .rename_domain(
            "eng",
            "platform",
            false,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        fx.engine
            .reflection_reconcile("platform", "guides/a.md")
            .await
            .unwrap(),
        Some(kept)
    );
}

/// An outcome this binary does not know (a newer one wrote it) reads as no
/// state, so the page seeds from its row instead of failing the read.
#[tokio::test]
async fn an_unknown_outcome_reads_as_no_state() {
    let (fx, store) = reflecting_on().await;
    put(&fx, "guides/a.md", PAGE).await;
    {
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        store
            .set_reflection_reconcile(
                id,
                &ReflectionReconcile {
                    path: "guides/a.md".into(),
                    last_reconciled: "2026-11-20T10:00:00Z".into(),
                    outcome: "postponed".into(),
                    pact_hash: "h".into(),
                    version: "v".into(),
                },
            )
            .await
            .unwrap();
    }
    let read = fx
        .engine
        .reflection_reconcile("eng", "guides/a.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.outcome, None);
    assert_eq!(
        read.last_reconciled,
        row_modified(&store, "guides/a.md").await
    );
}
