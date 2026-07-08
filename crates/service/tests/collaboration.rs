//! Shared-database collaboration tests: two instances over one database.
//!
//! Each body shares a single `Arc<Mutex<dyn Store>>` between two engines with
//! distinct instance ids, which is exactly two workers against one shared
//! database: the host lock, the non-host refusal, the database-served read, the
//! virtual-domain cross-instance write and the stale-checksum conflict, and the
//! `--take-over` host migration. Runs against Turso (in-memory, always; the one
//! store is shared through the `Arc`) and Postgres (a fresh per-test schema, both
//! engines opening the same tables, when `CRYSTALLINE_TEST_POSTGRES_URL` is set;
//! skipped with a note otherwise).

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::engine::{Engine, EngineError};
use crystalline_service::params::*;
use tokio::sync::Mutex;

#[cfg(feature = "postgres")]
fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres collaboration leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
                )
            });
            None
        }
    }
}

#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ctc_{}_{}", std::process::id(), n)
}

/// Run a body against Turso (always) and Postgres (when configured). The body is
/// handed one shared store; it clones the `Arc` for each of the two engines.
macro_rules! both_backends {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
                $body(store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let schema = unique_schema();
                    let pg = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .expect("open the postgres test schema");
                    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(pg));
                    $body(store).await;
                    let cleanup = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .unwrap();
                    cleanup.drop_schema().await.unwrap();
                }
            }
        }
    };
}

fn manifest(title: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {title}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n"
    )
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

fn write_params(domain: &str, title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: domain.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
    }
}

fn edit_params(
    identifier: &str,
    domain: &str,
    content: &str,
    expected_checksum: Option<String>,
) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        domain: domain.to_string(),
        operation: "append".to_string(),
        content: content.to_string(),
        section: None,
        find_text: None,
        expected_replacements: None,
        include_subsections: false,
        expected_checksum,
    }
}

async fn collaboration_flow(store: Arc<Mutex<dyn Store>>) {
    // One config both instances share: a file domain (real directory) and a
    // virtual domain. Two engines over the one store are two workers on one DB.
    let tmp = tempfile::tempdir().unwrap();
    let eng_dir = tmp.path().join("eng");
    std::fs::create_dir_all(&eng_dir).unwrap();
    std::fs::write(eng_dir.join("MANIFEST.md"), manifest("Eng")).unwrap();
    std::fs::write(
        eng_dir.join("alpha.md"),
        engram("Alpha", "alpha", "hosted file body about turbines"),
    )
    .unwrap();

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(eng_dir.clone()));
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());

    let engine_a =
        Engine::new(store.clone(), cfg.clone(), None, None).with_instance_id("inst-a".to_string());
    let engine_b =
        Engine::new(store.clone(), cfg.clone(), None, None).with_instance_id("inst-b".to_string());

    // A hosts and syncs the file domain: it claims the host lock and indexes it.
    engine_a.sync(None).await.unwrap();
    // domain_stats (through A) shows A hosts eng.
    let a_domains = engine_a
        .list_domains(&ListDomainsParams::default())
        .await
        .unwrap();
    let eng_entry = a_domains["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "eng")
        .unwrap();
    assert_eq!(eng_entry["host"]["instance_id"], "inst-a");
    assert_eq!(eng_entry["host"]["hosted_here"], true);

    // B is refused a named sync of the hosted file domain, error naming A.
    match engine_b.sync(Some("eng")).await {
        Err(EngineError::Conflict(msg)) => {
            assert!(msg.contains("inst-a"), "refusal names the host: {msg}");
            assert!(msg.contains("--take-over"), "refusal hints takeover: {msg}");
        }
        other => panic!("B's sync of a hosted domain must be refused, got {other:?}"),
    }

    // Remove the on-disk engram so B genuinely reads it from the shared database
    // (a non-host holds none of A's files); the row survives because A never
    // re-syncs in this test.
    std::fs::remove_file(eng_dir.join("alpha.md")).unwrap();

    // B searches A's hosted domain from the database.
    let hits = engine_b
        .search_engrams(&SearchParams {
            query: Some("turbines".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1, "B searches A's hosted domain from the DB");

    // B reads A's engram, served from the database content column (file gone).
    let read = engine_b
        .read_engram(&ReadParams {
            identifier: "alpha".to_string(),
            domain: Some("eng".to_string()),
        })
        .await
        .unwrap();
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("hosted file body about turbines"),
        "B reads A's engram from the database"
    );

    // B writes into the shared virtual domain (no host lock); A sees it.
    engine_b
        .write_engram(&write_params(
            "notes",
            "Shared Insight",
            "a virtual engram about photosynthesis",
        ))
        .await
        .unwrap();
    let a_hits = engine_a
        .search_engrams(&SearchParams {
            query: Some("photosynthesis".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(a_hits["total"], 1, "A searches B's virtual engram");

    // A stale-checksum edit conflicts: B reads, A moves the engram on, B's edit
    // with the now-stale checksum is refused.
    let b_read = engine_b
        .read_engram(&ReadParams {
            identifier: "shared-insight".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let stale = b_read["checksum"].as_str().unwrap().to_string();
    engine_a
        .edit_engram(&edit_params(
            "shared-insight",
            "notes",
            "A's addition",
            None,
        ))
        .await
        .unwrap();
    let conflict = engine_b
        .edit_engram(&edit_params(
            "shared-insight",
            "notes",
            "B's stale change",
            Some(stale),
        ))
        .await;
    assert!(
        matches!(conflict, Err(EngineError::Conflict(_))),
        "a stale virtual edit across instances conflicts, got {conflict:?}"
    );

    // B migrates hosting with --take-over and acquires the file domain.
    engine_b.sync_take_over(Some("eng"), true).await.unwrap();
    let b_domains = engine_b
        .list_domains(&ListDomainsParams::default())
        .await
        .unwrap();
    let eng_after = b_domains["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "eng")
        .unwrap();
    assert_eq!(eng_after["host"]["instance_id"], "inst-b");
    assert_eq!(eng_after["host"]["hosted_here"], true);

    // With B now hosting, A's named sync is refused, naming B.
    match engine_a.sync(Some("eng")).await {
        Err(EngineError::Conflict(msg)) => {
            assert!(
                msg.contains("inst-b"),
                "after takeover the host is B: {msg}"
            );
        }
        other => panic!("after B takes over, A must be refused, got {other:?}"),
    }
}
both_backends!(
    two_instances_collaborate_over_one_database,
    collaboration_flow
);
