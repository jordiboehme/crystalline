//! Crystalline stores and returns LF only: a CRLF (or mixed) file is read,
//! indexed and searched, never rewritten by a read, and turns into LF as a
//! whole on the next write that lands on it. Every tool parameter may carry
//! CRLF. The checksum a read hands out is of the bytes the file holds, so a
//! guarded edit of a CRLF file is not refused.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::params::{EditParams, ReadParams, SaveParams, SearchParams, WriteParams};
use tokio::sync::Mutex;

use crate::support::sha256_hex;

const CRLF: &str = "---\r\ntype: engram\r\ntitle: Windows\r\npermalink: windows\r\ntags:\r\n  - eng\r\nstatus: stable\r\nrecorded_at: 2026-01-01\r\n---\r\n\r\n# Windows\r\n\r\n- [fact] written on a windows machine #crlf\r\n\r\nA paragraph about zebras.\r\n";
const MIXED: &str = "---\r\ntype: engram\r\ntitle: Mixed\r\npermalink: mixed\r\ntags:\r\n  - eng\r\nstatus: stable\r\n---\r\n\r\nA CRLF line\nwith a lone LF\r\n";

async fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
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
    std::fs::write(dir.join("windows.md"), CRLF).unwrap();
    std::fs::write(dir.join("mixed.md"), MIXED).unwrap();
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

async fn read(engine: &Engine, domain: &str, identifier: &str) -> serde_json::Value {
    engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap()
}

fn on_disk(tmp: &tempfile::TempDir, rel: &str) -> String {
    std::fs::read_to_string(tmp.path().join("eng").join(rel)).unwrap()
}

fn edit(identifier: &str, operation: &str) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        domain: "eng".to_string(),
        operation: operation.to_string(),
        ..Default::default()
    }
}

/// Every edit operation turns the whole file to LF, not only the part it
/// touched.
#[tokio::test]
async fn one_edit_of_a_crlf_engram_leaves_no_cr_anywhere() {
    let cases: Vec<(&str, EditParams)> = vec![
        (
            "append",
            EditParams {
                content: Some("appended line".to_string()),
                ..edit("windows", "append")
            },
        ),
        (
            "find_replace",
            EditParams {
                find_text: Some("zebras".to_string()),
                content: Some("lions".to_string()),
                ..edit("windows", "find_replace")
            },
        ),
        (
            "set_frontmatter tags",
            EditParams {
                key: Some("tags".to_string()),
                values: Some(vec!["eng".to_string(), "windows".to_string()]),
                ..edit("windows", "set_frontmatter")
            },
        ),
        (
            "set_frontmatter status",
            EditParams {
                key: Some("status".to_string()),
                value: Some("draft".to_string()),
                ..edit("windows", "set_frontmatter")
            },
        ),
    ];
    for (name, params) in cases {
        let (tmp, engine) = fixture().await;
        engine
            .edit_engram(&params)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = on_disk(&tmp, "windows.md");
        assert!(!text.contains('\r'), "{name} left a CR: {text:?}");
        assert!(text.contains("A paragraph about"), "{name}: {text:?}");
    }
}

#[tokio::test]
async fn an_edit_of_a_mixed_file_writes_it_as_lf() {
    let (tmp, engine) = fixture().await;
    engine
        .edit_engram(&EditParams {
            content: Some("more".to_string()),
            ..edit("mixed", "append")
        })
        .await
        .unwrap();
    let text = on_disk(&tmp, "mixed.md");
    assert!(!text.contains('\r'), "{text:?}");
    assert!(text.contains("A CRLF line\nwith a lone LF\n"), "{text:?}");
}

