//! A capture whose `content` opens with its own frontmatter block.
//!
//! Agents naturally write a whole markdown file, frontmatter and all, and
//! send it as `content`. The engram must still end up with exactly one
//! frontmatter block: the keys of the agent's block are merged into the one
//! Crystalline writes (explicit parameters win, engine-owned keys are
//! ignored) and the receipt names what happened to each, so nothing the
//! agent wrote is dropped without a word.

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_core::parse_engram;
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
use crystalline_service::engine::EngineError;
use crystalline_service::params::{EditParams, WriteParams};
use serde_json::json;
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n";

/// The content an agent sends: a whole file, its own frontmatter included.
const WHOLE_FILE: &str = "---\ntitle: Ignored Title\ntype: decision\nstatus: draft\ntags:\n  - from-block\nsalience: 7\nvalid_from: 2026-01-01\nowner: platform\npermalink: somewhere/else\nrecorded_at: 2020-01-01\ngenerated:\n  by: someone\n---\n\n# Cache Policy\n\nWe cache for five minutes.\n";

/// A file domain `eng` and a virtual domain `scratch`, synced.
async fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.domains
        .insert("scratch".to_string(), DomainEntry::virtual_domain());
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(Arc::new(Mutex::new(store)), cfg, None, None));
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

fn capture(domain: &str, title: &str, content: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: domain.to_string(),
        title: title.to_string(),
        content: Some(content.to_string()),
        folder: None,
        engram_type: None,
        tags: vec![],
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

/// The stored text of an engram, read back through the engine.
async fn stored(engine: &Engine, domain: &str, permalink: &str) -> String {
    let read = engine
        .read_engram(
            &crystalline_service::params::ReadParams {
                identifier: permalink.to_string(),
                domain: Some(domain.to_string()),
                share_link: None,
                include_images: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    read["content"].as_str().unwrap().to_string()
}

/// How many lines of `text` are a bare `---`.
fn delimiter_lines(text: &str) -> usize {
    text.lines().filter(|line| *line == "---").count()
}

fn notices(receipt: &serde_json::Value) -> String {
    receipt["notices"]
        .as_array()
        .map(|all| {
            all.iter()
                .map(|n| n.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_leading_block_is_merged_into_the_one_frontmatter_on_both_storage_kinds() {
    let (tmp, engine) = fixture().await;
    for domain in ["eng", "scratch"] {
        let mut params = capture(domain, "Cache Policy", WHOLE_FILE);
        // An explicit parameter wins over the same key in the block.
        params.tags = vec!["eng".to_string()];
        let receipt = engine.write_engram(&params).await.unwrap();

        let text = stored(&engine, domain, "cache-policy").await;
        assert_eq!(delimiter_lines(&text), 2, "{domain}: one block: {text}");
        let engram = parse_engram(&text).unwrap();
        let fm = &engram.frontmatter;
        assert_eq!(
            fm.title, "Cache Policy",
            "{domain}: the title parameter wins"
        );
        assert_eq!(fm.engram_type, "decision", "{domain}: type from the block");
        assert_eq!(fm.status.as_deref(), Some("draft"), "{domain}");
        assert_eq!(
            fm.tags,
            vec!["eng".to_string()],
            "{domain}: tags param wins"
        );
        assert_eq!(fm.permalink.as_deref(), Some("cache-policy"), "{domain}");
        assert_ne!(
            fm.recorded_at.map(|d| d.ymd()).as_deref(),
            Some("2020-01-01"),
            "{domain}: recorded_at is engine-owned"
        );
        assert_ne!(
            fm.generated.as_ref().map(|g| g.by.as_str()),
            Some("someone"),
            "{domain}: generated is engine-owned"
        );
        assert_eq!(
            fm.valid_from.map(|d| d.ymd()).as_deref(),
            Some("2026-01-01"),
            "{domain}: a temporal key from the block lands typed"
        );
        let salience = fm.extra.get("salience").expect("salience kept");
        assert_eq!(format!("{salience:?}"), "Int(7)", "{domain}");
        assert!(
            fm.extra.contains_key("owner"),
            "{domain}: a custom key is kept"
        );
        assert!(
            engram.body.contains("We cache for five minutes."),
            "{domain}: {text}"
        );
        assert!(!engram.body.contains("salience"), "{domain}: {text}");

        assert_eq!(receipt["type"], "decision", "{domain}: {receipt}");
        assert_eq!(receipt["status"], "draft", "{domain}: {receipt}");
        let said = notices(&receipt);
        for key in [
            "salience",
            "owner",
            "tags",
            "title",
            "permalink",
            "generated",
            "recorded_at",
        ] {
            assert!(
                said.contains(key),
                "{domain}: the notice names {key}: {said}"
            );
        }
    }
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/cache-policy.md")).unwrap();
    assert_eq!(delimiter_lines(&on_disk), 2, "{on_disk}");
}

#[tokio::test]
async fn explicit_metadata_wins_over_the_same_key_in_the_block() {
    let (_tmp, engine) = fixture().await;
    let mut params = capture("eng", "Cache Policy", WHOLE_FILE);
    params.metadata = Some(json!({ "salience": 3, "owner": "search" }));
    params.status = Some("stable".to_string());
    let receipt = engine.write_engram(&params).await.unwrap();
    let fm = parse_engram(&stored(&engine, "eng", "cache-policy").await)
        .unwrap()
        .frontmatter;
    assert_eq!(format!("{:?}", fm.extra["salience"]), "Int(3)");
    assert_eq!(format!("{:?}", fm.extra["owner"]), "String(\"search\")");
    assert_eq!(fm.status.as_deref(), Some("stable"));
    assert_eq!(receipt["status"], "stable");
}

/// Two `---` lines with text between them that is not YAML is markdown (a
/// thematic break, some text, a thematic break) and no reader takes it for
/// frontmatter, so it stays body text as it always did.
#[tokio::test]
async fn a_block_that_is_not_yaml_stays_body_text() {
    let (_tmp, engine) = fixture().await;
    engine
        .write_engram(&capture(
            "eng",
            "Broken",
            "---\ntags: [unclosed\n---\n\nBody.\n",
        ))
        .await
        .unwrap();
    let engram = parse_engram(&stored(&engine, "eng", "broken").await).unwrap();
    assert!(engram.body.contains("tags: [unclosed"), "{}", engram.body);
    assert!(engram.frontmatter.tags.is_empty());
}

/// A thematic break, a line and a thematic break is markdown, not a
/// frontmatter block: it holds no keys, so it stays body text as before.
#[tokio::test]
async fn a_block_that_is_no_mapping_stays_body_text() {
    let (_tmp, engine) = fixture().await;
    engine
        .write_engram(&capture(
            "eng",
            "Breaks",
            "---\nJust a line between rules\n---\n\nBody.\n",
        ))
        .await
        .unwrap();
    let engram = parse_engram(&stored(&engine, "eng", "breaks").await).unwrap();
    assert!(engram.body.contains("Just a line between rules"));
}

/// A rule, a heading and a rule is markdown: YAML would read the heading as
/// a comment, so the block is not taken for frontmatter and the heading stays.
#[tokio::test]
async fn a_heading_between_rules_stays_body_text() {
    let (_tmp, engine) = fixture().await;
    engine
        .write_engram(&capture("eng", "Headed", "---\n# Overview\n---\n\nBody.\n"))
        .await
        .unwrap();
    let engram = parse_engram(&stored(&engine, "eng", "headed").await).unwrap();
    assert!(engram.body.contains("# Overview"), "{}", engram.body);
}

/// A line with a colon between two rules names no key the engram schema
/// knows, so it is a note in the body and not a metadata key.
#[tokio::test]
async fn a_note_with_a_colon_between_rules_stays_body_text() {
    let (_tmp, engine) = fixture().await;
    let receipt = engine
        .write_engram(&capture(
            "eng",
            "Noted",
            "---\nNote: remember X\n---\n\nBody.\n",
        ))
        .await
        .unwrap();
    let engram = parse_engram(&stored(&engine, "eng", "noted").await).unwrap();
    assert!(engram.body.contains("Note: remember X"), "{}", engram.body);
    assert!(!engram.frontmatter.extra.contains_key("Note"));
    assert!(receipt.get("notices").is_none(), "{receipt}");
}

/// An empty block carries nothing: it is dropped, the engram keeps one, and
/// the receipt says so.
#[tokio::test]
async fn an_empty_block_is_dropped() {
    let (_tmp, engine) = fixture().await;
    let receipt = engine
        .write_engram(&capture("eng", "Empty", "---\n---\n\nBody.\n"))
        .await
        .unwrap();
    let text = stored(&engine, "eng", "empty").await;
    assert_eq!(delimiter_lines(&text), 2, "{text}");
    let said = notices(&receipt);
    assert!(said.contains("empty frontmatter block"), "{receipt}");
}

fn edit(identifier: &str, operation: &str, content: &str) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        domain: "eng".to_string(),
        operation: operation.to_string(),
        content: Some(content.to_string()),
        ..Default::default()
    }
}

/// Every edit, not only a prepend, is refused when its result would open
/// the body with a frontmatter block: an insert before the first heading, an
/// append to an empty body and a find and replace at the top alike.
#[tokio::test]
async fn no_edit_puts_a_second_block_under_the_engram_s_own() {
    let (_tmp, engine) = fixture().await;
    engine
        .write_engram(&capture("eng", "Headed", "# Top\n\nBody."))
        .await
        .unwrap();
    engine
        .write_engram(&capture("eng", "Blank", ""))
        .await
        .unwrap();
    let block = "---\nstatus: draft\n---\nBefore";
    let mut insert = edit("headed", "insert_before_section", block);
    insert.section = Some("Top".to_string());
    let mut replace = edit("headed", "find_replace", "---\nstatus: draft\n---\n# Top");
    replace.find_text = Some("# Top".to_string());
    for (permalink, params) in [
        ("headed", insert),
        ("blank", edit("blank", "append", block)),
        ("headed", replace),
    ] {
        let before = stored(&engine, "eng", permalink).await;
        let err = engine.edit_engram(&params).await.unwrap_err();
        assert!(matches!(err, EngineError::Invalid(_)), "{err}");
        assert!(err.to_string().contains("set_frontmatter"), "{err}");
        assert_eq!(stored(&engine, "eng", permalink).await, before);
    }
    // A rule further down is markdown, and an append lands.
    engine
        .edit_engram(&edit("headed", "append", "---\nstatus: draft\n---\nAfter"))
        .await
        .unwrap();
}

/// What 0.33.0 stored for a capture of [`WHOLE_FILE`] titled "Cache Policy"
/// with no other parameter: its own block with the defaults, then the
/// content's block as body text.
const DOUBLED: &str = "---\ntype: engram\ntitle: Cache Policy\npermalink: cache-policy\ntags: []\nstatus: stable\nrecorded_at: 2026-11-01\ngenerated: { by: agent, at: 2026-11-01T10:00:00Z }\n---\n\n---\ntitle: Ignored Title\ntype: decision\nstatus: draft\ntags:\n  - from-block\nsalience: 7\nvalid_from: 2026-01-01\nowner: platform\npermalink: somewhere/else\nrecorded_at: 2020-01-01\ngenerated:\n  by: someone\n---\n\n# Cache Policy\n\nWe cache for five minutes.\n";

/// An engram 0.33.0 stored with two blocks is found by validate (E012),
/// still takes edits, and doctor's merge gives the frontmatter and the body
/// a rewrite of the same content through `write_engram` gives.
#[tokio::test]
async fn a_0_33_0_second_block_is_found_and_merges_as_a_rewrite_would() {
    let (tmp, engine) = fixture().await;
    std::fs::write(tmp.path().join("eng/cache-policy.md"), DOUBLED).unwrap();
    engine.sync(None).await.unwrap();

    let report = engine
        .validate_engrams(
            &crystalline_service::params::ValidateParams {
                domain: "eng".to_string(),
                identifier: Some("cache-policy".to_string()),
                engram_type: None,
                drift: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "E012"),
        "{report}"
    );

    // Still editable, so it can be repaired.
    engine
        .edit_engram(&edit("cache-policy", "append", "One more line."))
        .await
        .unwrap();
    let current = stored(&engine, "eng", "cache-policy").await;
    let crystalline_core::frontmatter::BlockMerge::Merged(merged) =
        crystalline_core::frontmatter::merge_second_block(&current).expect("a second block")
    else {
        panic!("not merged");
    };

    // The rewrite an agent does today with the same content.
    let content = current.splitn(3, "---\n").nth(2).unwrap().to_string();
    let mut rewrite = capture("eng", "Cache Policy", &content);
    rewrite.overwrite = true;
    engine.write_engram(&rewrite).await.unwrap();
    let rewritten = parse_engram(&stored(&engine, "eng", "cache-policy").await).unwrap();
    let merged = parse_engram(&merged).unwrap();
    let (a, b) = (&merged.frontmatter, &rewritten.frontmatter);
    assert_eq!(a.title, b.title);
    assert_eq!(a.engram_type, b.engram_type);
    assert_eq!(a.status, b.status);
    assert_eq!(a.tags, b.tags);
    assert_eq!(a.valid_from, b.valid_from);
    assert_eq!(a.permalink, b.permalink);
    assert_eq!(a.extra, b.extra);
    assert_eq!(merged.body, rewritten.body);
    assert_eq!(a.engram_type, "decision");
    assert_eq!(
        a.extra.get("owner").and_then(|v| v.as_str()),
        Some("platform")
    );
}

/// A prepend lands right under the frontmatter, so a block at its top would
/// be a second frontmatter block. It is refused with the way forward.
#[tokio::test]
async fn a_prepend_that_opens_with_a_frontmatter_block_is_refused() {
    let (_tmp, engine) = fixture().await;
    engine
        .write_engram(&capture("eng", "Plain", "Body."))
        .await
        .unwrap();
    let before = stored(&engine, "eng", "plain").await;
    let err = engine
        .edit_engram(&EditParams {
            identifier: "plain".to_string(),
            domain: "eng".to_string(),
            operation: "prepend".to_string(),
            content: Some("---\nstatus: draft\n---\n\nIntro.".to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    assert!(err.to_string().contains("set_frontmatter"), "{err}");
    assert_eq!(stored(&engine, "eng", "plain").await, before);
}

/// A prepend whose leading block is not YAML is text and lands as sent.
#[tokio::test]
async fn a_prepend_whose_block_is_not_yaml_lands_as_text() {
    let (_tmp, engine) = fixture().await;
    engine
        .write_engram(&capture("eng", "Plain", "Body."))
        .await
        .unwrap();
    engine
        .edit_engram(&EditParams {
            identifier: "plain".to_string(),
            domain: "eng".to_string(),
            operation: "prepend".to_string(),
            content: Some("---\nNot: [yaml\n---\n\nIntro.".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    let engram = parse_engram(&stored(&engine, "eng", "plain").await).unwrap();
    assert!(engram.body.contains("Not: [yaml"), "{}", engram.body);
}

/// An upload is a file, not an engram: a YAML file that opens with `---` is
/// stored byte for byte.
#[tokio::test]
async fn an_uploaded_yaml_file_keeps_its_leading_block() {
    let (tmp, engine) = fixture().await;
    let yaml = "---\nname: x\n---\nmore: y\n";
    let mut params = capture("eng", "", yaml);
    params.path = Some("assets/x.yaml".to_string());
    engine
        .upload_attachment(&params, &Scope::Unrestricted, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/assets/x.yaml")).unwrap(),
        yaml
    );
}
