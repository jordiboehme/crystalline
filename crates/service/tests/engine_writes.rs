//! Engine-level tests for the Group A write verbs: full-document save, guided
//! retirement, checksum-guarded delete and manifest save. Endpoint behavior
//! (status codes, ETags) lives in rest_write_api.rs; this file pins the verbs.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{
    DeleteParams, ReadParams, RetireParams, SaveParams, SplitParams,
};
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
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
            },
            &Scope::Unrestricted,
        )
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

/// **The permalink-collision message is an interface, not just prose.**
///
/// `write_engram`'s MCP handler (`crates/service/src/mcp.rs`, the
/// `COLLISION_MARKER` interception) recognizes this one failure by the marker
/// `already exists in domain` in the error's display, and reads the colliding
/// permalink back out of `permalink '<permalink>'` to word the
/// overwrite-or-cancel question an eliciting 2026-07-28 peer is offered.
/// Rewording the message would disarm that round silently - the write would
/// simply go back to erroring - so both halves are pinned here, where a
/// rewording breaks loudly beside the sentence being reworded.
#[tokio::test]
async fn a_permalink_collision_carries_the_marker_the_mcp_layer_intercepts() {
    let (_tmp, engine) = engine_fixture().await;

    // `alpha.md` is in the fixture, so a second Alpha collides with it.
    let err = engine
        .write_engram(&crystalline_service::params::WriteParams {
            domain: "eng".to_string(),
            title: "Alpha".to_string(),
            content: "A second rule about alpha.".to_string(),
            folder: None,
            engram_type: None,
            tags: vec![],
            status: None,
            metadata: None,
            overwrite: false,
        })
        .await
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("already exists in domain"),
        "the marker mcp.rs intercepts on: {message}"
    );
    assert!(
        message.contains("permalink 'alpha'"),
        "the permalink, quoted where mcp.rs reads it out: {message}"
    );
    assert!(
        message.contains("pass overwrite=true to replace"),
        "and the hint every non-eliciting peer still gets: {message}"
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
    assert!(alpha.contains("- superseded_by [[beta]]"), "{alpha}");
    let beta = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert!(beta.contains("- supersedes [[alpha]]"), "{beta}");
}

