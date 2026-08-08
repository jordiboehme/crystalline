//! Engine-level tests for the Group A write verbs: full-document save, guided
//! retirement, checksum-guarded delete and manifest save. Endpoint behavior
//! (status codes, ETags) lives in rest_write_api.rs; this file pins the verbs.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::{ReadParams, SaveParams};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// A file domain `eng` holding MANIFEST + alpha, synced into an in-memory
/// store: the same construction rest_api.rs uses, trimmed to what write tests
/// need.
async fn engine_fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
    )
    .unwrap();
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
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
    (tmp, engine)
}

/// The checksum a read reports, which is the save's CAS token.
async fn checksum_of(engine: &Engine, domain: &str, identifier: &str) -> (String, String) {
    let read = engine
        .read_engram(&ReadParams {
            identifier: identifier.to_string(),
            domain: Some(domain.to_string()),
        })
        .await
        .unwrap();
    (
        read["checksum"].as_str().unwrap().to_string(),
        read["content"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn save_writes_the_exact_bytes_and_moves_the_checksum() {
    let (tmp, engine) = engine_fixture().await;
    let (checksum, content) = checksum_of(&engine, "eng", "alpha").await;

    let edited = content.replace("A rule about alpha.", "A sharper rule about alpha.");
    let saved = engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content: edited.clone(),
            expected_checksum: checksum,
        })
        .await
        .unwrap();
    assert_eq!(saved["permalink"], "alpha");

    // Files are truth: the exact bytes landed, nothing was reserialized.
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(on_disk, edited);
    // And the read now reports the new checksum, which is the saved one.
    let (after, _) = checksum_of(&engine, "eng", "alpha").await;
    assert_eq!(saved["checksum"].as_str().unwrap(), after);
}

/// The fidelity property at the engine layer: saving what was read, unedited,
/// is byte-identical on disk.
#[tokio::test]
async fn a_zero_edit_save_is_byte_identical() {
    let (tmp, engine) = engine_fixture().await;
    let before = std::fs::read(tmp.path().join("eng/alpha.md")).unwrap();
    let (checksum, content) = checksum_of(&engine, "eng", "alpha").await;
    engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content,
            expected_checksum: checksum,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(tmp.path().join("eng/alpha.md")).unwrap(),
        before
    );
}

/// A stale token is refused on a FILE domain too - this is what the spec's
/// "the engine's expected_checksum path enforces it" adds over slice 1, where
/// file-domain edits ignored the token. The wording is pinned because the REST
/// layer keys its 412 on it.
#[tokio::test]
async fn a_stale_save_is_a_conflict_on_file_and_virtual_domains() {
    let (_tmp, engine) = engine_fixture().await;
    let err = engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content: ALPHA.replace("stable", "draft"),
            expected_checksum: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stale edit"), "{err}");

    // Virtual: seed an engram, then save with a stale token.
    engine
        .write_engram(&crystalline_service::params::WriteParams {
            domain: "scratch".to_string(),
            title: "Note".to_string(),
            content: "A note.".to_string(),
            folder: None,
            engram_type: None,
            tags: vec![],
            status: None,
            metadata: None,
            overwrite: false,
        })
        .await
        .unwrap();
    let (_, content) = checksum_of(&engine, "scratch", "note").await;
    let err = engine
        .save_engram(&SaveParams {
            domain: "scratch".to_string(),
            identifier: "note".to_string(),
            content,
            expected_checksum: "not-the-checksum".to_string(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stale edit"), "{err}");
}

/// The other half of the hard gate: frontmatter that is present but is not
/// parseable YAML. The engine refuses it for the same reason - the reindex
/// that follows the write would have to swallow the same failure - while a
/// document that merely violates a verify rule (a missing tag, an inverted
/// validity window) is left to the validation endpoint to report.
#[tokio::test]
async fn a_save_with_unparseable_frontmatter_is_refused_without_writing() {
    let (tmp, engine) = engine_fixture().await;
    let before = std::fs::read(tmp.path().join("eng/alpha.md")).unwrap();
    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    let err = engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content: "---\ntitle: [unclosed\n---\n\n# Alpha\n".to_string(),
            expected_checksum: checksum,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Invalid(_)),
        "{err}"
    );
    assert_eq!(
        std::fs::read(tmp.path().join("eng/alpha.md")).unwrap(),
        before
    );
}

/// An empty frontmatter block is the same identity strip as a missing one,
/// wearing delimiters: `type`, `title`, `permalink`, `tags` and `status` are
/// all gone and the index falls back to the path slug. The gate refuses it for
/// the same reason it refuses a document with no block at all.
#[tokio::test]
async fn a_save_with_an_empty_frontmatter_block_is_refused_without_writing() {
    let (tmp, engine) = engine_fixture().await;
    let before = std::fs::read(tmp.path().join("eng/alpha.md")).unwrap();
    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    let err = engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content: "---\n---\n\n# Alpha\n\nA rule about alpha.\n".to_string(),
            expected_checksum: checksum,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Invalid(_)),
        "{err}"
    );
    assert_eq!(
        std::fs::read(tmp.path().join("eng/alpha.md")).unwrap(),
        before
    );
}

/// A document missing a tag is an E-family verify finding, not an unsavable
/// document: refusing it would make an engram that already carries the flaw
/// uneditable through the very editor meant to fix it.
#[tokio::test]
async fn a_save_that_only_violates_a_verify_rule_still_lands() {
    let (tmp, engine) = engine_fixture().await;
    let (checksum, content) = checksum_of(&engine, "eng", "alpha").await;
    let untagged = content.replace("tags:\n  - eng\n", "");
    engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content: untagged.clone(),
            expected_checksum: checksum,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        untagged
    );
}

#[tokio::test]
async fn a_save_that_does_not_parse_is_refused_without_writing() {
    let (tmp, engine) = engine_fixture().await;
    let before = std::fs::read(tmp.path().join("eng/alpha.md")).unwrap();
    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    let err = engine
        .save_engram(&SaveParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            content: "not an engram at all".to_string(),
            expected_checksum: checksum,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Invalid(_)),
        "{err}"
    );
    assert_eq!(
        std::fs::read(tmp.path().join("eng/alpha.md")).unwrap(),
        before
    );
}
