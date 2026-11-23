//! The engrams a Reflection page keeps covered after it dropped them (rule A
//! of the reflect queue): written beside the page row by the one writer of
//! page rows, from the row as stored and the new text. They move with the
//! page and go with the page row, the domain row and a wipe.

use crystalline_index::reflection::index_reflection_row;
use crystalline_index::{
    ChunkParams, DomainId, DomainKind, EngramRecord, FileStamp, ReflectionCovered, ReflectionRow,
    Store, TursoStore,
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
                if let Some(scratch) = crate::pg::scratch("cv") {
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

/// A page with `front` as extra frontmatter whose one section cites the
/// engrams `cites` of its own domain, each with the checksum `sum-<engram>`.
fn citing(front: &str, cites: &[&str]) -> String {
    let mut block = String::new();
    if !cites.is_empty() {
        block.push_str("reflection:\n  a1b2:\n    sources:\n");
        for cite in cites {
            block.push_str(&format!(
                "    - engram: {cite}\n      checksum: sum-{cite}\n"
            ));
        }
    }
    format!("---\n{front}{block}---\n# Week\n\n## A\n\nText.\n")
}

async fn write(store: &dyn Store, domain: DomainId, path: &str, content: &str) {
    let row = ReflectionRow {
        path: path.to_string(),
        title: path.to_string(),
        sha256: format!("sha-{}", content.len()),
        size: content.len() as u64,
        content: content.to_string(),
        modified: "2026-11-01T00:00:00Z".to_string(),
    };
    index_reflection_row(store, domain, &row, &ChunkParams::for_model("test"))
        .await
        .unwrap();
}

fn covered(path: &str, permalink: &str) -> ReflectionCovered {
    covered_at(path, permalink, &format!("sum-{permalink}"))
}

fn covered_at(path: &str, permalink: &str, checksum: &str) -> ReflectionCovered {
    ReflectionCovered {
        path: path.to_string(),
        permalink: permalink.to_string(),
        checksum: checksum.to_string(),
    }
}

/// The engram `permalink` of `domain` in the index, at checksum `sha`.
async fn engram_at(store: &dyn Store, domain: DomainId, permalink: &str, sha: &str) {
    let content = format!("# {permalink}\n");
    store
        .upsert_engram(
            domain,
            &EngramRecord {
                path: format!("{permalink}.md"),
                permalink: permalink.to_string(),
                title: permalink.to_string(),
                engram_type: "engram".to_string(),
                status: "current".to_string(),
                recorded_at: None,
                valid_from: None,
                valid_to: None,
                timestamp: None,
                description: None,
                content: content.clone(),
                metadata: serde_json::json!({}),
                tags: Vec::new(),
                observations: Vec::new(),
                relations: Vec::new(),
                links: Vec::new(),
                stamp: FileStamp {
                    mtime: 0,
                    size: content.len() as u64,
                    sha256: sha.to_string(),
                },
                actor: String::new(),
                tombstone: false,
            },
        )
        .await
        .unwrap();
}

/// `w/a` to `w/c` in the index at the checksums the pages cite.
async fn engrams(store: &dyn Store, domain: DomainId) {
    for permalink in ["w/a", "w/b", "w/c"] {
        engram_at(store, domain, permalink, &format!("sum-{permalink}")).await;
    }
}

async fn kept_and_released(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    engrams(store, eng).await;
    let week = |cites: &[&str]| citing("freshness: 7d\n", cites);
    write(store, eng, "week.md", &week(&["w/a", "w/b"])).await;
    write(store, eng, "plain.md", &citing("", &["w/c"])).await;
    assert!(store.reflection_covered(eng).await.unwrap().is_empty());

    // The rolling page drops w/a, the plain page drops w/c.
    write(store, eng, "week.md", &week(&["w/b"])).await;
    write(store, eng, "plain.md", &citing("", &[])).await;
    assert_eq!(
        store.reflection_covered(eng).await.unwrap(),
        vec![covered("week.md", "w/a")]
    );

    // Cited again, it leaves the kept set.
    write(store, eng, "week.md", &week(&["w/a"])).await;
    assert_eq!(
        store.reflection_covered(eng).await.unwrap(),
        vec![covered("week.md", "w/b")]
    );
}

parity!(
    a_rolling_page_keeps_what_it_drops_until_it_cites_it_again,
    kept_and_released
);

async fn moves_and_goes(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    engrams(store, eng).await;
    let week = |cites: &[&str]| citing("freshness: 7d\n", cites);
    write(store, eng, "week.md", &week(&["w/a", "w/b"])).await;
    write(store, eng, "week.md", &week(&["w/b"])).await;

    // A move: the state goes first, then the old row, then the new one.
    store
        .move_reflection_reconcile(eng, "week.md", "moved.md")
        .await
        .unwrap();
    store.delete_reflection(eng, "week.md").await.unwrap();
    write(store, eng, "moved.md", &week(&["w/b"])).await;
    assert_eq!(
        store.reflection_covered(eng).await.unwrap(),
        vec![covered("moved.md", "w/a")]
    );

    assert!(store.delete_reflection(eng, "moved.md").await.unwrap());
    assert!(store.reflection_covered(eng).await.unwrap().is_empty());

    // The domain row and a wipe take what no page row holds any more.
    let kept = [("w/a".to_string(), "sum-w/a".to_string())];
    store
        .update_reflection_covered(eng, "x.md", &kept, &[])
        .await
        .unwrap();
    assert_eq!(store.reflection_covered(eng).await.unwrap().len(), 1);
    store.clear_domain(eng).await.unwrap();
    assert!(store.drop_empty_domain_row("eng").await.unwrap());
    let back = domain(store, "eng").await;
    assert!(store.reflection_covered(back).await.unwrap().is_empty());

    store
        .update_reflection_covered(back, "x.md", &kept, &[])
        .await
        .unwrap();
    store.wipe().await.unwrap();
    let wiped = domain(store, "eng").await;
    assert!(store.reflection_covered(wiped).await.unwrap().is_empty());
}

parity!(
    the_kept_set_moves_with_its_page_and_goes_with_the_page_and_the_domain,
    moves_and_goes
);

/// A dropped engram is kept with its checksum at the drop, not the one the
/// page cited; one the index no longer holds is not kept; and a kept engram
/// whose checksum moved since leaves the set the next time it is touched.
async fn drop_time_checksum(store: &dyn Store) {
    let eng = domain(store, "eng").await;
    engrams(store, eng).await;
    let week = |cites: &[&str]| citing("freshness: 7d\n", cites);
    write(store, eng, "week.md", &week(&["w/a", "w/b", "w/c"])).await;
    // w/a changed while the page cited it; w/c is gone from the index.
    engram_at(store, eng, "w/a", "sum-new").await;
    store.delete_engram(eng, "w/c.md").await.unwrap();
    write(store, eng, "week.md", &week(&["w/b"])).await;
    assert_eq!(
        store.reflection_covered(eng).await.unwrap(),
        vec![covered_at("week.md", "w/a", "sum-new")]
    );

    // w/a moves on; the next drop prunes the dead row.
    engram_at(store, eng, "w/a", "sum-newer").await;
    write(store, eng, "week.md", &week(&[])).await;
    assert_eq!(
        store.reflection_covered(eng).await.unwrap(),
        vec![covered("week.md", "w/b")]
    );
}

parity!(
    a_dropped_engram_is_kept_at_its_checksum_when_dropped,
    drop_time_checksum
);