/// Issue #65: an engram whose title's own first word ends in a colon.
///
/// `[[Log: Weekly Garden Notes]]` splits like `[[domain:Target]]` - the parser
/// is domain-agnostic and cannot tell the two apart - so a relation written by
/// title pointed at a domain called `Log` that nobody has, and the pair the
/// verb exists to wire came out broken. Every relation the engine writes for
/// itself names the permalink instead: it is the stable identity, and it never
/// carries a colon.
#[tokio::test]
async fn the_supersede_pair_links_by_permalink_so_a_colon_in_a_title_cannot_break_it() {
    let (tmp, engine) = engine_fixture().await;
    std::fs::write(
        tmp.path().join("eng/log-weekly.md"),
        "---\ntype: engram\ntitle: 'Log: Weekly Garden Notes'\npermalink: log-weekly\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-02-01\n---\n\n# Log: Weekly Garden Notes\n\nWhat the garden did this week.\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();

    engine
        .retire_engram(&RetireParams {
            domain: "eng".to_string(),
            identifier: "alpha".to_string(),
            status: "superseded".to_string(),
            successor: Some("log-weekly".to_string()),
            valid_to: None,
        })
        .await
        .unwrap();

    let alpha = std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(alpha.contains("- superseded_by [[log-weekly]]"), "{alpha}");
    let successor = std::fs::read_to_string(tmp.path().join("eng/log-weekly.md")).unwrap();
    assert!(successor.contains("- supersedes [[alpha]]"), "{successor}");

    // Both halves resolve, which is the whole point: the title form did not.
    for (identifier, rel_type) in [("alpha", "superseded_by"), ("log-weekly", "supersedes")] {
        let read = engine
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some("eng".to_string()),
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap();
        let relation = read["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["rel_type"] == rel_type)
            .unwrap_or_else(|| panic!("{identifier} declares {rel_type}"));
        assert_eq!(relation["resolved"], true, "{identifier} {rel_type}");
    }
}

/// The same rule on the other pair-writing verb, and the sharper case: the
/// engram carrying the colon is the SOURCE, so both bullets are affected.
#[tokio::test]
async fn split_links_by_permalink_so_a_colon_in_a_title_cannot_break_the_pair() {
    let (tmp, engine) = engine_fixture().await;
    std::fs::write(
        tmp.path().join("eng/log-weekly.md"),
        "---\ntype: engram\ntitle: 'Log: Weekly Garden Notes'\npermalink: log-weekly\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-02-01\n---\n\n# Log: Weekly Garden Notes\n\nWhat the garden did this week.\n\n- [fact] The beans went in on Tuesday\n- [fact] The compost bin was turned\n- [decision] The tomatoes stay under glass\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    let (checksum, _) = checksum_of(&engine, "eng", "log-weekly").await;
    let lines = observation_lines(&engine, "log-weekly", &["beans went in"]).await;

    engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "log-weekly".to_string(),
            title: "Sowing: What Went In".to_string(),
            folder: None,
            observations: lines,
            sections: Vec::new(),
            expected_checksum: Some(checksum),
        })
        .await
        .unwrap();

    let new = std::fs::read_to_string(tmp.path().join("eng/sowing-what-went-in.md")).unwrap();
    assert!(new.contains("- derived_from [[log-weekly]]"), "{new}");
    let source = std::fs::read_to_string(tmp.path().join("eng/log-weekly.md")).unwrap();
    assert!(
        source.contains("- split_into [[sowing-what-went-in]]"),
        "{source}"
    );

    for (identifier, rel_type) in [
        ("log-weekly", "split_into"),
        ("sowing-what-went-in", "derived_from"),
    ] {
        let read = engine
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some("eng".to_string()),
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap();
        let relation = read["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["rel_type"] == rel_type)
            .unwrap_or_else(|| panic!("{identifier} declares {rel_type}"));
        assert_eq!(relation["resolved"], true, "{identifier} {rel_type}");
    }
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
        alpha.matches("- superseded_by [[beta]]").count(),
        1,
        "{alpha}"
    );
    let beta = std::fs::read_to_string(tmp.path().join("eng/beta.md")).unwrap();
    assert_eq!(beta.matches("- supersedes [[alpha]]").count(), 1, "{beta}");
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

/// The collab external-delete resolution: put the exact bytes back and
/// reindex, no CAS. The same parse gate as save: a restore that would strip
/// frontmatter is refused.
#[tokio::test]
async fn restore_puts_the_exact_bytes_back_and_reindexes() {
    let (tmp, engine) = engine_fixture().await;
    std::fs::remove_file(tmp.path().join("eng/alpha.md")).unwrap();
    engine.sync(None).await.unwrap();

    let receipt = engine
        .restore_engram("eng", "alpha.md", ALPHA)
        .await
        .unwrap();
    assert_eq!(receipt["permalink"], "alpha");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/alpha.md")).unwrap(),
        ALPHA
    );
    // Indexed again: the read path resolves it.
    let (_, content) = checksum_of(&engine, "eng", "alpha").await;
    assert_eq!(content, ALPHA);

    let refused = engine
        .restore_engram("eng", "alpha.md", "no frontmatter")
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("frontmatter"), "{refused}");
}

