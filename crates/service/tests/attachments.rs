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
    named_fixture("eng", "scratch").await
}

/// [`engine_fixture`] with the two domain names chosen by the caller, for a
/// test whose assertions are about process-wide state (the maintenance file)
/// and so must not collide with a sibling running in the same process under
/// plain `cargo test`.
async fn named_fixture(
    file_domain: &str,
    virtual_domain: &str,
) -> (
    tempfile::TempDir,
    Arc<Engine>,
    std::path::PathBuf,
    support::ScratchStateDir,
) {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join(file_domain);
    std::fs::create_dir_all(&dir).unwrap();
    write_manifest(&dir, file_domain);
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert(file_domain.to_string(), DomainEntry::file(dir.clone()));
    cfg.domains
        .insert(virtual_domain.to_string(), DomainEntry::virtual_domain());
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

/// The cross-domain move fixture: the file domains `from` and `into` plus the
/// virtual domain `vault`, on one engine, so a move can cross a domain
/// boundary and a domain kind. Returns the two file roots.
async fn move_fixture() -> (
    tempfile::TempDir,
    Arc<Engine>,
    std::path::PathBuf,
    std::path::PathBuf,
    support::ScratchStateDir,
) {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let from = root.join("from");
    let into = root.join("into");
    for (dir, name) in [(&from, "from"), (&into, "into")] {
        std::fs::create_dir_all(dir).unwrap();
        write_manifest(dir, name);
        cfg.domains
            .insert(name.to_string(), DomainEntry::file(dir.clone()));
    }
    cfg.domains
        .insert("vault".to_string(), DomainEntry::virtual_domain());
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
    (tmp, engine, from, into, scratch)
}

/// An engram source with `extra` frontmatter lines (each already newline
/// terminated) and `body` as its only prose.
fn engram_source(title: &str, permalink: &str, extra: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n{extra}---\n\n# {title}\n\n{body}\n"
    )
}

/// A move request, spelled once for the tests that only vary its ends.
fn move_params(
    domain: &str,
    identifier: &str,
    destination: &str,
    destination_domain: Option<&str>,
) -> crystalline_service::params::MoveParams {
    crystalline_service::params::MoveParams {
        domain: domain.to_string(),
        identifier: identifier.to_string(),
        destination: destination.to_string(),
        destination_domain: destination_domain.map(str::to_string),
        update_links: None,
    }
}

/// The engram text a move landed at `path` in a file domain rooted at `root`.
fn moved_text(root: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
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

/// The maintenance state file is process-wide (`ScratchStateDir` redirects one
/// `HOME` per process, refcounted), and under plain `cargo test` the siblings
/// in this binary run as threads beside this test and write to the same file.
/// So the assertions are `contains` over domain names private to this test,
/// plus a before/after snapshot where an exact claim is wanted: the brief asks
/// that the pending set *gains* the domain, which is exactly what that proves.
#[tokio::test]
async fn a_write_marks_its_domain_pending_and_so_does_a_delete() {
    let (_tmp, engine, _root, scratch) = named_fixture("pending-eng", "pending-scratch").await;
    let before = crystalline_service::maintenance::load().pending_domains;
    assert!(!before.contains(&"pending-eng".to_string()));
    assert!(!before.contains(&"pending-scratch".to_string()));

    engine
        .attachment_write("pending-eng", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    let after_write = crystalline_service::maintenance::load().pending_domains;
    assert!(
        after_write.contains(&"pending-eng".to_string()),
        "an upload owes the human a sweep: {after_write:?}"
    );
    assert!(
        !after_write.contains(&"pending-scratch".to_string()),
        "only the written domain goes pending: {after_write:?}"
    );
    assert!(scratch.maintenance_path().exists());

    // A delete is a change to what the domain carries too, so the domain whose
    // attachment is removed goes pending on the delete alone.
    engine
        .attachment_write("pending-scratch", "assets/x.txt", b"hi".to_vec())
        .await
        .unwrap();
    crystalline_service::maintenance::record_run(&["pending-scratch".to_string()]);
    let after_sweep = crystalline_service::maintenance::load().pending_domains;
    assert!(!after_sweep.contains(&"pending-scratch".to_string()));

    engine
        .attachment_delete("pending-scratch", "assets/x.txt")
        .await
        .unwrap();
    assert!(
        crystalline_service::maintenance::load()
            .pending_domains
            .contains(&"pending-scratch".to_string()),
        "a delete owes the human a sweep too"
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

    for folder in [
        "assets",
        "assets/deep",
        "/assets/",
        "./assets",
        // Case-insensitive: APFS and NTFS resolve `Assets` to the same
        // directory, so the lowercase spelling cannot be the only one refused.
        "Assets",
        "ASSETS/deep",
    ] {
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

    // A folder that climbs out and back in is refused before any path is
    // built: unscreened it would land the file in `assets/` on disk while
    // reading as an ordinary destination.
    for folder in ["a/../assets", "a/b/../../assets/deep"] {
        let err = engine
            .write_engram(&write_params("Notes", Some(folder)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, EngineError::Invalid(_)),
            "folder '{folder}' must be refused, got {err:?}"
        );
    }
    // And so is a plain climb out of the domain, which is the same screen.
    let err = engine
        .write_engram(&write_params("Notes", Some("../outside")))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "got {err:?}");

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

    // Same for the spellings that only look like something else.
    for destination in ["Assets/alpha.md", "a/../assets/alpha.md"] {
        let err = engine
            .move_engram(&crystalline_service::params::MoveParams {
                domain: "eng".to_string(),
                identifier: "alpha".to_string(),
                destination: destination.to_string(),
                destination_domain: None,
                update_links: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, EngineError::Invalid(_)),
            "a move to '{destination}' must be refused, got {err:?}"
        );
    }

    let err = engine
        .restore_engram("eng", "assets/alpha.md", ALPHA)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("assets is reserved for attachments"),
        "a restore into assets/ must be refused, got: {err}"
    );
    let err = engine
        .restore_engram("eng", "a/../assets/alpha.md", ALPHA)
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Invalid(_)),
        "a restore that climbs out and back in must be refused, got {err:?}"
    );
}

