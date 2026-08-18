//! The engine's attachment seam: bytes in and out of a file domain's `assets/`
//! folder and out of a virtual domain's blob table, plus the reserved-prefix
//! refusal on the engram write path.
//!
//! Every fixture holds a [`support::ScratchStateDir`]: an attachment write
//! marks its domain pending in the maintenance state file, which lives under
//! the state directory, so a run must never reach the developer's own.

mod support;

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::params::WriteParams;
use crystalline_service::{Engine, EngineError};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// A single-pixel-ish PNG stand-in: the bytes never have to decode, only to
/// round-trip, so this is a short binary blob with a NUL in it (which is what
/// would break a text-shaped path).
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00binary\x00bytes";

/// A minimal `write_engram` request into `eng`, optionally filed in a folder.
fn write_params(title: &str, folder: Option<&str>) -> WriteParams {
    WriteParams {
        domain: "eng".to_string(),
        title: title.to_string(),
        content: "body".to_string(),
        folder: folder.map(str::to_string),
        engram_type: None,
        tags: vec!["eng".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
    }
}

/// Write `name`'s MANIFEST into `dir`, the routing file every domain needs.
fn write_manifest(dir: &std::path::Path, name: &str) {
    std::fs::write(
        dir.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
        ),
    )
    .unwrap();
}

/// A file domain `eng` holding MANIFEST + alpha and a virtual domain
/// `scratch`, synced into an in-memory store. The tuple's last element is the
/// scratch state directory guard; dropping it restores the environment.
async fn engine_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    std::path::PathBuf,
    support::ScratchStateDir,
) {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    write_manifest(&dir, "eng");
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir.clone()));
    cfg.domains
        .insert("scratch".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    (tmp, engine, dir, scratch)
}

#[tokio::test]
async fn a_file_domain_attachment_lands_under_assets_and_round_trips() {
    let (_tmp, engine, root, _scratch) = engine_fixture().await;

    let row = engine
        .attachment_write("eng", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    assert_eq!(row.path, "assets/shot.png");
    assert_eq!(row.mime, "image/png");
    assert_eq!(row.size, PNG.len() as u64);
    assert_eq!(row.sha256.len(), 64, "a hex sha256");

    // The bytes are a plain file under the domain root, which is what makes a
    // git team domain carry them.
    let on_disk = root.join("assets").join("shot.png");
    assert_eq!(std::fs::read(&on_disk).unwrap(), PNG);
    // Nothing of the atomic write survives beside it.
    let leftovers: Vec<String> = std::fs::read_dir(root.join("assets"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name != "shot.png")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );

    // The row is there without waiting for a walker pass.
    let listed = engine.attachment_list("eng").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], row);

    let (bytes, read_row) = engine
        .attachment_read("eng", "assets/shot.png")
        .await
        .unwrap();
    assert_eq!(bytes, PNG);
    assert_eq!(read_row, row);

    engine
        .attachment_delete("eng", "assets/shot.png")
        .await
        .unwrap();
    assert!(!on_disk.exists(), "the file is gone");
    assert!(engine.attachment_list("eng").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_virtual_domain_attachment_lives_in_the_database_only() {
    let (tmp, engine, _root, _scratch) = engine_fixture().await;

    let row = engine
        .attachment_write("scratch", "assets/notes/data.json", b"{\"a\":1}".to_vec())
        .await
        .unwrap();
    assert_eq!(row.mime, "application/json");
    assert_eq!(row.size, 7);

    // Nothing anywhere on disk: a virtual domain has no root to write to, and
    // the file domain beside it is untouched.
    assert!(
        !tmp.path().join("scratch").exists(),
        "a virtual domain grew a folder"
    );
    assert!(
        !tmp.path().join("eng").join("assets").exists(),
        "a virtual attachment landed in the file domain"
    );

    let (bytes, read_row) = engine
        .attachment_read("scratch", "assets/notes/data.json")
        .await
        .unwrap();
    assert_eq!(bytes, b"{\"a\":1}");
    assert_eq!(read_row, row);

    engine
        .attachment_delete("scratch", "assets/notes/data.json")
        .await
        .unwrap();
    assert!(engine.attachment_list("scratch").await.unwrap().is_empty());
    let err = engine
        .attachment_read("scratch", "assets/notes/data.json")
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::NotFound(_)),
        "a deleted blob reads as a miss, got {err:?}"
    );
}