/// The path-addressed read a collab room needs before it restores: it has no
/// identifier left to resolve, and it must never write over bytes that came
/// back at that path under another name.
#[tokio::test]
async fn text_at_path_reports_what_is_there_and_nothing_when_it_is_gone() {
    let (tmp, engine) = engine_fixture().await;
    let found = engine
        .engram_text_at_path("eng", "alpha.md")
        .await
        .unwrap()
        .expect("alpha.md is on disk");
    assert_eq!(found.content, ALPHA);
    assert_eq!(found.permalink, "alpha");
    let (checksum, _) = checksum_of(&engine, "eng", "alpha").await;
    assert_eq!(found.checksum, checksum);

    // Renamed in its frontmatter by somebody else: the path still answers,
    // with the permalink the index holds for it now.
    let renamed = ALPHA
        .replace("permalink: alpha", "permalink: beta")
        .replace("title: Alpha", "title: Beta");
    std::fs::write(tmp.path().join("eng/alpha.md"), &renamed).unwrap();
    engine.sync(None).await.unwrap();
    let moved = engine
        .engram_text_at_path("eng", "alpha.md")
        .await
        .unwrap()
        .expect("the file is still there");
    assert_eq!(moved.content, renamed);
    assert_eq!(moved.permalink, "beta");

    std::fs::remove_file(tmp.path().join("eng/alpha.md")).unwrap();
    assert!(
        engine
            .engram_text_at_path("eng", "alpha.md")
            .await
            .unwrap()
            .is_none(),
        "nothing is there any more"
    );
    // A virtual domain answers from the store the same way.
    assert!(
        engine
            .engram_text_at_path("scratch", "ghost.md")
            .await
            .unwrap()
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// split_engram
// ---------------------------------------------------------------------------

/// A five-observation bundle that mixes lifecycles: two facts about the purge
/// outlive the mix decision they were written beside. The shape `split_engram`
/// exists for.
const BUNDLE: &str = "---\ntype: decision\ntitle: Coolant Bundle\npermalink: coolant-bundle\ntags:\n  - coolant\n  - cooling\nstatus: stable\nrecorded_at: 2026-01-01\nvalid_to: 2026-08-01\n---\n\n# Coolant Bundle\n\n## Observations\n\n- [decision] Run the coolant loop on glycol mix B\n- [fact] The loop needs a 40 minute purge before a mix swap\n- [fact] The purge pump is rated for 12 bar\n- [gotcha] Mix B runs hot above 80 percent load\n- [convention] Log every mix swap in the ship register\n\n## Notes\n\nMix B was chosen when the fleet still ran the old pumps.\n";

/// The bundle on disk in `eng`, synced, with the checksum of what was written.
async fn bundle_fixture() -> (tempfile::TempDir, Arc<Engine>, String) {
    let (tmp, engine) = engine_fixture().await;
    std::fs::write(tmp.path().join("eng/coolant-bundle.md"), BUNDLE).unwrap();
    engine.sync(None).await.unwrap();
    let (checksum, _) = checksum_of(&engine, "eng", "coolant-bundle").await;
    (tmp, engine, checksum)
}

/// The one-based lines of the observations whose text contains `needle`, read
/// back exactly the way `read_engram` reports them to a caller.
async fn observation_lines(engine: &Engine, identifier: &str, needles: &[&str]) -> Vec<usize> {
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some("eng".to_string()),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let observations = read["observations"].as_array().unwrap().clone();
    needles
        .iter()
        .map(|needle| {
            observations
                .iter()
                .find(|o| o["content"].as_str().unwrap_or_default().contains(needle))
                .unwrap_or_else(|| panic!("no observation mentions {needle}"))["line"]
                .as_u64()
                .unwrap() as usize
        })
        .collect()
}

#[tokio::test]
async fn split_moves_the_selected_observations_and_wires_the_pair_both_ways() {
    let (tmp, engine, checksum) = bundle_fixture().await;
    let lines = observation_lines(&engine, "coolant-bundle", &["40 minute purge", "12 bar"]).await;

    let receipt = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Purge Procedure".to_string(),
            folder: None,
            observations: lines.clone(),
            sections: Vec::new(),
            expected_checksum: Some(checksum),
        })
        .await
        .unwrap();
    assert_eq!(receipt["source"]["permalink"], "coolant-bundle");
    assert_eq!(receipt["new"]["permalink"], "purge-procedure");
    assert_eq!(receipt["new"]["title"], "Purge Procedure");
    assert_eq!(receipt["moved_observations"], 2);
    assert_eq!(receipt["moved_sections"], 0);

    // The new engram, re-read from disk: the moved bullets verbatim, the
    // source's tags, a stable status, no window it inherited from the bundle.
    let new = std::fs::read_to_string(tmp.path().join("eng/purge-procedure.md")).unwrap();
    assert!(new.contains("- [fact] The loop needs a 40 minute purge before a mix swap"));
    assert!(new.contains("- [fact] The purge pump is rated for 12 bar"));
    assert!(new.contains("- derived_from [[coolant-bundle]]"));
    assert!(new.contains("status: stable"), "{new}");
    assert!(
        new.contains("- coolant") && new.contains("- cooling"),
        "{new}"
    );
    assert!(
        !new.contains("valid_to"),
        "the moved facts carry no window: {new}"
    );
    assert!(
        !new.contains("glycol mix B"),
        "only the selection moved: {new}"
    );

    // The source keeps what was not selected and gains the back-link.
    let source = std::fs::read_to_string(tmp.path().join("eng/coolant-bundle.md")).unwrap();
    assert!(!source.contains("40 minute purge"), "{source}");
    assert!(!source.contains("12 bar"), "{source}");
    assert!(source.contains("- [decision] Run the coolant loop on glycol mix B"));
    assert!(source.contains("- [convention] Log every mix swap in the ship register"));
    assert!(
        source.contains("- split_into [[purge-procedure]]"),
        "{source}"
    );

    // Both halves resolve: each engram's relation points at an engram that is
    // really there, which is what keeps V103 quiet about the pair.
    for (identifier, rel_type) in [
        ("coolant-bundle", "split_into"),
        ("purge-procedure", "derived_from"),
    ] {
        let read = engine
            .read_engram(
                &ReadParams {
                    identifier: identifier.to_string(),
                    domain: Some("eng".to_string()),
                },
                &Scope::Unrestricted,
            )
            .await
            .unwrap();
        let relation = read["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["rel_type"] == rel_type)
            .unwrap_or_else(|| panic!("{identifier} declares {rel_type}"));
        assert_eq!(relation["resolved"], true, "{identifier} {rel_type}");
    }
}

#[tokio::test]
async fn split_refuses_a_stale_checksum_and_writes_nothing() {
    let (tmp, engine, _) = bundle_fixture().await;
    let lines = observation_lines(&engine, "coolant-bundle", &["12 bar"]).await;

    let err = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Purge Procedure".to_string(),
            folder: None,
            observations: lines,
            sections: Vec::new(),
            expected_checksum: Some("deadbeef".to_string()),
        })
        .await
        .expect_err("a stale checksum is refused");
    assert!(format!("{err}").contains("stale"), "{err}");

    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/coolant-bundle.md")).unwrap(),
        BUNDLE,
        "the source is byte-identical"
    );
    assert!(
        !tmp.path().join("eng/purge-procedure.md").exists(),
        "nothing was created"
    );
}