/// The containment screen on the engram paths refuses a climb out of the
/// domain and nothing else. A colon is a legal filename character on macOS and
/// Linux and the sync walk indexes such a file like any other engram, so a
/// restore or a move addressing one has to keep working: the stricter character
/// rules belong to untrusted input (an archive entry, an attachment path), not
/// to a name a person gave their own engram. Unix only: a colon is not a legal
/// filename character on Windows, so the fixture itself cannot exist there.
#[cfg(unix)]
#[tokio::test]
async fn an_engram_path_with_a_colon_still_moves_and_restores() {
    let (_tmp, engine, root, _scratch) = engine_fixture().await;

    let plan = ALPHA
        .replace("title: Alpha", "title: Plan v2")
        .replace("permalink: alpha", "permalink: plan-v2");
    engine
        .restore_engram("eng", "notes/plan: v2.md", &plan)
        .await
        .unwrap();
    assert!(root.join("notes").join("plan: v2.md").exists());

    engine
        .move_engram(&crystalline_service::params::MoveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            destination: "notes/rule: two.md".to_string(),
            destination_domain: None,
            update_links: None,
        })
        .await
        .unwrap();
    assert!(root.join("notes").join("rule: two.md").exists());

    // An attachment path is untrusted input and keeps the stricter rule.
    let err = engine
        .attachment_write("eng", "assets/a:b.png", PNG.to_vec())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "got {err:?}");
}

#[tokio::test]
async fn an_over_cap_file_is_refused_by_the_read_and_never_gains_a_row() {
    let (_tmp, engine, root, _scratch) = engine_fixture().await;

    // A file over the ceiling can only arrive behind the index (the write path
    // refuses one and the walker skips it), and the read must agree with both:
    // no bytes loaded, no row minted, so a full scan and a read never disagree
    // about whether the attachment exists.
    let dir = root.join("assets");
    std::fs::create_dir_all(&dir).unwrap();
    let over = vec![0u8; (crystalline_core::attachment::MAX_ATTACHMENT_BYTES + 1) as usize];
    std::fs::write(dir.join("huge.pdf"), &over).unwrap();

    let err = engine
        .attachment_read("eng", "assets/huge.pdf")
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Invalid(_)),
        "an over-cap file must be refused by the read, got {err:?}"
    );
    assert!(
        engine.attachment_list("eng").await.unwrap().is_empty(),
        "the refused read must not mint a row"
    );
}