/// A find_text sent with CRLF matches the LF text it means, and the
/// replacement's own CRLF lands as LF.
#[tokio::test]
async fn a_crlf_find_text_matches_lf_text() {
    let (tmp, engine) = fixture().await;
    std::fs::write(
        tmp.path().join("eng/plain.md"),
        "---\ntype: engram\ntitle: Plain\npermalink: plain\ntags:\n  - eng\nstatus: stable\n---\n\nfirst line\nsecond line\n",
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    engine
        .edit_engram(&EditParams {
            find_text: Some("first line\r\nsecond line".to_string()),
            content: Some("one\r\ntwo".to_string()),
            expected_replacements: Some(1),
            ..edit("plain", "find_replace")
        })
        .await
        .unwrap();
    let text = on_disk(&tmp, "plain.md");
    assert!(text.ends_with("\n\none\ntwo\n"), "{text:?}");
    assert!(!text.contains('\r'), "{text:?}");
}

#[tokio::test]
async fn write_engram_content_with_crlf_is_stored_lf() {
    let (tmp, engine) = fixture().await;
    for domain in ["eng", "scratch"] {
        engine
            .write_engram(&WriteParams {
                domain: domain.to_string(),
                title: "Captured".to_string(),
                content: "line one\r\nline two\r\n\r\n- [fact] a fact\r\n".to_string(),
                folder: None,
                engram_type: None,
                tags: vec!["eng".to_string()],
                status: None,
                metadata: None,
                overwrite: false,
                share_link: None,
                model: None,
            })
            .await
            .unwrap();
        let back = read(&engine, domain, "captured").await;
        let content = back["content"].as_str().unwrap();
        assert!(!content.contains('\r'), "{domain}: {content:?}");
        assert!(
            content.contains("line one\nline two\n"),
            "{domain}: {content:?}"
        );
        // What was stored is what is read, so the checksum is of the LF text.
        assert_eq!(back["checksum"], sha256_hex(content.as_bytes()), "{domain}");
    }
    let file = on_disk(&tmp, "captured.md");
    assert!(!file.contains('\r'), "{file:?}");
}

#[tokio::test]
async fn save_engram_content_with_crlf_is_stored_lf() {
    let (tmp, engine) = fixture().await;
    let before = read(&engine, "eng", "windows").await;
    let content = before["content"].as_str().unwrap().replace('\n', "\r\n");
    engine
        .save_engram(
            &SaveParams {
                domain: "eng".to_string(),
                identifier: "windows".to_string(),
                content: content.replace("zebras", "giraffes"),
                expected_checksum: before["checksum"].as_str().unwrap().to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let text = on_disk(&tmp, "windows.md");
    assert!(!text.contains('\r'), "{text:?}");
    assert!(text.contains("giraffes"), "{text:?}");
}

/// A read never rewrites: syncing and reindexing a CRLF file leaves its
/// bytes alone, and a sync after that finds nothing to do.
#[tokio::test]
async fn a_sync_over_a_crlf_file_leaves_its_bytes_unchanged() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/windows.md");
    let before = std::fs::read(&path).unwrap();
    engine.sync(None).await.unwrap();
    let _ = read(&engine, "eng", "windows").await;
    engine.sync(Some("eng")).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read(tmp.path().join("eng/mixed.md")).unwrap(),
        MIXED.as_bytes()
    );
}

/// The read returns LF; its checksum is of the bytes the file holds, so the
/// guarded edit made with it is not refused; the edit's result is LF and the
/// next read's checksum is of those LF bytes.
#[tokio::test]
async fn a_read_of_a_crlf_file_returns_lf_and_its_checksum_guards_an_edit() {
    let (tmp, engine) = fixture().await;
    let first = read(&engine, "eng", "windows").await;
    let content = first["content"].as_str().unwrap();
    assert!(!content.contains('\r'), "{content:?}");
    assert_eq!(content, CRLF.replace("\r\n", "\n"));
    assert_eq!(first["checksum"], sha256_hex(CRLF.as_bytes()));
    let observations = serde_json::to_string(&first["observations"]).unwrap();
    assert!(!observations.contains("\\r"), "{observations}");

    engine
        .edit_engram(&EditParams {
            content: Some("guarded".to_string()),
            expected_checksum: Some(first["checksum"].as_str().unwrap().to_string()),
            ..edit("windows", "append")
        })
        .await
        .unwrap();
    let bytes = std::fs::read(tmp.path().join("eng/windows.md")).unwrap();
    assert!(!bytes.contains(&b'\r'));
    let second = read(&engine, "eng", "windows").await;
    assert_eq!(second["checksum"], sha256_hex(&bytes));

    // The CRLF checksum described the file before that edit, so it is stale.
    let err = engine
        .edit_engram(&EditParams {
            content: Some("again".to_string()),
            expected_checksum: Some(first["checksum"].as_str().unwrap().to_string()),
            ..edit("windows", "append")
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("stale edit"), "{err}");
}

#[tokio::test]
async fn a_search_snippet_from_a_crlf_file_has_no_cr() {
    let (_tmp, engine) = fixture().await;
    let page = engine
        .search_engrams(
            &SearchParams {
                query: Some("zebras".to_string()),
                domains: vec!["eng".to_string()],
                limit: Some(5),
                ..SearchParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let hits = page["hits"].as_array().unwrap();
    assert!(
        hits.iter().any(|h| h["permalink"] == "windows"),
        "the CRLF file is found: {page}"
    );
    let text = page.to_string();
    assert!(!text.contains("\\r"), "{text}");
}

/// An indexed file that is no longer UTF-8 is refused by an edit in one
/// line that says so.
#[tokio::test]
async fn an_edit_of_a_file_that_is_not_utf8_is_refused_in_one_line() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/windows.md");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\xff\xfe broken\n");
    std::fs::write(&path, &bytes).unwrap();
    let err = engine
        .edit_engram(&EditParams {
            content: Some("more".to_string()),
            ..edit("windows", "append")
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not valid UTF-8"), "{err}");
    assert!(!err.contains('\n'), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "nothing was written");
}

/// The read path refuses a file that is no longer UTF-8 rather than falling
/// back to the index row, which for a file domain holds the body only: an
/// answer without the frontmatter would be wrong, and a draft edit built on
/// it (the review-mode arm reads through the same loader) would write a
/// document with the frontmatter gone.
#[tokio::test]
async fn a_read_of_a_file_that_is_not_utf8_is_refused_not_answered_from_the_row() {
    let (tmp, engine) = fixture().await;
    let path = tmp.path().join("eng/windows.md");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\xff\xfe broken\n");
    std::fs::write(&path, &bytes).unwrap();
    let err = engine
        .read_engram(
            &ReadParams {
                identifier: "windows".to_string(),
                domain: Some("eng".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not valid UTF-8"), "{err}");
    assert!(!err.contains('\n'), "{err}");
}

/// An export (and the archive download, which reads through the same
/// function) hands a CRLF engram over as LF.
#[tokio::test]
async fn an_export_hands_a_crlf_engram_over_as_lf() {
    let (tmp, engine) = fixture().await;
    let dest = tmp.path().join("exported");
    engine
        .export_domain("eng", &dest, false, false)
        .await
        .unwrap();
    for rel in ["windows.md", "mixed.md"] {
        let text = std::fs::read_to_string(dest.join(rel)).unwrap();
        assert!(!text.contains('\r'), "{rel}: {text:?}");
    }
}
