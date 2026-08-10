//! Engine-level tests for the Group A write verbs: full-document save, guided
//! retirement, checksum-guarded delete and manifest save. Endpoint behavior
//! (status codes, ETags) lives in rest_write_api.rs; this file pins the verbs.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::params::{DeleteParams, ReadParams, RetireParams, SaveParams};
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

/// The MCP promise made true on files: an edit that presents the checksum of
/// the version it read is refused once the content moved on, on BOTH storage
/// kinds. The file arm compares inside the same per-path lock the save takes,
/// and speaks through stale_edit_message so the REST layer's "stale edit"
/// seam holds. An edit with no checksum stays last-write-wins, unchanged.
#[tokio::test]
async fn a_stale_edit_is_a_conflict_on_file_and_virtual_domains() {
    let (_tmp, engine) = engine_fixture().await;

    // File domain: a stale token refuses, the honest token lands.
    let stale: crystalline_service::params::EditParams =
        serde_json::from_value(serde_json::json!({
            "identifier": "alpha",
            "domain": "eng",
            "operation": "append",
            "content": "A late thought.",
            "expected_checksum": "0000000000000000000000000000000000000000000000000000000000000000",
        }))
        .unwrap();
    let err = engine.edit_engram(&stale).await.unwrap_err();
    assert!(err.to_string().contains("stale edit"), "{err}");

    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    let fresh: crystalline_service::params::EditParams =
        serde_json::from_value(serde_json::json!({
            "identifier": "alpha",
            "domain": "eng",
            "operation": "append",
            "content": "A timely thought.",
            "expected_checksum": checksum,
        }))
        .unwrap();
    engine.edit_engram(&fresh).await.unwrap();

    // And with no token at all: last-write-wins, exactly as before.
    let unguarded: crystalline_service::params::EditParams =
        serde_json::from_value(serde_json::json!({
            "identifier": "alpha",
            "domain": "eng",
            "operation": "append",
            "content": "An unguarded thought.",
        }))
        .unwrap();
    engine.edit_engram(&unguarded).await.unwrap();

    // Virtual domain: the seam already held; pin it beside the file case.
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
    let stale_virtual: crystalline_service::params::EditParams =
        serde_json::from_value(serde_json::json!({
            "identifier": "note",
            "domain": "scratch",
            "operation": "append",
            "content": "A late thought.",
            "expected_checksum": "not-the-checksum",
        }))
        .unwrap();
    let err = engine.edit_engram(&stale_virtual).await.unwrap_err();
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

#[tokio::test]
async fn retirement_sets_status_and_wires_the_supersede_pair() {
    let (tmp, engine) = engine_fixture().await;
    // A successor beside alpha.
    std::fs::write(
        tmp.path().join("eng/beta.md"),
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-02-01\n---\n\n# Beta\n\nThe sharper rule.\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    let out = engine
        .retire_engram(&RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "superseded".to_string(),
            successor: Some("beta".to_string()),
            valid_to: Some("2026-08-01".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(out["status"], "superseded");
    assert_eq!(out["successor"], "beta");

    let alpha = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(alpha.contains("status: superseded"), "{alpha}");
    assert!(alpha.contains("valid_to: 2026-08-01"), "{alpha}");
    assert!(alpha.contains("- superseded_by [[Beta]]"), "{alpha}");
    let beta = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(beta.contains("- supersedes [[Alpha]]"), "{beta}");
}

#[tokio::test]
async fn retirement_validates_status_successor_and_date() {
    let (_tmp, engine) = engine_fixture().await;
    let retire = |status: &str, successor: Option<&str>, valid_to: Option<&str>| RetireParams {
        domain: "eng".to_string(),
        identifier: "alpha".to_string(),
        status: status.to_string(),
        successor: successor.map(str::to_string),
        valid_to: valid_to.map(str::to_string),
    };

    // Not a retirement status.
    assert!(
        engine
            .retire_engram(&retire("stable", None, None))
            .await
            .is_err()
    );
    // Superseded needs its successor; the others refuse one.
    assert!(
        engine
            .retire_engram(&retire("superseded", None, None))
            .await
            .is_err()
    );
    assert!(
        engine
            .retire_engram(&retire("deprecated", Some("beta"), None))
            .await
            .is_err()
    );
    // A bad date never lands, and neither does a missing successor.
    assert!(
        engine
            .retire_engram(&retire("archived", None, Some("soon")))
            .await
            .is_err()
    );
    assert!(
        engine
            .retire_engram(&retire("superseded", Some("ghost"), None))
            .await
            .is_err()
    );

    // Nothing was written by any refusal.
    let alpha = std::fs::read_to_string(_tmp.path().join("eng/alpha.md")).unwrap();
    assert!(alpha.contains("status: stable"), "{alpha}");
}

/// A successor that resolves to the target itself is refused rather than
/// appended as a supersedes-self relation: no deadlock (the target's lock is
/// released before a successor's would be taken), just a nonsense pair
/// nothing should ever produce.
#[tokio::test]
async fn a_self_referential_retirement_is_refused() {
    let (tmp, engine) = engine_fixture().await;
    let err = engine
        .retire_engram(&RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "superseded".to_string(),
            successor: Some("alpha".to_string()),
            valid_to: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::Invalid(_)),
        "{err}"
    );
    let alpha = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        alpha.contains("status: stable"),
        "the refusal must not have written anything: {alpha}"
    );
}

#[tokio::test]
async fn plain_deprecation_needs_no_successor() {
    let (tmp, engine) = engine_fixture().await;
    engine
        .retire_engram(&RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "deprecated".to_string(),
            successor: None,
            valid_to: None,
        })
        .await
        .unwrap();
    let alpha = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(alpha.contains("status: deprecated"), "{alpha}");
    assert!(
        !alpha.contains("valid_to"),
        "no sentinel dates, ever: {alpha}"
    );
}

#[tokio::test]
async fn retirement_strips_a_sentinel_valid_to_instead_of_writing_it() {
    let (tmp, engine) = engine_fixture().await;
    engine
        .retire_engram(&RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "archived".to_string(),
            successor: None,
            valid_to: Some("9999-12-31".to_string()),
        })
        .await
        .unwrap();
    let alpha = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(alpha.contains("status: archived"), "{alpha}");
    assert!(
        !alpha.contains("valid_to"),
        "a sentinel valid_to must be stripped, exactly as edit_engram's set_frontmatter \
         strips it, never written verbatim: {alpha}"
    );
}