/// The containment proof, which no path-string test can reach:
/// `validate_asset_path` refuses every textual escape before
/// `contained_asset_path` is called, so a symlink is the only way to exercise
/// the canonicalization half - the half that stops an `assets/` folder (or a
/// folder or file inside it) from pointing at somebody else's disk.
#[cfg(unix)]
#[tokio::test]
async fn a_symlink_out_of_the_domain_is_refused_by_every_attachment_verb() {
    use std::os::unix::fs::symlink;

    let (tmp, engine, root, _scratch) = engine_fixture().await;
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(outside.join("nested")).unwrap();
    std::fs::write(outside.join("nested").join("secret.png"), PNG).unwrap();
    std::fs::write(outside.join("target.png"), PNG).unwrap();

    // 1. The `assets` folder itself is a symlink out of the domain.
    symlink(&outside, root.join("assets")).unwrap();
    for path in ["assets/target.png", "assets/nested/secret.png"] {
        assert_refused_everywhere(&engine, path).await;
    }
    std::fs::remove_file(root.join("assets")).unwrap();

    // 2. An inner segment is the symlink; the folder above it is real.
    std::fs::create_dir_all(root.join("assets")).unwrap();
    symlink(&outside, root.join("assets").join("link")).unwrap();
    assert_refused_everywhere(&engine, "assets/link/target.png").await;

    // 3. The target file itself is a symlink pointing outside.
    symlink(
        outside.join("target.png"),
        root.join("assets").join("file.png"),
    )
    .unwrap();
    assert_refused_everywhere(&engine, "assets/file.png").await;

    // Nothing about this is a blanket refusal of the folder: a real file
    // beside the symlinks still round-trips.
    engine
        .attachment_write("eng", "assets/real.png", PNG.to_vec())
        .await
        .unwrap();
    assert_eq!(
        engine
            .attachment_read("eng", "assets/real.png")
            .await
            .unwrap()
            .0,
        PNG
    );
    // And the bytes outside the domain were never touched.
    assert_eq!(std::fs::read(outside.join("target.png")).unwrap(), PNG);
}

/// Read, write and delete must all refuse `path`, and the file the path
/// resolves to must still be there afterwards.
#[cfg(unix)]
async fn assert_refused_everywhere(engine: &Engine, path: &str) {
    let read = engine.attachment_read("eng", path).await.unwrap_err();
    assert!(
        matches!(read, EngineError::Invalid(_)),
        "read of '{path}' must be refused, got {read:?}"
    );
    let write = engine
        .attachment_write("eng", path, b"overwritten".to_vec())
        .await
        .unwrap_err();
    assert!(
        matches!(write, EngineError::Invalid(_)),
        "write of '{path}' must be refused, got {write:?}"
    );
    let delete = engine.attachment_delete("eng", path).await.unwrap_err();
    assert!(
        matches!(delete, EngineError::Invalid(_)),
        "delete of '{path}' must be refused, got {delete:?}"
    );
}

// --- cross-domain moves carry their attachments -----------------------------

/// A second set of PNG-shaped bytes, so a destination collision can be a
/// genuine one rather than the same file under the same name.
const OTHER_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00other\x00bytes\x00entirely";

#[tokio::test]
async fn a_cross_domain_move_carries_a_sole_referent_attachment() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    let written = engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert!(
        engine.attachment_list("from").await.unwrap().is_empty(),
        "the only referent left, so the row went with it"
    );
    assert!(
        !from.join("assets").join("shot.png").exists(),
        "the source file went with the engram"
    );
    let landed = engine.attachment_list("into").await.unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].path, "assets/shot.png");
    assert_eq!(landed[0].sha256, written.sha256, "the same bytes arrived");
    assert_eq!(
        std::fs::read(into.join("assets").join("shot.png")).unwrap(),
        PNG
    );
    assert!(
        moved_text(&into, "note.md").contains("![shot](assets/shot.png)"),
        "an uncontested name needs no rewrite"
    );
}

#[tokio::test]
async fn a_same_domain_move_leaves_the_attachments_where_they_are() {
    let (_tmp, engine, from, _into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "notes/note.md", None))
        .await
        .unwrap();

    assert_eq!(
        engine.attachment_list("from").await.unwrap().len(),
        1,
        "an assets/ reference is domain-root relative and stays valid"
    );
    assert_eq!(
        std::fs::read(from.join("assets").join("shot.png")).unwrap(),
        PNG
    );
    assert!(moved_text(&from, "notes/note.md").contains("![shot](assets/shot.png)"));
}

#[tokio::test]
async fn an_attachment_another_source_engram_references_is_copied_not_moved() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .restore_engram(
            "from",
            "keeper.md",
            &engram_source("Keeper", "keeper", "", "See [the shot](assets/shot.png)."),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert_eq!(
        engine.attachment_list("from").await.unwrap().len(),
        1,
        "the engram that stayed behind still needs the file"
    );
    assert_eq!(
        std::fs::read(from.join("assets").join("shot.png")).unwrap(),
        PNG
    );
    assert_eq!(engine.attachment_list("into").await.unwrap().len(), 1);
    assert_eq!(
        std::fs::read(into.join("assets").join("shot.png")).unwrap(),
        PNG
    );
}