#[tokio::test]
async fn split_moves_a_section_by_heading_path() {
    let (tmp, engine, _) = bundle_fixture().await;
    let receipt = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Mix B Background".to_string(),
            folder: Some("history".to_string()),
            observations: Vec::new(),
            sections: vec!["## Notes".to_string()],
            expected_checksum: None,
        })
        .await
        .unwrap();
    assert_eq!(receipt["moved_sections"], 1);
    assert_eq!(receipt["new"]["path"], "history/mix-b-background.md");

    let new = std::fs::read_to_string(tmp.path().join("eng/history/mix-b-background.md")).unwrap();
    assert!(new.contains("## Notes"), "{new}");
    assert!(new.contains("Mix B was chosen when the fleet still ran the old pumps."));
    let source = std::fs::read_to_string(tmp.path().join("eng/coolant-bundle.md")).unwrap();
    assert!(!source.contains("## Notes"), "{source}");
    assert!(source.contains("- [decision] Run the coolant loop on glycol mix B"));
}

#[tokio::test]
async fn split_refuses_a_selection_that_would_leave_the_source_a_stub() {
    let (tmp, engine, _) = bundle_fixture().await;
    let lines = observation_lines(
        &engine,
        "coolant-bundle",
        &[
            "glycol mix B",
            "40 minute purge",
            "12 bar",
            "80 percent load",
            "ship register",
        ],
    )
    .await;

    let err = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Everything".to_string(),
            folder: None,
            observations: lines,
            sections: vec!["## Notes".to_string()],
            expected_checksum: None,
        })
        .await
        .expect_err("a split that empties the source is refused");
    let message = format!("{err}");
    assert!(message.contains("retire"), "the fix is named: {message}");
    assert!(
        !tmp.path().join("eng/everything.md").exists(),
        "nothing was created"
    );
}

#[tokio::test]
async fn split_refuses_an_empty_selection() {
    let (_tmp, engine, _) = bundle_fixture().await;
    let err = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Nothing".to_string(),
            folder: None,
            observations: Vec::new(),
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .expect_err("a split with nothing selected is refused");
    assert!(format!("{err}").contains("observations"), "{err}");
}

