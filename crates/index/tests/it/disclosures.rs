//! The disclosures a person confirmed on a Reflection: one row per page, the
//! records opaque JSON text the engine reads. A set replaces the page's row,
//! `None` drops it, and each domain keeps its own.

use crystalline_index::{DomainId, DomainKind, Store, TursoStore};

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
                if let Some(scratch) = crate::pg::scratch("di") {
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

async fn set_replace_and_drop(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    let ops = domain(store, "ops").await;
    assert!(store.reflection_disclosures(eng).await.unwrap().is_empty());
    store
        .set_reflection_disclosures(eng, "b.md", Some("[1]"))
        .await
        .unwrap();
    store
        .set_reflection_disclosures(eng, "a.md", Some("[2]"))
        .await
        .unwrap();
    store
        .set_reflection_disclosures(ops, "a.md", Some("[3]"))
        .await
        .unwrap();
    store
        .set_reflection_disclosures(eng, "b.md", Some("[4]"))
        .await
        .unwrap();
    assert_eq!(
        store.reflection_disclosures(eng).await.unwrap(),
        vec![
            ("a.md".to_string(), "[2]".to_string()),
            ("b.md".to_string(), "[4]".to_string())
        ]
    );
    store
        .set_reflection_disclosures(eng, "a.md", None)
        .await
        .unwrap();
    // Dropping a page that has none is fine.
    store
        .set_reflection_disclosures(eng, "c.md", None)
        .await
        .unwrap();
    assert_eq!(
        store.reflection_disclosures(eng).await.unwrap(),
        vec![("b.md".to_string(), "[4]".to_string())]
    );
    assert_eq!(
        store.reflection_disclosures(ops).await.unwrap(),
        vec![("a.md".to_string(), "[3]".to_string())]
    );
}

parity!(a_page_row_is_set_replaced_and_dropped, set_replace_and_drop);
