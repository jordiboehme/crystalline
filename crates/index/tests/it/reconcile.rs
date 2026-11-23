//! The reconcile state of a Reflection page: one row per page, kept beside
//! the page row and never in the file. A set replaces the page's row, a move
//! carries it to the new path, and it goes with the page row and with the
//! domain row. A domain rename keeps it, and so does a write of the page row,
//! which is all a reindex does to a page that is still there.

use crystalline_index::{
    DomainId, DomainKind, ReflectionReconcile, ReflectionRow, Store, TursoStore,
};

/// Run a body against Turso (always) and Postgres (when configured).
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                $body(&store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(scratch) = crate::pg::scratch("rc") {
                    let store = crystalline_index::PostgresStore::open_in_schema(
                        scratch.url(),
                        scratch.name(),
                    )
                    .await
                    .expect("open the postgres test schema");
                    $body(&store).await;
                }
            }
        }
    };
}

async fn domain(store: &dyn Store, name: &str) -> DomainId {
    store
        .upsert_domain(name, Some(&format!("/k/{name}")), DomainKind::File)
        .await
        .unwrap()
}

fn state(path: &str, at: &str, outcome: &str) -> ReflectionReconcile {
    ReflectionReconcile {
        path: path.to_string(),
        last_reconciled: at.to_string(),
        outcome: outcome.to_string(),
        pact_hash: format!("hash-of-{path}"),
        version: format!("v-{at}"),
    }
}

async fn page(store: &dyn Store, domain: DomainId, path: &str) {
    let content = format!("# {path}\n");
    store
        .upsert_reflection(
            domain,
            &ReflectionRow {
                path: path.to_string(),
                title: path.to_string(),
                sha256: format!("sha-{path}"),
                size: content.len() as u64,
                content,
                modified: "2026-11-01T00:00:00Z".to_string(),
            },
        )
        .await
        .unwrap();
}

async fn set_replace_and_read(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    let ops = domain(store, "ops").await;
    assert!(store.reflection_reconciles(eng).await.unwrap().is_empty());
    assert_eq!(store.reflection_reconcile(eng, "a.md").await.unwrap(), None);

    let b = state("b.md", "2026-11-20T10:00:00Z", "updated");
    let a = state("a.md", "2026-11-20T11:00:00Z", "unchanged");
    let a_ops = state("a.md", "2026-11-19T09:00:00Z", "updated");
    store.set_reflection_reconcile(eng, &b).await.unwrap();
    store.set_reflection_reconcile(eng, &a).await.unwrap();
    store.set_reflection_reconcile(ops, &a_ops).await.unwrap();
    // A second set replaces the page's row.
    let b2 = state("b.md", "2026-11-21T10:00:00Z", "unchanged");
    store.set_reflection_reconcile(eng, &b2).await.unwrap();

    assert_eq!(
        store.reflection_reconciles(eng).await.unwrap(),
        vec![a.clone(), b2.clone()],
        "sorted by path, the newest set of each page"
    );
    assert_eq!(
        store.reflection_reconcile(eng, "b.md").await.unwrap(),
        Some(b2)
    );
    assert_eq!(
        store.reflection_reconciles(ops).await.unwrap(),
        vec![a_ops],
        "each domain keeps its own"
    );
}

parity!(a_page_state_is_set_replaced_and_read, set_replace_and_read);

async fn move_carries_it(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    let a = state("guides/a.md", "2026-11-20T10:00:00Z", "updated");
    store.set_reflection_reconcile(eng, &a).await.unwrap();
    // A row at the target is replaced by the moved one.
    store
        .set_reflection_reconcile(
            eng,
            &state("guides/b.md", "2026-01-01T00:00:00Z", "updated"),
        )
        .await
        .unwrap();
    store.begin().await.unwrap();
    store
        .move_reflection_reconcile(eng, "guides/a.md", "guides/b.md")
        .await
        .unwrap();
    store.commit().await.unwrap();
    assert_eq!(
        store
            .reflection_reconcile(eng, "guides/a.md")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .reflection_reconcile(eng, "guides/b.md")
            .await
            .unwrap(),
        Some(ReflectionReconcile {
            path: "guides/b.md".to_string(),
            ..a
        })
    );
    // A page with no state moves none, and leaves the target's alone.
    store
        .move_reflection_reconcile(eng, "guides/none.md", "guides/b.md")
        .await
        .unwrap();
    assert!(
        store
            .reflection_reconcile(eng, "guides/b.md")
            .await
            .unwrap()
            .is_some()
    );
}

parity!(a_move_carries_the_state_to_the_new_path, move_carries_it);

async fn goes_with_the_page_row(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    page(store, eng, "a.md").await;
    page(store, eng, "b.md").await;
    store
        .set_reflection_reconcile(eng, &state("a.md", "2026-11-20T10:00:00Z", "updated"))
        .await
        .unwrap();
    store
        .set_reflection_reconcile(eng, &state("b.md", "2026-11-20T10:00:00Z", "updated"))
        .await
        .unwrap();
    assert!(store.delete_reflection(eng, "a.md").await.unwrap());
    assert_eq!(store.reflection_reconcile(eng, "a.md").await.unwrap(), None);
    assert!(
        store
            .reflection_reconcile(eng, "b.md")
            .await
            .unwrap()
            .is_some()
    );

    // A write of the page row (an edit, a sync, a full reindex) only
    // upserts it and never touches the state, so not every page is due
    // after a reindex.
    page(store, eng, "b.md").await;
    assert!(
        store
            .reflection_reconcile(eng, "b.md")
            .await
            .unwrap()
            .is_some()
    );
}

parity!(the_state_goes_with_its_page_row, goes_with_the_page_row);

async fn domain_rename_and_drop(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    let a = state("a.md", "2026-11-20T10:00:00Z", "updated");
    store.set_reflection_reconcile(eng, &a).await.unwrap();
    store.rename_domain_row("eng", "platform").await.unwrap();
    let platform = store.domain_id("platform").await.unwrap().unwrap();
    assert_eq!(
        store.reflection_reconcile(platform, "a.md").await.unwrap(),
        Some(a),
        "the state is the domain row's, so a rename keeps it"
    );
    store.clear_domain(platform).await.unwrap();
    assert!(store.drop_empty_domain_row("platform").await.unwrap());
    // The id may come back for the next registration of the name: nothing
    // of the dropped row's state may come with it.
    let again = domain(store, "platform").await;
    assert!(store.reflection_reconciles(again).await.unwrap().is_empty());
    assert!(
        store
            .reflection_reconciles(platform)
            .await
            .unwrap()
            .is_empty()
    );
}

parity!(
    a_rename_keeps_the_state_and_a_dropped_domain_takes_it,
    domain_rename_and_drop
);

async fn wiped(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    store
        .set_reflection_reconcile(eng, &state("a.md", "2026-11-20T10:00:00Z", "updated"))
        .await
        .unwrap();
    store.wipe().await.unwrap();
    let eng = domain(store, "eng").await;
    assert!(store.reflection_reconciles(eng).await.unwrap().is_empty());
}

parity!(a_wipe_takes_the_state, wiped);