#[tokio::test]
async fn retiring_the_same_engram_twice_is_idempotent() {
    let (tmp, engine) = engine_fixture().await;
    std::fs::write(
        tmp.path().join("eng/beta.md"),
        "---\ntype: engram\ntitle: Beta\npermalink: beta\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-02-01\n---\n\n# Beta\n\nThe sharper rule.\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    let params = RetireParams {
        domain: "eng".to_string(),
        identifier: "alpha".to_string(),
        status: "superseded".to_string(),
        successor: Some("beta".to_string()),
        valid_to: None,
    };
    engine.retire_engram(&params).await.unwrap();
    // A retry (after a timeout, say) must not duplicate the relation on
    // either side, and must still succeed rather than error.
    engine.retire_engram(&params).await.unwrap();

    let alpha = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert_eq!(
        alpha.matches("- superseded_by [[Beta]]").count(),
        1,
        "{alpha}"
    );
    let beta = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert_eq!(beta.matches("- supersedes [[Alpha]]").count(), 1, "{beta}");
}

#[tokio::test]
async fn a_guarded_delete_refuses_when_the_engram_moved_on() {
    let (tmp, engine) = engine_fixture().await;
    let err = engine
        .delete_engram(&DeleteParams {
            identifier: "alpha".to_string(),
            domain: "eng".to_string(),
            expected_checksum: Some("0".repeat(64)),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stale edit"), "{err}");
    assert!(
        tmp.path().join("eng/alpha.md").exists(),
        "nothing was deleted"
    );

    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    engine
        .delete_engram(&DeleteParams {
            identifier: "alpha".to_string(),
            domain: "eng".to_string(),
            expected_checksum: Some(checksum),
        })
        .await
        .unwrap();
    assert!(!tmp.path().join("eng/alpha.md").exists());
}

#[tokio::test]
async fn manifest_save_is_guarded_verbatim_and_refreshes_routing() {
    let (tmp, engine) = engine_fixture().await;
    let current = engine.manifest_markdown("eng").await.unwrap();
    let checksum = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(current.as_bytes());
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };

    // Stale token: refused, file untouched.
    let err = engine
        .save_manifest("eng", &current.replace("eng questions", "nothing"), "beef")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stale edit"), "{err}");

    // Fresh token: the exact bytes land.
    let edited = current.replace(
        "Route here for eng questions",
        "Route here for everything eng",
    );
    engine
        .save_manifest("eng", &edited, &checksum)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
        edited
    );

    // Unparseable markdown never lands.
    let (_, checksum2) = {
        let now = engine.manifest_markdown("eng").await.unwrap();
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(now.as_bytes());
        (
            now,
            h.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        )
    };
    assert!(
        engine
            .save_manifest("eng", "no frontmatter", &checksum2)
            .await
            .is_err()
    );

    // The routing refresh the name promises: `save_manifest` calls
    // `refresh_routing_cache` unconditionally, but on a file domain like
    // `eng`, `routing_text` reads `MANIFEST.md` straight off disk regardless
    // of that cache (see `save_manifest`'s own doc comment on why), so
    // nothing above actually exercises the refresh. `scratch` is virtual: its
    // bullets come only from the `routing_virtual` cache, so a save's effect
    // on `routing_text` is observable only if the cache genuinely refreshed.
    engine
        .scaffold_virtual_manifest(
            "scratch",
            "---\ntype: manifest\ntitle: scratch\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# scratch\n\n## Scope\n\n- Everything about scratch\n\n## When to Use\n\n- Route here for scratch questions\n",
        )
        .await
        .unwrap();
    let before_routing = engine.routing_text();
    assert!(
        before_routing.contains("Route here for scratch questions"),
        "{before_routing}"
    );

    let scratch_manifest = engine.manifest_markdown("scratch").await.unwrap();
    let scratch_checksum = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(scratch_manifest.as_bytes());
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let edited_scratch = scratch_manifest.replace(
        "Route here for scratch questions",
        "Route here for updated scratch topics",
    );
    engine
        .save_manifest("scratch", &edited_scratch, &scratch_checksum)
        .await
        .unwrap();

    let after_routing = engine.routing_text();
    assert!(
        after_routing.contains("Route here for updated scratch topics"),
        "the routing cache reflects the save:\n{after_routing}"
    );
    assert!(
        !after_routing.contains("Route here for scratch questions"),
        "the stale bullet is gone:\n{after_routing}"
    );
}

/// The collab layer's thin read: exact bytes, identity, checksum - no
/// reference resolution. The checksum is the same CAS token a save takes.
#[tokio::test]
async fn engram_text_reports_exact_bytes_and_the_save_token() {
    let (_tmp, engine) = engine_fixture().await;
    let text = engine.engram_text("eng", "alpha").await.unwrap();
    assert_eq!(text.domain, "eng");
    assert_eq!(text.permalink, "alpha");
    assert_eq!(text.path, "alpha.md");
    assert_eq!(text.content, ALPHA);
    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    assert_eq!(text.checksum, checksum);

    let missing = engine.engram_text("eng", "ghost").await.unwrap_err();
    assert!(missing.to_string().contains("ghost"), "{missing}");
}
