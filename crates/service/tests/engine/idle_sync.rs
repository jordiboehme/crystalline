//! An engine sync of a domain whose files did not change writes nothing to
//! the index, with collaboration on: the daemon syncs a domain on every
//! watcher flush and every catch-up, and each sync claims the domain's host
//! lock before it walks. A lock this engine already holds, under the lease it
//! keeps and with a fresh heartbeat, is not claimed again, so an idle sync
//! does not rewrite the lock row either.
//!
//! The signal is the write-ahead log of a file-backed Turso index, as in the
//! index suite's `idle_sync`: a transaction that commits a changed page
//! appends a frame to `<db>-wal`, a read appends none, and a checkpoint
//! empties the log before the test measures. The 32-byte log header the
//! engine may write into an emptied log is not a frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore};
use crystalline_service::params::ListDomainsParams;
use crystalline_service::{Engine, Scope};
use tokio::sync::Mutex;

fn manifest() -> String {
    "---\ntype: manifest\ntitle: Notes\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Notes\n\n## Scope\n\n- notes\n\n## When to Use\n\n- always\n\n## Tag Aliases\n\n- old -> new\n".to_string()
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// The bytes of frames in the log, past its 32-byte header.
fn wal_frames(db: &Path) -> u64 {
    let mut wal = db.as_os_str().to_os_string();
    wal.push("-wal");
    std::fs::metadata(PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0)
        .saturating_sub(32)
}

async fn last_sync(engine: &Engine) -> Option<String> {
    let listed = engine
        .list_domains(&ListDomainsParams::default(), &Scope::Unrestricted)
        .await
        .unwrap();
    listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "notes")
        .and_then(|d| d["last_sync"].as_str().map(str::to_string))
}

#[tokio::test]
async fn an_unchanged_engine_sync_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("notes");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        root.join("a.md"),
        engram("Alpha", "alpha", "links to [[Beta]] and [[Nowhere Yet]]"),
    )
    .unwrap();
    std::fs::write(root.join("b.md"), engram("Beta", "beta", "plain body")).unwrap();

    let db = tmp.path().join("index.db");
    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(TursoStore::open(&db).await.unwrap()));
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::file(root.clone()));
    let engine = Engine::new(store.clone(), cfg, None, None)
        .with_state_dir(tmp.path().join("state"))
        .with_instance_id("inst-a".to_string());

    // The first sync claims the lock and indexes everything; the targeted
    // pass after it settles anything the first pass leaves to a second.
    engine.sync(None).await.unwrap();
    engine
        .sync_paths("notes", vec!["a.md".to_string()])
        .await
        .unwrap();
    store.lock().await.checkpoint_wal().await.unwrap();
    assert_eq!(wal_frames(&db), 0, "the checkpoint emptied the log");
    let stamped = last_sync(&engine).await.expect("the domain was synced");

    std::thread::sleep(std::time::Duration::from_millis(20));
    for _ in 0..2 {
        engine.sync(None).await.unwrap();
        engine.sync(Some("notes")).await.unwrap();
        engine
            .sync_paths("notes", vec!["a.md".to_string(), "MANIFEST.md".to_string()])
            .await
            .unwrap();
    }
    assert_eq!(
        wal_frames(&db),
        0,
        "an unchanged engine sync wrote to the index"
    );
    // The listing still says the domain was synced just now: the engine
    // keeps the time of its last sync, which the index no longer stamps
    // when nothing changed.
    assert!(
        last_sync(&engine).await.unwrap() > stamped,
        "the listing shows the last sync, not the last change"
    );

    // The signal sees a real write.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(root.join("b.md"), engram("Beta", "beta", "edited body")).unwrap();
    engine.sync(None).await.unwrap();
    assert!(wal_frames(&db) > 0, "a sync that changed a file wrote");
}