#[tokio::test]
async fn a_retired_referent_in_the_source_forces_a_copy() {
    let (_tmp, engine, from, _into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .restore_engram(
            "from",
            "old.md",
            &engram_source("Old", "old", "", "![shot](assets/shot.png)")
                .replace("status: stable", "status: archived"),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert_eq!(
        engine.attachment_list("from").await.unwrap().len(),
        1,
        "a retired engram is still a referent, so the file stays"
    );
    assert!(from.join("assets").join("shot.png").exists());
    assert_eq!(engine.attachment_list("into").await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_claimed_attachment_travels_with_no_body_reference_at_all() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source(
                "Note",
                "note",
                "analyzes: assets/deck.pptx\nanalyzed_hash: nope\n",
                "What the deck said.",
            ),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/deck.pptx", b"deck bytes".to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert!(engine.attachment_list("from").await.unwrap().is_empty());
    assert!(!from.join("assets").join("deck.pptx").exists());
    let landed = engine.attachment_list("into").await.unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].path, "assets/deck.pptx");
    assert_eq!(
        std::fs::read(into.join("assets").join("deck.pptx")).unwrap(),
        b"deck bytes"
    );
}

#[tokio::test]
async fn a_destination_holding_the_identical_file_reuses_it() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    engine
        .attachment_write("into", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert_eq!(
        engine.attachment_list("into").await.unwrap().len(),
        1,
        "the same bytes under the same name are already the same file"
    );
    assert_eq!(
        std::fs::read(into.join("assets").join("shot.png")).unwrap(),
        PNG
    );
    assert!(
        engine.attachment_list("from").await.unwrap().is_empty(),
        "the source copy still leaves with its only referent"
    );
    assert!(!from.join("assets").join("shot.png").exists());
    assert!(
        moved_text(&into, "note.md").contains("![shot](assets/shot.png)"),
        "reuse renames nothing"
    );
}

#[tokio::test]
async fn a_destination_collision_with_other_bytes_suffixes_and_rewrites_the_engram() {
    let (_tmp, engine, _from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source(
                "Note",
                "note",
                "analyzes: assets/shot.png\n",
                "![shot](assets/shot.png#right) and again [here](./assets/shot.png).",
            ),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    engine
        .attachment_write("into", "assets/shot.png", OTHER_PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    let landed = engine.attachment_list("into").await.unwrap();
    let paths: Vec<&str> = landed.iter().map(|row| row.path.as_str()).collect();
    assert_eq!(paths, vec!["assets/shot-2.png", "assets/shot.png"]);
    assert_eq!(
        std::fs::read(into.join("assets").join("shot.png")).unwrap(),
        OTHER_PNG,
        "the destination's own file is never overwritten"
    );
    assert_eq!(
        std::fs::read(into.join("assets").join("shot-2.png")).unwrap(),
        PNG
    );
    assert!(engine.attachment_list("from").await.unwrap().is_empty());

    let text = moved_text(&into, "note.md");
    assert!(
        text.contains("![shot](assets/shot-2.png#right)"),
        "the fragment survives the rename: {text}"
    );
    assert!(
        text.contains("[here](./assets/shot-2.png)"),
        "every spelling of the reference follows: {text}"
    );
    assert!(
        text.contains("analyzes: assets/shot-2.png"),
        "the claim follows too: {text}"
    );
}

#[tokio::test]
async fn a_reference_to_a_missing_file_never_fails_the_move() {
    let (_tmp, engine, _from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source(
                "Note",
                "note",
                "analyzes: assets/also-gone.pdf\n",
                "![gone](assets/gone.png)",
            ),
        )
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert!(
        engine.attachment_list("into").await.unwrap().is_empty(),
        "a dangling reference carries nothing"
    );
    let text = moved_text(&into, "note.md");
    assert!(text.contains("![gone](assets/gone.png)"), "{text}");
    assert!(text.contains("analyzes: assets/also-gone.pdf"), "{text}");
}

#[tokio::test]
async fn a_carry_that_cannot_land_surfaces_a_warning_in_the_move_result() {
    let (_tmp, engine, _from, _into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![ghost](assets/ghost.png)"),
        )
        .await
        .unwrap();

    let result = engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    let warnings = result["attachment_warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{result}");
    assert_eq!(
        warnings[0].as_str().unwrap(),
        "attachment 'assets/ghost.png' referenced by 'note' is not in 'from'; \
         the move carries nothing for it"
    );
}

/// The other way a carry can fail: the name is taken at the destination by a
/// different file, and every suffix the rename is allowed to offer is taken
/// too. Nothing is lost - the file stays whole in the source domain - but the
/// reference travelling with the engram now reads as a file that is not the one
/// it was written about, which is exactly the thing a receipt has to say out
/// loud.
#[tokio::test]
async fn an_exhausted_rename_warns_and_leaves_the_file_in_the_source() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    // `assets/shot.png` plus every `-2` through `-99` the suffixing is allowed
    // to reach, each holding bytes of its own so none of them can be reused.
    for attempt in 1..=99u32 {
        let path = if attempt == 1 {
            "assets/shot.png".to_string()
        } else {
            format!("assets/shot-{attempt}.png")
        };
        let bytes = [OTHER_PNG, attempt.to_string().as_bytes()].concat();
        engine.attachment_write("into", &path, bytes).await.unwrap();
    }

    let result = engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    let warnings = result["attachment_warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{result}");
    assert_eq!(
        warnings[0].as_str().unwrap(),
        "attachment 'assets/shot.png' could not be carried to 'into'; its \
         reference at the destination may resolve to a different same-name file"
    );
    assert_eq!(
        std::fs::read(from.join("assets").join("shot.png")).unwrap(),
        PNG,
        "the file stays whole where it already was"
    );
    assert_eq!(
        engine.attachment_list("into").await.unwrap().len(),
        99,
        "nothing was written at the destination"
    );
    assert!(
        moved_text(&into, "note.md").contains("![shot](assets/shot.png)"),
        "a reference that was not carried keeps the spelling it had"
    );
}

/// **One move, both failure modes, and the healthy attachment still travels.**
///
/// The two warnings are pushed at different points of the same plan - a
/// reference the source domain does not hold is reported while the present
/// files are being read, a name that cannot be freed at the destination while
/// the destinations are being settled - so a move that hits both is what proves
/// they accumulate rather than replace one another. The clean attachment beside
/// them carries the other half of the contract: one reference failing must not
/// stop the next one from arriving.
#[tokio::test]
async fn a_move_accumulates_every_carry_warning() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source(
                "Note",
                "note",
                "",
                "![clean](assets/clean.png) ![ghost](assets/ghost.png) ![shot](assets/shot.png)",
            ),
        )
        .await
        .unwrap();
    // Two of the three references have bytes behind them; `assets/ghost.png`
    // deliberately has none, so the source domain holds nothing to carry for it.
    engine
        .attachment_write("from", "assets/clean.png", PNG.to_vec())
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();
    // And every name the rename is allowed to offer `assets/shot.png` at the
    // destination is taken by bytes of its own, so no free name can be settled.
    for attempt in 1..=99u32 {
        let path = if attempt == 1 {
            "assets/shot.png".to_string()
        } else {
            format!("assets/shot-{attempt}.png")
        };
        let bytes = [OTHER_PNG, attempt.to_string().as_bytes()].concat();
        engine.attachment_write("into", &path, bytes).await.unwrap();
    }

    let result = engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    let warnings: Vec<&str> = result["attachment_warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning.as_str().unwrap())
        .collect();
    assert_eq!(
        warnings.len(),
        2,
        "one warning per failure, and none for the reference that travelled: {result}"
    );
    assert!(
        warnings.contains(
            &"attachment 'assets/ghost.png' referenced by 'note' is not in 'from'; \
              the move carries nothing for it"
        ),
        "the missing reference is named: {warnings:?}"
    );
    assert!(
        warnings.contains(
            &"attachment 'assets/shot.png' could not be carried to 'into'; its \
              reference at the destination may resolve to a different same-name file"
        ),
        "and so is the one that found no free name: {warnings:?}"
    );

    // The clean attachment is the sole referent's, so it moved outright.
    assert_eq!(
        std::fs::read(into.join("assets").join("clean.png")).unwrap(),
        PNG,
        "a failure elsewhere in the plan does not strand the healthy carry"
    );
    assert!(
        !from.join("assets").join("clean.png").exists(),
        "and it left the source, the way a sole referent's attachment does"
    );
    assert_eq!(
        std::fs::read(from.join("assets").join("shot.png")).unwrap(),
        PNG,
        "the one that could not land stays whole where it already was"
    );
    assert_eq!(
        engine.attachment_list("into").await.unwrap().len(),
        100,
        "the ninety-nine squatters plus the one attachment that arrived"
    );
}

#[tokio::test]
async fn a_clean_move_reports_an_empty_warnings_array() {
    let (_tmp, engine, _from, _into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    let crossed = engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();
    assert_eq!(crossed["attachment_warnings"], serde_json::json!([]));

    let renamed = engine
        .move_engram(&move_params("into", "note", "notes/note.md", None))
        .await
        .unwrap();
    assert_eq!(
        renamed["attachment_warnings"],
        serde_json::json!([]),
        "a same-domain move carries nothing and so warns about nothing"
    );
}

#[tokio::test]
async fn a_move_between_domain_kinds_carries_the_bytes_both_ways() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    // File domain to virtual domain: the bytes land in the blob table.
    engine
        .move_engram(&move_params("from", "note", "note.md", Some("vault")))
        .await
        .unwrap();
    assert!(engine.attachment_list("from").await.unwrap().is_empty());
    assert!(!from.join("assets").join("shot.png").exists());
    let (bytes, row) = engine
        .attachment_read("vault", "assets/shot.png")
        .await
        .unwrap();
    assert_eq!(bytes, PNG);
    assert_eq!(row.mime, "image/png");

    // And back the other way: the file materializes on disk.
    engine
        .move_engram(&move_params("vault", "note", "note.md", Some("into")))
        .await
        .unwrap();
    assert!(
        engine.attachment_list("vault").await.unwrap().is_empty(),
        "the blob left with its only referent"
    );
    assert_eq!(
        std::fs::read(into.join("assets").join("shot.png")).unwrap(),
        PNG
    );
}

#[tokio::test]
async fn a_case_variant_claim_in_the_source_still_forces_a_copy() {
    let (_tmp, engine, from, _into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "![shot](assets/shot.png)"),
        )
        .await
        .unwrap();
    // The claim names the reserved folder in another case, which is the same
    // folder on APFS and NTFS and is folded to one spelling when the claim is
    // read. Whatever screens the engrams before that reading has to be at
    // least as wide, or a live claim loses its file.
    engine
        .restore_engram(
            "from",
            "claimer.md",
            &engram_source(
                "Claimer",
                "claimer",
                "analyzes: Assets/shot.png\n",
                "What the shot showed.",
            ),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", "assets/shot.png", PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    assert_eq!(
        engine.attachment_list("from").await.unwrap().len(),
        1,
        "the claim that stayed behind still owns the file"
    );
    assert_eq!(
        std::fs::read(from.join("assets").join("shot.png")).unwrap(),
        PNG
    );
    assert_eq!(engine.attachment_list("into").await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_collision_on_a_path_at_the_length_cap_lands_on_a_valid_name() {
    let (_tmp, engine, _from, into, _scratch) = move_fixture().await;
    // Exactly at the 256 byte ceiling, spread over folders because a single
    // filename has its own (shorter) limit on every real filesystem:
    // `assets/` + 100 + `/` + 100 + `/` + 43 + `.png`. Appending `-2` would
    // push the suffixed path past the ceiling, so the name that lands has to
    // be shortened rather than refused, or the rewritten reference would point
    // at a path no write will ever accept.
    let folders = format!("{}/{}", "d".repeat(100), "e".repeat(100));
    let long_path = format!("assets/{folders}/{}.png", "a".repeat(43));
    assert_eq!(long_path.len(), 256);
    let expected = format!("assets/{folders}/{}-2.png", "a".repeat(41));
    assert_eq!(expected.len(), 256);

    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", &format!("![shot]({long_path})")),
        )
        .await
        .unwrap();
    engine
        .attachment_write("from", &long_path, PNG.to_vec())
        .await
        .unwrap();
    engine
        .attachment_write("into", &long_path, OTHER_PNG.to_vec())
        .await
        .unwrap();

    engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap();

    let landed = engine.attachment_list("into").await.unwrap();
    let paths: Vec<&str> = landed.iter().map(|row| row.path.as_str()).collect();
    assert!(
        paths.contains(&expected.as_str()),
        "the carried file needs a name that fits: {paths:?}"
    );
    assert_eq!(
        engine.attachment_read("into", &expected).await.unwrap().0,
        PNG
    );
    assert_eq!(
        std::fs::read(into.join(&long_path)).unwrap(),
        OTHER_PNG,
        "the destination's own file is untouched"
    );
    let text = moved_text(&into, "note.md");
    assert!(
        text.contains(&format!("![shot]({expected})")),
        "the reference follows the shortened name: {text}"
    );
}

/// A cross-domain move re-emits the engram at the destination, so the store's
/// content column must never stand in for an unreadable source file: a file
/// domain indexes the body only, and the fallback would land a
/// frontmatter-stripped engram in the destination domain.
#[tokio::test]
async fn a_cross_domain_move_with_an_unreadable_source_file_fails_loudly() {
    let (_tmp, engine, from, into, _scratch) = move_fixture().await;
    engine
        .restore_engram(
            "from",
            "note.md",
            &engram_source("Note", "note", "", "A rule about note."),
        )
        .await
        .unwrap();

    // Remove the markdown behind the index's back: the row still points at it,
    // and the store still holds the body-only text the file domain indexed.
    std::fs::remove_file(from.join("note.md")).unwrap();

    let err = engine
        .move_engram(&move_params("from", "note", "note.md", Some("into")))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unreadable"), "got: {msg}");
    assert!(msg.contains("resync"), "got: {msg}");
    // The file that could not be read, named outright: an operator reading the
    // refusal should not have to reconstruct the path from the domain root.
    assert!(
        msg.contains(&from.join("note.md").display().to_string()),
        "got: {msg}"
    );

    assert!(
        !into.join("note.md").exists(),
        "no file landed in the destination domain"
    );
    assert!(
        engine
            .read_engram(&crystalline_service::params::ReadParams {
                identifier: "note".to_string(),
                domain: Some("into".to_string()),
            })
            .await
            .is_err(),
        "and nothing was indexed there either"
    );
}

/// `delete_engram` with an `assets/` identifier deletes the attachment, on both
/// domain kinds, and records the sweep the change owes. This is the one write
/// the MCP attachment surface offers, so an agent can complete an orphaned
/// attachment finding after the user's yes without a second verb existing.
#[tokio::test]
async fn delete_engram_deletes_an_attachment_on_either_domain_kind() {
    let (_tmp, engine, root, scratch) = named_fixture("folded-eng", "folded-scratch").await;

    for (domain, path) in [
        ("folded-eng", "assets/deck.png"),
        ("folded-scratch", "assets/notes.txt"),
    ] {
        engine
            .attachment_write(domain, path, PNG.to_vec())
            .await
            .unwrap();
        crystalline_service::maintenance::record_run(&[domain.to_string()]);

        let v = engine
            .delete_engram(&crystalline_service::params::DeleteParams {
                identifier: path.to_string(),
                domain: domain.to_string(),
                expected_checksum: None,
            })
            .await
            .unwrap();
        assert_eq!(v["attachment"], true, "{v}");
        assert_eq!(v["deleted"], true, "{v}");
        assert_eq!(v["path"], path, "{v}");
        assert!(v.get("permalink").is_none(), "no engram was involved: {v}");

        assert!(
            engine.attachment_read(domain, path).await.is_err(),
            "the bytes are gone from {domain}"
        );
        assert!(
            crystalline_service::maintenance::load()
                .pending_domains
                .contains(&domain.to_string()),
            "deleting through the folded verb owes the human a sweep too"
        );
    }

    // The file itself, not only its row.
    assert!(!root.join("assets/deck.png").exists());
    assert!(scratch.maintenance_path().exists());

    // A path nothing holds is a 404's worth of not-found, not a silent success.
    let err = engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "assets/never-there.png".to_string(),
            domain: "folded-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotFound(_)), "{err}");
}

/// An attachment delete refuses `expected_checksum` rather than ignoring it: it
/// is a promise about markdown a caller read, and accepting it silently would
/// let a caller believe a delete was guarded when nothing compared anything.
#[tokio::test]
async fn an_attachment_delete_refuses_an_expected_checksum() {
    let (_tmp, engine, root, _scratch) = named_fixture("guard-eng", "guard-scratch").await;
    engine
        .attachment_write("guard-eng", "assets/deck.png", PNG.to_vec())
        .await
        .unwrap();

    let err = engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "assets/deck.png".to_string(),
            domain: "guard-eng".to_string(),
            expected_checksum: Some("0".repeat(64)),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    assert!(err.to_string().contains("expected_checksum"), "{err}");
    assert!(
        root.join("assets/deck.png").exists(),
        "a refused delete deletes nothing"
    );

    // The reserved folder decides, whatever its spelling and whatever leads it.
    engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "./Assets/deck.png".to_string(),
            domain: "guard-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    assert!(!root.join("assets/deck.png").exists());
}

// --- the delete preview -----------------------------------------------------
//
// `Engine::delete_preview` answers "what would this delete take with it"
// without taking any of it, which is what the MCP confirmation round asks
// before it acts. Its attachment list is the part with real logic in it: the
// same referent count the cross-domain move uses, screened against what the
// domain actually holds.

/// The preview names the attachments this engram is the last referent of, and
/// nothing else: not one another engram still uses, and not one nothing holds.
#[tokio::test]
async fn a_delete_preview_names_only_the_attachments_this_engram_is_the_last_referent_of() {
    let (_tmp, engine, root, _scratch) = named_fixture("preview-eng", "preview-scratch").await;
    engine
        .restore_engram(
            "preview-eng",
            "note.md",
            &engram_source(
                "Note",
                "note",
                "",
                "![solo](assets/solo.png) ![both](assets/both.png) ![gone](assets/gone.png)",
            ),
        )
        .await
        .unwrap();
    engine
        .restore_engram(
            "preview-eng",
            "peer.md",
            &engram_source("Peer", "peer", "", "![both](assets/both.png)"),
        )
        .await
        .unwrap();
    for path in ["assets/solo.png", "assets/both.png"] {
        engine
            .attachment_write("preview-eng", path, PNG.to_vec())
            .await
            .unwrap();
    }

    let preview = engine
        .delete_preview(&crystalline_service::params::DeleteParams {
            identifier: "note".to_string(),
            domain: "preview-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();

    assert_eq!(preview["domain"], "preview-eng", "{preview}");
    assert_eq!(preview["permalink"], "note", "{preview}");
    assert_eq!(preview["title"], "Note", "{preview}");
    assert_eq!(preview["path"], "note.md", "{preview}");
    assert!(
        preview["attachments"].is_array(),
        "a domain this far inside MAX_PREVIEW_SCAN_ENGRAMS is enumerated, so the \
         field is a list rather than the null that says nobody looked: {preview}"
    );
    assert_eq!(
        preview["attachments"],
        serde_json::json!(["assets/solo.png"]),
        "one referent left for solo, two for both, and gone is not stored: {preview}"
    );

    // A preview previews. Everything it named is still exactly where it was.
    assert!(root.join("note.md").exists());
    assert_eq!(
        engine.attachment_list("preview-eng").await.unwrap().len(),
        2,
        "no attachment was touched"
    );
}

/// The `assets/` branch previews the attachment itself, refuses the checksum
/// the delete refuses, and reports a miss as a miss rather than asking the
/// user to confirm deleting nothing.
#[tokio::test]
async fn a_delete_preview_of_an_attachment_reports_its_size() {
    let (_tmp, engine, _root, _scratch) = named_fixture("size-eng", "size-scratch").await;
    engine
        .attachment_write("size-eng", "assets/deck.png", PNG.to_vec())
        .await
        .unwrap();

    let preview = engine
        .delete_preview(&crystalline_service::params::DeleteParams {
            identifier: "assets/deck.png".to_string(),
            domain: "size-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    assert_eq!(preview["attachment"], true, "{preview}");
    assert_eq!(preview["path"], "assets/deck.png", "{preview}");
    assert_eq!(preview["size"], PNG.len(), "{preview}");
    assert!(
        preview.get("permalink").is_none(),
        "no engram was involved: {preview}"
    );

    let refused = engine
        .delete_preview(&crystalline_service::params::DeleteParams {
            identifier: "assets/deck.png".to_string(),
            domain: "size-eng".to_string(),
            expected_checksum: Some("0".repeat(64)),
        })
        .await
        .unwrap_err();
    assert!(matches!(refused, EngineError::Invalid(_)), "{refused}");

    let missing = engine
        .delete_preview(&crystalline_service::params::DeleteParams {
            identifier: "assets/never-there.png".to_string(),
            domain: "size-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(missing, EngineError::NotFound(_)), "{missing}");

    assert!(
        engine
            .attachment_read("size-eng", "assets/deck.png")
            .await
            .is_ok(),
        "a preview deletes nothing"
    );
}

/// **A preview must never be stricter than the act it previews.**
///
/// `attachment_delete` reads no bytes and succeeds when either half of the
/// pair is there, so the preview has to succeed everywhere the delete would.
/// The half-present case is the reachable one on a file domain: a
/// hand-edited domain (or a `git pull` that dropped a file) leaves the row
/// standing over nothing, and the delete still removes the row. Built on the
/// byte read instead, round one would refuse and an eliciting peer would lose
/// a delete a legacy peer still gets.
#[tokio::test]
async fn a_delete_preview_is_never_stricter_than_the_delete_it_previews() {
    let (_tmp, engine, root, _scratch) = named_fixture("strict-eng", "strict-scratch").await;
    engine
        .attachment_write("strict-eng", "assets/deck.png", PNG.to_vec())
        .await
        .unwrap();
    // The file goes, the row stays: exactly what the delete's own doc comment
    // describes as still counting as a delete.
    std::fs::remove_file(root.join("assets/deck.png")).unwrap();
    assert!(
        engine
            .attachment_read("strict-eng", "assets/deck.png")
            .await
            .is_err(),
        "the read this preview used to be built on refuses here"
    );

    let preview = engine
        .delete_preview(&crystalline_service::params::DeleteParams {
            identifier: "assets/deck.png".to_string(),
            domain: "strict-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    assert_eq!(preview["attachment"], true, "{preview}");
    assert_eq!(
        preview["size"],
        PNG.len(),
        "the standing row is where the number comes from: {preview}"
    );

    // And the delete the preview promised still goes through.
    engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "assets/deck.png".to_string(),
            domain: "strict-eng".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    assert!(
        engine
            .attachment_list("strict-eng")
            .await
            .unwrap()
            .is_empty(),
        "the row went with it"
    );
}