#[tokio::test]
async fn split_works_on_a_virtual_domain_too() {
    let (_tmp, engine, _) = bundle_fixture().await;
    engine
        .write_engram(&crystalline_service::params::WriteParams {
            domain: "scratch".to_string(),
            title: "Scratch Bundle".to_string(),
            content: "# Scratch Bundle\n\n- [fact] The gate closes at 22:00\n- [fact] The night crew logs the closing\n- [fact] The register lives in the wardroom\n".to_string(),
            folder: None,
            engram_type: None,
            tags: vec!["ops".to_string()],
            status: None,
            metadata: None,
            overwrite: false,
        })
        .await
        .unwrap();
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "scratch-bundle".to_string(),
                domain: Some("scratch".to_string()),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let line = read["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["content"].as_str().unwrap().contains("wardroom"))
        .unwrap()["line"]
        .as_u64()
        .unwrap() as usize;

    let receipt = engine
        .split_engram(&SplitParams {
            domain: "scratch".to_string(),
            identifier: "scratch-bundle".to_string(),
            title: "Register Location".to_string(),
            folder: None,
            observations: vec![line],
            sections: Vec::new(),
            expected_checksum: Some(read["checksum"].as_str().unwrap().to_string()),
        })
        .await
        .unwrap();
    assert_eq!(receipt["moved_observations"], 1);

    let new = engine
        .engram_text("scratch", "register-location")
        .await
        .unwrap();
    assert!(new.content.contains("The register lives in the wardroom"));
    assert!(new.content.contains("- derived_from [[scratch-bundle]]"));
    let source = engine
        .engram_text("scratch", "scratch-bundle")
        .await
        .unwrap();
    assert!(!source.content.contains("wardroom"), "{}", source.content);
    assert!(
        source
            .content
            .contains("- split_into [[register-location]]")
    );
}

/// The rollback: the new engram lands, the source edit fails, and the new
/// engram is taken back out so the archive is exactly as it was.
///
/// The failure is made deterministic without a race: every engram write goes to
/// a sibling temp file and is renamed into place, so taking write permission
/// off the domain's root directory - while leaving the `history/` subfolder the
/// new engram is filed in writable - lets the whole plan, the new engram and
/// its reindex through and stops exactly one thing, the write back to the
/// source. Unix only, since that is where a directory mode refuses a write to
/// the user who owns it.
#[cfg(unix)]
#[tokio::test]
async fn split_takes_the_new_engram_back_out_when_the_source_edit_fails() {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, engine, _) = bundle_fixture().await;
    let lines = observation_lines(&engine, "coolant-bundle", &["12 bar"]).await;
    let root = tmp.path().join("eng");
    std::fs::create_dir_all(root.join("history")).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();

    let err = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Purge Procedure".to_string(),
            folder: Some("history".to_string()),
            observations: lines,
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .expect_err("the source cannot be written");

    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("coolant-bundle.md")).unwrap(),
        BUNDLE,
        "the source never changed: {err}"
    );
    assert!(
        !root.join("history/purge-procedure.md").exists(),
        "the new engram was taken back out"
    );
    assert!(
        engine.engram_text("eng", "purge-procedure").await.is_err(),
        "and its index rows went with it"
    );
}

#[tokio::test]
async fn split_counts_a_line_named_twice_once() {
    let (_tmp, engine, _) = bundle_fixture().await;
    let lines = observation_lines(&engine, "coolant-bundle", &["12 bar"]).await;
    let receipt = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Purge Pump Rating".to_string(),
            folder: None,
            observations: vec![lines[0], lines[0]],
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    assert_eq!(
        receipt["moved_observations"], 1,
        "the receipt counts what moved"
    );
}

/// The invariant the rollback must not break: once the source's bytes have been
/// replaced, the new engram stays, whatever fails next.
///
/// The source no longer holds the moved bullets at that point, so deleting the
/// engram that does hold them is the one outcome the verb must never produce.
/// The failure is armed through `Engine::fail_next_source_edit`, the engine's
/// one test seam, because the window it stands in for - a store or IO fault
/// after an atomic rename - is not reachable from a test any other way.
#[tokio::test]
async fn split_keeps_both_files_when_the_reindex_fails_after_the_source_was_written() {
    let (tmp, engine, _) = bundle_fixture().await;
    let lines = observation_lines(&engine, "coolant-bundle", &["40 minute purge", "12 bar"]).await;
    engine.fail_next_source_edit();

    let err = engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Purge Procedure".to_string(),
            folder: None,
            observations: lines,
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .expect_err("the reindex failed after the write");

    // The error names both files and says what is stale.
    let message = format!("{err}");
    assert!(message.contains("coolant-bundle.md"), "{message}");
    assert!(message.contains("purge-procedure.md"), "{message}");
    assert!(message.contains("sync"), "{message}");

    // Both files are on disk, and between them they hold every bullet: the
    // moved ones in the new engram, the rest in the source.
    let new = std::fs::read_to_string(tmp.path().join("eng/purge-procedure.md"))
        .expect("the new engram was NOT taken back out");
    assert!(new.contains("- [fact] The loop needs a 40 minute purge before a mix swap"));
    assert!(new.contains("- [fact] The purge pump is rated for 12 bar"));
    let source = std::fs::read_to_string(tmp.path().join("eng/coolant-bundle.md")).unwrap();
    assert!(source.contains("- [decision] Run the coolant loop on glycol mix B"));
    assert!(
        source.contains("- split_into [[purge-procedure]]"),
        "{source}"
    );
    assert!(!source.contains("40 minute purge"), "{source}");

    // And the index catches up on the next sync, with nothing lost.
    engine.sync(None).await.unwrap();
    let reread = engine.engram_text("eng", "coolant-bundle").await.unwrap();
    assert_eq!(reread.content, source);
    assert!(
        engine.engram_text("eng", "purge-procedure").await.is_ok(),
        "the new engram is indexed too"
    );
}

