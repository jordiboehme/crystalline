//! The generated OKF `index.md` files and the reserved filenames, driven
//! through the engine.
//!
//! Covers the four content mutations and the sync path that regenerate a file
//! domain's listings, the guards that refuse a reserved destination, and the
//! three cases that must generate nothing at all: a virtual domain, a
//! read-only engine and the `index.files` setting turned off.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, IndexConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::*;
use tokio::sync::Mutex;

fn engram(title: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

fn seed(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

/// An engine over one file domain named `notes` rooted at `root`.
async fn file_engine(root: &Path) -> Engine {
    engine_with(root, GlobalConfig::default(), false).await
}

async fn engine_with(root: &Path, mut cfg: GlobalConfig, read_only: bool) -> Engine {
    cfg.domains
        .insert("notes".to_string(), DomainEntry::file(root.to_path_buf()));
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(Arc::new(Mutex::new(store)), cfg, None, None).with_read_only(read_only)
}

fn write_params(title: &str, folder: Option<&str>) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: "the body".to_string(),
        folder: folder.map(str::to_string),
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
    }
}

#[tokio::test]
async fn a_write_generates_the_root_and_folder_listings() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(root, "MANIFEST.md", &engram("Notes", "the manifest"));
    let engine = file_engine(root).await;
    engine.sync(None).await.unwrap();

    // The seeding sync alone already produced the root listing.
    assert_eq!(
        read(root, "index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [Notes](MANIFEST.md)\n"
    );

    engine
        .write_engram(&write_params("Rollback Runbook", Some("runbooks")))
        .await
        .unwrap();

    assert_eq!(
        read(root, "index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n\
         # Contents\n\n\
         * [Notes](MANIFEST.md)\n\
         * [runbooks/](runbooks/)\n"
    );
    assert_eq!(
        read(root, "runbooks/index.md"),
        "# Contents\n\n* [Rollback Runbook](rollback-runbook.md)\n"
    );
}

#[tokio::test]
async fn a_move_and_a_delete_keep_both_folders_in_step() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let engine = file_engine(root).await;
    engine
        .write_engram(&write_params("Restart", Some("runbooks")))
        .await
        .unwrap();
    engine
        .write_engram(&write_params("Loose", None))
        .await
        .unwrap();

    // Moving the only engram out of a folder removes that folder's listing.
    engine
        .move_engram(&MoveParams {
            identifier: "runbooks/restart".to_string(),
            domain: "notes".to_string(),
            destination: "archive/restart.md".to_string(),
            destination_domain: None,
            update_links: None,
        })
        .await
        .unwrap();
    assert!(!root.join("runbooks/index.md").exists());
    assert_eq!(
        read(root, "archive/index.md"),
        "# Contents\n\n* [Restart](restart.md)\n"
    );

    // Deleting it empties the archive folder again.
    engine
        .delete_engram(&DeleteParams {
            identifier: "archive/restart".to_string(),
            domain: "notes".to_string(),
        })
        .await
        .unwrap();
    assert!(!root.join("archive/index.md").exists());
    assert!(!read(root, "index.md").contains("archive/"));
}

