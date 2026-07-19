//! Virtual-domain engine tests, run against both backends.
//!
//! Every body is a function of an `Arc<Mutex<dyn Store>>`, so the same
//! assertions run against Turso (in-memory, always) and Postgres (a fresh
//! per-test schema when `CRYSTALLINE_TEST_POSTGRES_URL` is set, dropped
//! afterwards; skipped with a note otherwise). They exercise the virtual CRUD
//! path, the stale-edit CAS conflict, the full-reindex safety guard that
//! preserves virtual rows and the import/export round-trip, all through the
//! shared `Engine`.

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
                    "note: skipping the postgres virtual-domain leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
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
    format!("ctv_{}_{}", std::process::id(), n)
}

/// Run a body against Turso (always) and Postgres (when configured), each with a
/// fresh, isolated store handed to the engine as a trait object.
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
                    // Drop the schema through a fresh connection (the boxed store
                    // no longer exposes the inherent drop_schema).
                    let cleanup = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .unwrap();
                    cleanup.drop_schema().await.unwrap();
                }
            }
        }
    };
}

/// An engine over a config with a single virtual domain named `notes`.
fn virtual_engine(store: Arc<Mutex<dyn Store>>) -> Engine {
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    Engine::new(store, cfg, None, None)
}

fn write_params(title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
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

// --- virtual CRUD ------------------------------------------------------------

async fn virtual_crud(store: Arc<Mutex<dyn Store>>) {
    let engine = virtual_engine(store);

    // Write goes straight to the database; no filesystem is touched.
    let written = engine
        .write_engram(&write_params("First Note", "the body of the note"))
        .await
        .unwrap();
    assert_eq!(written["permalink"], "first-note");
    assert_eq!(written["action"], "created");

    // Read serves the full markdown from the database, with frontmatter parsed
    // and a checksum returned.
    let read = engine
        .read_engram(&ReadParams {
            identifier: "first-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let content = read["content"].as_str().unwrap();
    assert!(content.contains("the body of the note"));
    assert!(content.contains("title: First Note"), "frontmatter present");
    assert!(read["checksum"].as_str().is_some());

    // Edit rewrites the database row.
    engine
        .edit_engram(&EditParams {
            identifier: "first-note".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: "an appended line".to_string(),
            section: None,
            find_text: None,
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: None,
        })
        .await
        .unwrap();
    let read2 = engine
        .read_engram(&ReadParams {
            identifier: "first-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    assert!(
        read2["content"]
            .as_str()
            .unwrap()
            .contains("an appended line")
    );

    // Search finds it (text mode; no embeddings configured).
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("appended".to_string()),
            domains: vec!["notes".to_string()],
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 1);

    // Browse lists the virtual domain's engrams.
    let browsed = engine
        .browse_domain(&BrowseParams {
            domain: "notes".to_string(),
            path: None,
            depth: None,
            glob: None,
        })
        .await
        .unwrap();
    let engrams = browsed["engrams"].as_array().unwrap();
    assert_eq!(engrams.len(), 1);

    // Delete drops the row.
    engine
        .delete_engram(&DeleteParams {
            identifier: "first-note".to_string(),
            domain: "notes".to_string(),
        })
        .await
        .unwrap();
    let gone = engine
        .read_engram(&ReadParams {
            identifier: "first-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await;
    assert!(matches!(gone, Err(EngineError::NotFound(_))));
}
both_backends!(virtual_domain_crud_round_trips, virtual_crud);

// --- stale-edit CAS conflict -------------------------------------------------

async fn stale_edit_conflict(store: Arc<Mutex<dyn Store>>) {
    let engine = virtual_engine(store);
    engine
        .write_engram(&write_params("Note", "original body"))
        .await
        .unwrap();

    // Read to capture the checksum, then let another edit move the engram on.
    let read = engine
        .read_engram(&ReadParams {
            identifier: "note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let stale_checksum = read["checksum"].as_str().unwrap().to_string();

    engine
        .edit_engram(&EditParams {
            identifier: "note".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: "someone else's change".to_string(),
            section: None,
            find_text: None,
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: None,
        })
        .await
        .unwrap();

    // Editing with the now-stale checksum is refused as a conflict.
    let conflict = engine
        .edit_engram(&EditParams {
            identifier: "note".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: "my stale change".to_string(),
            section: None,
            find_text: None,
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: Some(stale_checksum),
        })
        .await;
    assert!(
        matches!(conflict, Err(EngineError::Conflict(_))),
        "a stale expected_checksum must conflict, got {conflict:?}"
    );

    // The stale change never landed.
    let read2 = engine
        .read_engram(&ReadParams {
            identifier: "note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let content = read2["content"].as_str().unwrap();
    assert!(content.contains("someone else's change"));
    assert!(!content.contains("my stale change"));

    // Re-reading and editing with the fresh checksum succeeds.
    let fresh = read2["checksum"].as_str().unwrap().to_string();
    engine
        .edit_engram(&EditParams {
            identifier: "note".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: "my retried change".to_string(),
            section: None,
            find_text: None,
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: Some(fresh),
        })
        .await
        .unwrap();
}
both_backends!(stale_virtual_edit_conflicts, stale_edit_conflict);

// --- temporal enforcement on the virtual edit path ---------------------------

async fn virtual_edit_drop_is_cas_consistent(store: Arc<Mutex<dyn Store>>) {
    let engine = virtual_engine(store);
    engine
        .write_engram(&write_params("Sentinel Note", "original body"))
        .await
        .unwrap();

    engine
        .edit_engram(&EditParams {
            identifier: "sentinel-note".to_string(),
            domain: "notes".to_string(),
            operation: "find_replace".to_string(),
            content: "status: current\nvalid_to: 9999-12-30\n".to_string(),
            section: None,
            find_text: Some("status: current\n".to_string()),
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: None,
        })
        .await
        .unwrap();

    let read = engine
        .read_engram(&ReadParams {
            identifier: "sentinel-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let content = read["content"].as_str().unwrap();
    assert!(
        !content.contains("valid_to"),
        "the sentinel bound is dropped, not stored: {content}"
    );
    let checksum = read["checksum"].as_str().unwrap().to_string();

    // A second, checksum-guarded edit against the checksum just read must
    // succeed. If enforce_temporal ran after virtual_stamp/index_markdown
    // instead of before, the stored stamp would hash the pre-drop bytes while
    // the stored content column held the post-drop bytes, and this edit would
    // spuriously conflict.
    engine
        .edit_engram(&EditParams {
            identifier: "sentinel-note".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: "a follow-up line".to_string(),
            section: None,
            find_text: None,
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: Some(checksum),
        })
        .await
        .unwrap();
}
both_backends!(
    virtual_edit_sentinel_drop_keeps_cas_consistent,
    virtual_edit_drop_is_cas_consistent
);

async fn virtual_edit_rejects_malformed_temporal_date(store: Arc<Mutex<dyn Store>>) {
    let engine = virtual_engine(store);
    engine
        .write_engram(&write_params("Timestamp Note", "original body"))
        .await
        .unwrap();

    let before = engine
        .read_engram(&ReadParams {
            identifier: "timestamp-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let before_content = before["content"].as_str().unwrap().to_string();
    let before_checksum = before["checksum"].as_str().unwrap().to_string();

    let err = engine
        .edit_engram(&EditParams {
            identifier: "timestamp-note".to_string(),
            domain: "notes".to_string(),
            operation: "find_replace".to_string(),
            content: "status: current\nvalid_to: 2026-07-15T10:30:00Z\n".to_string(),
            section: None,
            find_text: Some("status: current\n".to_string()),
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("must be a plain ISO date (YYYY-MM-DD)"),
        "expected a day-granularity rejection, got {err}"
    );

    // The row is untouched by the rejected edit: a fresh read still returns
    // the pre-edit content and checksum, which still guards a normal edit.
    let after = engine
        .read_engram(&ReadParams {
            identifier: "timestamp-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(after["content"].as_str().unwrap(), before_content);
    assert_eq!(after["checksum"].as_str().unwrap(), before_checksum);

    engine
        .edit_engram(&EditParams {
            identifier: "timestamp-note".to_string(),
            domain: "notes".to_string(),
            operation: "append".to_string(),
            content: "a normal follow-up".to_string(),
            section: None,
            find_text: None,
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: Some(before_checksum),
        })
        .await
        .unwrap();
}
both_backends!(
    virtual_edit_rejects_a_malformed_temporal_date,
    virtual_edit_rejects_malformed_temporal_date
);

// --- full reindex preserves virtual data -------------------------------------

async fn full_reindex_preserves_virtual(store: Arc<Mutex<dyn Store>>) {
    // A config with one file domain (real temp dir) and one virtual domain.
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(
        docs.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: Docs\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Docs\n\n## Scope\n\n- docs\n\n## When to Use\n\n- routing\n",
    )
    .unwrap();
    std::fs::write(
        docs.join("page.md"),
        "---\ntype: engram\ntitle: Page\npermalink: page\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Page\n\nfile body\n",
    )
    .unwrap();

    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("docs".to_string(), DomainEntry::file(docs.clone()));
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    let engine = Engine::new(store, cfg, None, None);

    // Index the file domain and capture a virtual engram.
    engine.sync(None).await.unwrap();
    engine
        .write_engram(&write_params("Kept Note", "virtual body that must survive"))
        .await
        .unwrap();

    // A full reindex clears and resyncs the file domain but must leave the
    // virtual rows intact.
    engine.reindex(true).await.unwrap();

    let read = engine
        .read_engram(&ReadParams {
            identifier: "kept-note".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .expect("virtual engram survives a full reindex");
    assert!(
        read["content"]
            .as_str()
            .unwrap()
            .contains("virtual body that must survive")
    );

    // The file domain is still indexed too.
    let page = engine
        .read_engram(&ReadParams {
            identifier: "page".to_string(),
            domain: Some("docs".to_string()),
        })
        .await
        .unwrap();
    assert!(page["content"].as_str().unwrap().contains("file body"));
}
both_backends!(
    full_reindex_keeps_virtual_rows,
    full_reindex_preserves_virtual
);

// --- import / export round-trip ----------------------------------------------

async fn import_export_round_trip(store: Arc<Mutex<dyn Store>>) {
    let engine = virtual_engine(store);

    // A source tree of well-formed engram files.
    let src = tempfile::tempdir().unwrap();
    let one = "---\ntype: engram\ntitle: One\npermalink: one\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# One\n\nbody one\n";
    let two = "---\ntype: engram\ntitle: Two\npermalink: two\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Two\n\nbody two\n";
    std::fs::write(src.path().join("one.md"), one).unwrap();
    std::fs::create_dir_all(src.path().join("sub")).unwrap();
    std::fs::write(src.path().join("sub/two.md"), two).unwrap();

    // Import loads them verbatim into the virtual domain.
    let report = engine
        .import_domain("notes", src.path(), false, false)
        .await
        .unwrap();
    assert_eq!(report["files_written"], 2);
    assert_eq!(report["collisions"].as_array().unwrap().len(), 0);

    // A re-import without overwrite is a no-op: both collide.
    let again = engine
        .import_domain("notes", src.path(), false, false)
        .await
        .unwrap();
    assert_eq!(again["files_written"], 0);
    assert_eq!(again["collisions"].as_array().unwrap().len(), 2);

    // Export writes the engrams back to a folder, byte-identical to the source.
    let dest = tempfile::tempdir().unwrap();
    let export = engine
        .export_domain("notes", dest.path(), true, false)
        .await
        .unwrap();
    assert_eq!(export["files_written"], 2);

    let exported_one = std::fs::read_to_string(dest.path().join("one.md")).unwrap();
    let exported_two = std::fs::read_to_string(dest.path().join("sub/two.md")).unwrap();
    assert_eq!(exported_one, one, "export round-trips byte-identically");
    assert_eq!(
        exported_two, two,
        "nested export round-trips byte-identically"
    );
}
both_backends!(virtual_import_export_round_trips, import_export_round_trip);

// --- import refuses a file domain --------------------------------------------

async fn import_refuses_file_domain(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains.insert(
        "files".to_string(),
        DomainEntry::file(tmp.path().to_path_buf()),
    );
    let engine = Engine::new(store, cfg, None, None);

    let src = tempfile::tempdir().unwrap();
    let err = engine
        .import_domain("files", src.path(), false, false)
        .await;
    assert!(
        matches!(err, Err(EngineError::Invalid(_))),
        "domain import into a file domain is refused, got {err:?}"
    );
}
both_backends!(import_refuses_a_file_domain, import_refuses_file_domain);

// --- tag rename / merge ------------------------------------------------------

fn tagged(title: &str, content: &str, tags: Vec<&str>) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: tags.into_iter().map(String::from).collect(),
        status: None,
        metadata: None,
        overwrite: false,
    }
}

async fn retag_renames_and_merges(store: Arc<Mutex<dyn Store>>) {
    let engine = virtual_engine(store);
    // Alpha carries `topic` on its frontmatter and on an observation; Beta
    // carries a distinct `other`.
    engine
        .write_engram(&tagged(
            "Alpha",
            "Alpha body.\n\n- [decision] chose it #topic\n",
            vec!["topic"],
        ))
        .await
        .unwrap();
    engine
        .write_engram(&tagged("Beta", "Beta body.", vec!["other"]))
        .await
        .unwrap();

    // Dry run reports the one affected engram and writes nothing.
    let dry = engine
        .retag("topic", "subject", Some("notes"), false, true)
        .await
        .unwrap();
    assert_eq!(dry["rewritten"], 1);
    assert_eq!(dry["dry_run"], true);
    let pre = engine
        .read_engram(&ReadParams {
            identifier: "alpha".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    assert!(
        pre["content"].as_str().unwrap().contains("#topic"),
        "the dry run rewrote nothing"
    );

    // The real rename rewrites both the frontmatter tag and the hashtag.
    let done = engine
        .retag("topic", "subject", Some("notes"), false, false)
        .await
        .unwrap();
    assert_eq!(done["rewritten"], 1);
    let after = engine
        .read_engram(&ReadParams {
            identifier: "alpha".to_string(),
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let content = after["content"].as_str().unwrap();
    assert!(content.contains("- subject"), "frontmatter tag: {content}");
    assert!(
        content.contains("#subject"),
        "observation hashtag: {content}"
    );
    assert!(
        !content.contains("topic"),
        "no trace of the old tag: {content}"
    );

    // The vocabulary now reports `subject`, never `topic`.
    let vocab = engine
        .vocabulary(&VocabularyParams {
            domain: Some("notes".to_string()),
        })
        .await
        .unwrap();
    let names: Vec<&str> = vocab["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"subject") && !names.contains(&"topic"));

    // Renaming into an existing tag is a conflict pointing at merge.
    let conflict = engine
        .retag("other", "subject", Some("notes"), false, false)
        .await;
    assert!(matches!(conflict, Err(EngineError::Conflict(_))));

    // Merging into a missing tag is a not-found.
    let missing = engine
        .retag("other", "ghost", Some("notes"), true, false)
        .await;
    assert!(matches!(missing, Err(EngineError::NotFound(_))));

    // Merging into the existing tag folds `other` away.
    let merged = engine
        .retag("other", "subject", Some("notes"), true, false)
        .await
        .unwrap();
    assert_eq!(merged["rewritten"], 1);
}
both_backends!(
    retag_renames_and_merges_a_virtual_domain,
    retag_renames_and_merges
);

#[tokio::test]
async fn retag_refuses_on_a_read_only_instance() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let engine = virtual_engine(store).with_read_only(true);
    let refused = engine.retag("foo", "bar", Some("notes"), false, true).await;
    assert!(matches!(refused, Err(EngineError::ReadOnly)));
}

#[tokio::test]
async fn retag_rejects_a_non_canonical_name() {
    let store = TursoStore::open_in_memory().await.unwrap();
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
    let engine = virtual_engine(store);
    let bad = engine
        .retag("foo", "Bar_Baz", Some("notes"), false, true)
        .await;
    assert!(matches!(bad, Err(EngineError::Invalid(_))));
}