/// The seam is one-shot, so an ordinary split right after an armed one behaves
/// exactly as it always does. Without this the seam could latch and silently
/// change every later write in a process.
#[tokio::test]
async fn the_reindex_seam_fires_once() {
    let (_tmp, engine, _) = bundle_fixture().await;
    let lines = observation_lines(&engine, "coolant-bundle", &["12 bar"]).await;
    engine.fail_next_source_edit();
    engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Purge Pump Rating".to_string(),
            folder: None,
            observations: lines,
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .expect_err("armed");
    engine.sync(None).await.unwrap();

    let lines = observation_lines(&engine, "coolant-bundle", &["80 percent load"]).await;
    engine
        .split_engram(&SplitParams {
            domain: "eng".to_string(),
            identifier: "coolant-bundle".to_string(),
            title: "Mix B Heat Margin".to_string(),
            folder: None,
            observations: lines,
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .expect("the seam is spent");
}

/// The other half of the same invariant, on the other storage kind: a virtual
/// source's edit is one store transaction, so a compare-and-swap conflict
/// leaves the stored bytes exactly as they were and the new engram must be
/// taken back out again.
///
/// The seam arms a token nothing can match, so the conflict is the store's own
/// rather than a fabricated error: `upsert_engram_checked` refuses, the
/// transaction rolls back, and what the caller sees is the `Conflict` a
/// concurrent edit really produces.
#[tokio::test]
async fn a_virtual_split_that_loses_the_compare_and_swap_is_a_conflict_with_no_orphan() {
    let (_tmp, engine) = engine_fixture().await;
    engine
        .write_engram(&crystalline_service::params::WriteParams {
            domain: "scratch".to_string(),
            title: "Scratch Bundle".to_string(),
            content: "# Scratch Bundle\n\n- [fact] The gate closes at 22:00\n- [fact] The night crew logs the closing\n- [fact] The register lives in the wardroom\n".to_string(),
            folder: None,
            engram_type: None,
            tags: vec!["ops".to_string()],
            status: None,
            metadata: None,
            overwrite: false,
        })
        .await
        .unwrap();
    let before = engine
        .engram_text("scratch", "scratch-bundle")
        .await
        .unwrap();
    let line = engine
        .read_engram(
            &ReadParams {
                identifier: "scratch-bundle".to_string(),
                domain: Some("scratch".to_string()),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["content"].as_str().unwrap().contains("wardroom"))
        .unwrap()["line"]
        .as_u64()
        .unwrap() as usize;

    engine.fail_next_source_edit();
    let err = engine
        .split_engram(&SplitParams {
            domain: "scratch".to_string(),
            identifier: "scratch-bundle".to_string(),
            title: "Register Location".to_string(),
            folder: None,
            observations: vec![line],
            sections: Vec::new(),
            expected_checksum: None,
        })
        .await
        .expect_err("the compare and swap refused");

    // The conflict the store raised, not an internal fault, so the caller knows
    // to re-read and retry rather than to call somebody.
    let message = format!("{err}");
    assert!(message.contains("stale edit"), "{message}");
    assert!(!message.contains("neither was undone"), "{message}");

    // The source is byte-identical and the new engram is gone: the rollback
    // still fires on this side of the write.
    let after = engine
        .engram_text("scratch", "scratch-bundle")
        .await
        .unwrap();
    assert_eq!(after.content, before.content);
    assert!(
        engine
            .engram_text("scratch", "register-location")
            .await
            .is_err(),
        "the new engram was taken back out"
    );
}