#[tokio::test]
async fn an_edit_that_changes_the_title_updates_the_listing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let engine = file_engine(root).await;
    engine
        .write_engram(&write_params("Draft", None))
        .await
        .unwrap();
    assert_eq!(
        read(root, "index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [Draft](draft.md)\n"
    );

    engine
        .edit_engram(&EditParams {
            identifier: "draft".to_string(),
            domain: "notes".to_string(),
            operation: "find_replace".to_string(),
            content: "title: Final".to_string(),
            section: None,
            find_text: Some("title: Draft".to_string()),
            expected_replacements: None,
            include_subsections: false,
            expected_checksum: None,
        })
        .await
        .unwrap();

    assert_eq!(
        read(root, "index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [Final](draft.md)\n"
    );
}

#[tokio::test]
async fn a_sync_picks_up_an_out_of_band_file_and_leaves_an_unchanged_index_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(
        root,
        "a.md",
        &engram("A", "the body").replace("status: current", "status: current\ndescription: An A"),
    );
    let engine = file_engine(root).await;
    engine.sync(None).await.unwrap();
    assert_eq!(
        read(root, "index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [A](a.md) - An A\n"
    );

    // A second sync changes nothing on disk, so the index file is not rewritten.
    let before = std::fs::metadata(root.join("index.md"))
        .unwrap()
        .modified()
        .unwrap();
    engine.sync(None).await.unwrap();
    let after = std::fs::metadata(root.join("index.md"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after, "an unchanged index must keep its mtime");

    // A file dropped in out of band lands in the listing on the next sync.
    seed(root, "b.md", &engram("B", "another body"));
    engine.sync(None).await.unwrap();
    assert_eq!(
        read(root, "index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Contents\n\n* [A](a.md) - An A\n* [B](b.md)\n"
    );
}

#[tokio::test]
async fn a_dropped_in_index_or_log_file_is_never_indexed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(root, "a.md", &engram("A", "the body"));
    seed(root, "log.md", "# Log\n\nreservedlogtoken\n");
    let engine = file_engine(root).await;
    engine.sync(None).await.unwrap();

    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("reservedlogtoken".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 0, "a reserved file never reaches search");

    let browsed = engine
        .browse_domain(&BrowseParams {
            domain: "notes".to_string(),
            path: None,
            depth: None,
            glob: None,
        })
        .await
        .unwrap();
    let paths: Vec<&str> = browsed["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["a.md"], "browse lists concepts only");
    // The generated index file itself is not indexed either.
    assert!(root.join("index.md").is_file());
    assert!(!paths.contains(&"index.md"));
}

#[tokio::test]
async fn a_reserved_destination_is_refused_with_an_actionable_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let engine = file_engine(root).await;

    // A title that slugifies onto a reserved filename.
    let err = engine
        .write_engram(&write_params("Index", None))
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("index.md"), "{message}");
    assert!(message.contains("reserved"), "{message}");
    assert!(!root.join("index.md").exists(), "nothing was written");

    let err = engine
        .write_engram(&write_params("Log", Some("runbooks")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("log.md"), "{err}");

    // And a move onto one.
    engine
        .write_engram(&write_params("Keeper", None))
        .await
        .unwrap();
    let err = engine
        .move_engram(&MoveParams {
            identifier: "keeper".to_string(),
            domain: "notes".to_string(),
            destination: "runbooks/index.md".to_string(),
            destination_domain: None,
            update_links: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("reserved"), "{err}");
    assert!(root.join("keeper.md").is_file(), "the engram stayed put");
}

#[tokio::test]
async fn the_setting_turned_off_generates_nothing_and_leaves_existing_files_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(root, "a.md", &engram("A", "the body"));
    seed(root, "index.md", "# Contents\n\nhand written\n");

    let cfg = GlobalConfig {
        index: Some(IndexConfig { files: Some(false) }),
        ..GlobalConfig::default()
    };
    let engine = engine_with(root, cfg, false).await;
    engine.sync(None).await.unwrap();
    engine.write_engram(&write_params("B", None)).await.unwrap();

    assert_eq!(
        read(root, "index.md"),
        "# Contents\n\nhand written\n",
        "the setting off leaves an existing index file untouched"
    );
    // It stays out of the index either way.
    let hits = engine
        .search_engrams(&SearchParams {
            query: Some("hand".to_string()),
            ..SearchParams::default()
        })
        .await
        .unwrap();
    assert_eq!(hits["total"], 0);
}

#[tokio::test]
async fn a_read_only_engine_never_writes_an_index_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed(root, "a.md", &engram("A", "the body"));
    let engine = engine_with(root, GlobalConfig::default(), true).await;
    engine.sync(None).await.unwrap();
    assert!(
        !root.join("index.md").exists(),
        "a read-only instance leaves the curating side to write the index"
    );
}

#[tokio::test]
async fn a_virtual_domain_generates_no_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("ideas".to_string(), DomainEntry::virtual_domain());
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, None);

    engine
        .write_engram(&WriteParams {
            domain: "ideas".to_string(),
            title: "An Idea".to_string(),
            content: "the body".to_string(),
            folder: None,
            engram_type: None,
            tags: Vec::new(),
            status: None,
            metadata: None,
            overwrite: false,
        })
        .await
        .unwrap();

    // Nothing anywhere: a virtual domain has no filesystem root to navigate.
    assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
}