#[tokio::test]
async fn a_write_refuses_a_traversal_path_a_bad_extension_and_an_oversized_body() {
    let (_tmp, engine, _root, _scratch) = engine_fixture().await;

    for bad in [
        "assets/../escape.png",
        "assets/tool.exe",
        "notes/shot.png",
        "assets/.hidden/x.png",
    ] {
        let err = engine
            .attachment_write("eng", bad, PNG.to_vec())
            .await
            .unwrap_err();
        assert!(
            matches!(err, EngineError::Invalid(_)),
            "'{bad}' must be refused as invalid, got {err:?}"
        );
    }

    let over = vec![0u8; (crystalline_core::attachment::MAX_ATTACHMENT_BYTES + 1) as usize];
    let err = engine
        .attachment_write("eng", "assets/big.pdf", over)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Invalid(_)),
        "an over-cap body must be refused, got {err:?}"
    );
    assert!(engine.attachment_list("eng").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_write_marks_its_domain_pending_and_so_does_a_delete() {
    let (_tmp, engine, _root, scratch) = engine_fixture().await;
    assert!(
        crystalline_service::maintenance::load()
            .pending_domains
            .is_empty()
    );

    engine
        .attachment_write("eng", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    assert_eq!(
        crystalline_service::maintenance::load().pending_domains,
        vec!["eng".to_string()],
        "an upload owes the human a sweep"
    );
    assert!(scratch.maintenance_path().exists());

    // A delete is a change to what the domain carries too.
    engine
        .attachment_write("scratch", "assets/x.txt", b"hi".to_vec())
        .await
        .unwrap();
    engine
        .attachment_delete("scratch", "assets/x.txt")
        .await
        .unwrap();
    assert_eq!(
        crystalline_service::maintenance::load().pending_domains,
        vec!["eng".to_string(), "scratch".to_string()]
    );
}

#[tokio::test]
async fn a_read_indexes_a_file_that_has_no_row_yet_and_misses_on_an_absent_one() {
    let (_tmp, engine, root, _scratch) = engine_fixture().await;

    // A file placed by hand (a git pull, an editor) with no walker pass since.
    let dir = root.join("assets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hand.png"), PNG).unwrap();
    assert!(engine.attachment_list("eng").await.unwrap().is_empty());

    let (bytes, row) = engine
        .attachment_read("eng", "assets/hand.png")
        .await
        .unwrap();
    assert_eq!(bytes, PNG);
    assert_eq!(row.mime, "image/png");
    assert_eq!(row.size, PNG.len() as u64);
    assert_eq!(
        engine.attachment_list("eng").await.unwrap(),
        vec![row],
        "the read indexed it on the fly"
    );

    let err = engine
        .attachment_read("eng", "assets/missing.png")
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::NotFound(_)),
        "an absent file is a miss, got {err:?}"
    );
    let err = engine
        .attachment_delete("eng", "assets/missing.png")
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::NotFound(_)),
        "deleting nothing is a miss, got {err:?}"
    );
}

#[tokio::test]
async fn a_rewritten_attachment_refreshes_its_row_and_a_stale_row_heals_on_read() {
    let (_tmp, engine, root, _scratch) = engine_fixture().await;

    let first = engine
        .attachment_write("eng", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    let second = engine
        .attachment_write("eng", "assets/shot.png", b"replaced bytes".to_vec())
        .await
        .unwrap();
    assert_ne!(first.sha256, second.sha256, "the row followed the bytes");
    assert_eq!(second.size, 14);
    assert_eq!(
        engine.attachment_list("eng").await.unwrap().len(),
        1,
        "a replace refreshes the row rather than adding one"
    );

    // An edit behind the index (a git pull) leaves the row describing the old
    // bytes; the read that serves the new ones refreshes it, so the sha a
    // caller caches on always describes what it received.
    std::fs::write(
        root.join("assets").join("shot.png"),
        b"a third, longer set of bytes",
    )
    .unwrap();
    let (bytes, row) = engine
        .attachment_read("eng", "assets/shot.png")
        .await
        .unwrap();
    assert_eq!(bytes, b"a third, longer set of bytes");
    assert_eq!(row.size, bytes.len() as u64);
    assert_ne!(row.sha256, second.sha256);
}

#[tokio::test]
async fn an_engram_write_refuses_the_reserved_assets_prefix() {
    let (_tmp, engine, _root, _scratch) = engine_fixture().await;

    for folder in ["assets", "assets/deep", "/assets/"] {
        let err = engine
            .write_engram(&write_params("Notes", Some(folder)))
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("assets is reserved for attachments"),
            "folder '{folder}' must be refused with the reserved message, got: {message}"
        );
    }

    // The neighbouring folder name is an ordinary one.
    engine
        .write_engram(&write_params("Notes", Some("assets-notes")))
        .await
        .unwrap();
    // And a title that slugifies to `assets` is fine at the root: it is a file
    // named assets.md, not the reserved folder.
    engine
        .write_engram(&write_params("Assets", None))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_move_and_a_restore_refuse_the_reserved_assets_prefix() {
    let (_tmp, engine, _root, _scratch) = engine_fixture().await;

    let err = engine
        .move_engram(&crystalline_service::params::MoveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            destination: "assets/alpha.md".to_string(),
            destination_domain: None,
            update_links: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("assets is reserved for attachments"),
        "a move into assets/ must be refused, got: {err}"
    );

    let err = engine
        .restore_engram("eng", "assets/alpha.md", ALPHA)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("assets is reserved for attachments"),
        "a restore into assets/ must be refused, got: {err}"
    );
}
