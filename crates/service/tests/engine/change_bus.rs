//! The change bus, driven at the engine: every path that lands a change in
//! the store announces it exactly once, a generated listing never does, an
//! edit that composed into an open room does not until the room saves, and
//! a resync collapses above the threshold.

use std::path::Path;
use std::sync::Arc;

use crate::support;
use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::TursoStore;
use crystalline_service::changes::{
    Change, ChangeKind, DomainAudience, DomainChanged, EngramChanged, Envelope,
};
use crystalline_service::params::*;
use crystalline_service::{Engine, Scope};
use tokio::sync::{Mutex, broadcast};

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

fn manifest(name: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
    )
}

fn seed(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Two file domains, `notes` (alpha, beta) and `oak` (empty), synced; `review`
/// puts `notes` in review mode so a write lands in the actor's own draft.
async fn engine_fixture(
    review: bool,
) -> (tempfile::TempDir, Arc<Engine>, support::ScratchStateDir) {
    let scratch = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    let notes = root.join("notes");
    seed(&notes, "MANIFEST.md", &manifest("notes"));
    seed(
        &notes,
        "alpha.md",
        &engram("Alpha", "alpha", "A rule about alpha."),
    );
    seed(
        &notes,
        "beta.md",
        &engram("Beta", "beta", "A rule about beta."),
    );
    let mut entry = DomainEntry::file(notes);
    if review {
        entry.review = Some(ReviewMode::Overlay);
    }
    cfg.domains.insert("notes".to_string(), entry);
    let oak = root.join("oak");
    seed(&oak, "MANIFEST.md", &manifest("oak"));
    cfg.domains
        .insert("oak".to_string(), DomainEntry::file(oak));
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
}

/// Everything announced since the receiver subscribed, in order.
fn drain(rx: &mut broadcast::Receiver<Envelope>) -> Vec<Change> {
    let mut out = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        out.push(envelope.change);
    }
    out
}

fn engram_of(change: &Change) -> &EngramChanged {
    match change {
        Change::Engram(change) => change,
        other => panic!("expected an engram change, got {other:?}"),
    }
}

fn write_params(title: &str, overwrite: bool) -> WriteParams {
    WriteParams {
        domain: "notes".to_string(),
        title: title.to_string(),
        content: "the body".to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite,
        share_link: None,
        model: None,
    }
}

fn append_edit(identifier: &str, content: &str) -> EditParams {
    EditParams {
        identifier: identifier.to_string(),
        domain: "notes".to_string(),
        operation: "append".to_string(),
        content: Some(content.to_string()),
        ..EditParams::default()
    }
}

#[tokio::test]
async fn a_file_the_watcher_finds_announces_one_modified_with_no_actor() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    let revised = engram(
        "Alpha",
        "alpha",
        "A longer rule about alpha, revised on disk.",
    );
    seed(&tmp.path().join("notes"), "alpha.md", &revised);
    engine
        .sync_paths("notes", vec!["alpha.md".to_string()])
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(change.kind, ChangeKind::Modified);
    assert_eq!(change.domain, "notes");
    assert_eq!(change.permalink, "alpha");
    assert_eq!(change.path, "alpha.md");
    assert_eq!(change.actor, None);
    assert_eq!(
        change.checksum.as_deref(),
        Some(support::sha256_hex(revised.as_bytes()).as_str())
    );
    assert_eq!(change.draft_of, None);
}

#[tokio::test]
async fn write_engram_announces_added_then_modified_on_overwrite() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    let receipt = engine
        .write_engram(&write_params("Gamma", false))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let added = engram_of(&heard[0]);
    assert_eq!(added.kind, ChangeKind::Added);
    assert_eq!(added.permalink, "gamma");
    assert_eq!(added.actor.as_deref(), Some(engine.actor(None).as_str()));
    assert!(added.checksum.is_some());
    assert_eq!(receipt["permalink"], "gamma");

    engine
        .write_engram(&write_params("Gamma", true))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(engram_of(&heard[0]).kind, ChangeKind::Modified);
}

#[tokio::test]
async fn an_engine_write_is_not_announced_again_by_the_watcher_pass_behind_it() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .write_engram(&write_params("Gamma", false))
        .await
        .unwrap();
    assert_eq!(drain(&mut rx).len(), 1);
    // The watcher's targeted pass a moment later: the file's stamp is already
    // the index's, so the scan classifies it unchanged and nothing rides.
    engine
        .sync_paths("notes", vec!["gamma.md".to_string()])
        .await
        .unwrap();
    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn edit_engram_announces_one_modified_with_the_editing_actor() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .edit_engram_as(
            &append_edit("alpha", "another line"),
            Some("agent/1.0"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(change.kind, ChangeKind::Modified);
    assert_eq!(
        change.actor.as_deref(),
        Some(engine.actor(Some("agent/1.0")).as_str())
    );
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("notes".to_string()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        change.checksum.as_deref(),
        read["checksum"].as_str(),
        "the event and the read agree"
    );
}

#[tokio::test]
async fn save_engram_announces_modified_under_the_accounts_name() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("notes".to_string()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let mut rx = engine.changes().subscribe();
    let content = engram("Alpha", "alpha", "Saved from the browser.");
    engine
        .save_engram(
            &SaveParams {
                domain: "notes".to_string(),
                identifier: "alpha".to_string(),
                content: content.clone(),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(change.kind, ChangeKind::Modified);
    assert_eq!(change.actor.as_deref(), Some("ada"));
    assert_eq!(
        change.checksum.as_deref(),
        Some(support::sha256_hex(content.as_bytes()).as_str())
    );
}

#[tokio::test]
async fn a_move_inside_a_domain_is_one_moved_with_from_and_across_domains_is_deleted_plus_added() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .move_engram(
            &MoveParams {
                identifier: "alpha".to_string(),
                domain: "notes".to_string(),
                destination: "topics/alpha".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let moved = engram_of(&heard[0]);
    assert_eq!(moved.kind, ChangeKind::Moved);
    assert_eq!(moved.path, "topics/alpha.md");
    // `alpha` was in step with its path, so the permalink follows the move
    // (`Engine::moved_permalink`); the receipt says the same.
    assert_eq!(
        moved.permalink, "topics/alpha",
        "an in-step permalink follows the path"
    );
    let from = moved.from.as_ref().expect("a move names where from");
    assert_eq!(
        (from.path.as_str(), from.permalink.as_str()),
        ("alpha.md", "alpha")
    );
    assert!(moved.checksum.is_some());

    engine
        .move_engram(
            &MoveParams {
                identifier: "beta".to_string(),
                domain: "notes".to_string(),
                destination: "beta".to_string(),
                destination_domain: Some("oak".to_string()),
                permalink: None,
                update_links: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 2, "{heard:?}");
    let gone = engram_of(&heard[0]);
    assert_eq!(
        (gone.domain.as_str(), gone.kind, gone.permalink.as_str()),
        ("notes", ChangeKind::Deleted, "beta")
    );
    let landed = engram_of(&heard[1]);
    assert_eq!(
        (landed.domain.as_str(), landed.kind, landed.path.as_str()),
        ("oak", ChangeKind::Added, "beta.md")
    );
    assert!(landed.checksum.is_some());
}

#[tokio::test]
async fn delete_engram_announces_deleted_with_the_permalink_filled() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .delete_engram(&DeleteParams {
            identifier: "alpha".to_string(),
            domain: "notes".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(
        (change.kind, change.permalink.as_str(), change.path.as_str()),
        (ChangeKind::Deleted, "alpha", "alpha.md")
    );
    assert_eq!(change.checksum, None);
    // The listing that lost a row is regenerated and never announced.
    engine
        .sync_paths("notes", vec!["index.md".to_string()])
        .await
        .unwrap();
    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn split_engram_announces_the_new_engram_and_the_edited_source() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    engine
        .edit_engram(&append_edit(
            "alpha",
            "- [fact] one\n- [fact] two\n- [fact] three\n- [fact] four",
        ))
        .await
        .unwrap();
    // The one-based line of `- [fact] two`, as `read_engram` reports it.
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "alpha".to_string(),
                domain: Some("notes".to_string()),
                ..ReadParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let line = read["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["content"].as_str().unwrap_or_default().contains("two"))
        .expect("the fact to split out")["line"]
        .as_u64()
        .unwrap() as usize;
    let mut rx = engine.changes().subscribe();
    engine
        .split_engram(&SplitParams {
            domain: "notes".to_string(),
            identifier: "alpha".to_string(),
            title: "Alpha Facts".to_string(),
            observations: vec![line],
            ..SplitParams::default()
        })
        .await
        .unwrap();
    let kinds: Vec<(String, ChangeKind)> = drain(&mut rx)
        .iter()
        .map(|c| (engram_of(c).permalink.clone(), engram_of(c).kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("alpha-facts".to_string(), ChangeKind::Added),
            ("alpha".to_string(), ChangeKind::Modified)
        ]
    );
}

#[tokio::test]
async fn an_overlay_write_announces_with_draft_of_and_a_tombstone_as_deleted() {
    let (_tmp, engine, _scratch) = engine_fixture(true).await;
    let mut rx = engine.changes().subscribe();
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    engine
        .edit_engram_as(&append_edit("alpha", "ada's line"), None, &ada)
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let draft = engram_of(&heard[0]);
    assert_eq!(draft.kind, ChangeKind::Modified);
    assert_eq!(draft.draft_of.as_deref(), Some("ada"));
    assert_eq!(draft.actor.as_deref(), Some("ada"));
    assert!(draft.checksum.is_some());

    engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "alpha".to_string(),
                domain: "notes".to_string(),
                expected_checksum: None,
            },
            None,
            &ada,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let tomb = engram_of(&heard[0]);
    assert_eq!(
        (tomb.kind, tomb.draft_of.as_deref()),
        (ChangeKind::Deleted, Some("ada"))
    );
}

#[tokio::test]
async fn a_resync_of_forty_files_announces_one_domain_and_of_three_announces_three_engrams() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let root = tmp.path().join("notes");
    let mut rx = engine.changes().subscribe();
    for i in 0..40 {
        seed(
            &root,
            &format!("bulk/b{i}.md"),
            &engram(&format!("B{i}"), &format!("b{i}"), "bulk"),
        );
    }
    engine.sync(Some("notes")).await.unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert!(matches!(&heard[0], Change::Domain(d) if d.domain == "notes" && d.actor.is_none()));

    for i in 0..3 {
        seed(
            &root,
            &format!("few/f{i}.md"),
            &engram(&format!("F{i}"), &format!("f{i}"), "few"),
        );
    }
    engine.sync(Some("notes")).await.unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 3, "{heard:?}");
    assert!(heard.iter().all(|c| engram_of(c).kind == ChangeKind::Added));
}

#[tokio::test]
async fn a_discard_announces_under_the_discarding_account() {
    // Wave 4's discard restores base copies and reindexes the touched paths
    // through `sync_paths_as`. The team fixture that reaches it lives in
    // `origin.rs`; this test only pins the label rule at the seam it can
    // reach without a forge: a targeted sync run on somebody's behalf.
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    seed(
        &tmp.path().join("notes"),
        "alpha.md",
        &engram("Alpha", "alpha", "put back the way the team has it"),
    );
    engine
        .sync_paths_as("notes", vec!["alpha.md".to_string()], Some("ada"))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1);
    assert_eq!(engram_of(&heard[0]).actor.as_deref(), Some("ada"));
}

#[tokio::test]
async fn a_move_announces_every_linker_it_rewrote() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    seed(
        &tmp.path().join("notes"),
        "linker.md",
        &engram("Linker", "linker", "See [[alpha]]."),
    );
    engine
        .sync_paths("notes", vec!["linker.md".to_string()])
        .await
        .unwrap();
    let mut rx = engine.changes().subscribe();
    engine
        .move_engram(
            &MoveParams {
                identifier: "alpha".to_string(),
                domain: "notes".to_string(),
                destination: "topics/alpha".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: Some(true),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard: Vec<(String, ChangeKind)> = drain(&mut rx)
        .iter()
        .map(|c| (engram_of(c).path.clone(), engram_of(c).kind))
        .collect();
    assert!(
        heard.contains(&("linker.md".to_string(), ChangeKind::Modified)),
        "{heard:?}"
    );
    assert!(
        heard.contains(&("topics/alpha.md".to_string(), ChangeKind::Moved)),
        "{heard:?}"
    );
}

#[tokio::test]
async fn a_manifest_save_announces_the_manifest() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let current = std::fs::read_to_string(tmp.path().join("notes/MANIFEST.md")).unwrap();
    let mut rx = engine.changes().subscribe();
    let revised = current.replace("Everything about notes", "Everything about notes, revised");
    engine
        .save_manifest("notes", &revised, &support::sha256_hex(current.as_bytes()))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(
        (change.path.as_str(), change.kind),
        ("MANIFEST.md", ChangeKind::Modified)
    );
    assert_eq!(
        change.checksum.as_deref(),
        Some(support::sha256_hex(revised.as_bytes()).as_str())
    );
}

#[tokio::test]
async fn a_domain_removal_and_a_rename_announce_domain_events() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .rename_domain("oak", "elm", true, &Scope::Unrestricted)
        .await
        .unwrap();
    let domains: Vec<DomainChanged> = drain(&mut rx)
        .into_iter()
        .filter_map(|c| match c {
            Change::Domain(d) => Some(d),
            Change::Engram(_) => None,
        })
        .collect();
    let names: Vec<String> = domains.iter().map(|d| d.domain.clone()).collect();
    assert!(
        names.contains(&"oak".to_string()) && names.contains(&"elm".to_string()),
        "{names:?}"
    );
    // Section J (k), ruled 2026-09-27: only the old name's audience is
    // captured, since it is the one leaving the privacy records; the new
    // name is re-keyed, not destroyed, so it keeps the ordinary lazy check.
    // Neither `oak` nor `elm` is private in this fixture, so the captured
    // snapshot is `Everyone`; the member-set case is T2's, which has an
    // `AuthStore` to make a domain private against.
    let old = domains.iter().find(|d| d.domain == "oak").unwrap();
    let new = domains.iter().find(|d| d.domain == "elm").unwrap();
    assert_eq!(old.audience, Some(DomainAudience::Everyone), "{old:?}");
    assert_eq!(new.audience, None, "{new:?}");

    engine
        .unregister_domain("elm", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(
        heard.iter().any(|c| matches!(c, Change::Domain(d) if d.domain == "elm" && d.audience == Some(DomainAudience::Everyone))),
        "{heard:?}"
    );
}

#[tokio::test]
async fn a_virtual_import_announces_what_it_wrote() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    engine.domain_add_virtual("vault").await.unwrap();
    let src = tmp.path().join("incoming");
    seed(&src, "one.md", &engram("One", "one", "imported"));
    seed(&src, "two.md", &engram("Two", "two", "imported"));
    let mut rx = engine.changes().subscribe();
    engine
        .import_domain("vault", &src, false, false)
        .await
        .unwrap();
    let mut paths: Vec<String> = drain(&mut rx)
        .iter()
        .map(|c| engram_of(c).path.clone())
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["one.md".to_string(), "two.md".to_string()]);
}

/// A registration binds the references other domains already spelled with
/// the new domain's name. Nothing's text changed, but the reading page's link
/// states, the backlinks and the graph did, so the domain holding the linker
/// is announced whole. Registered at runtime with `domain_add_local`, the
/// call `domain_names.rs` makes.
#[tokio::test]
async fn a_registration_that_binds_pending_links_announces_the_linking_domain() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    seed(
        &tmp.path().join("notes"),
        "linker.md",
        &engram("Linker", "linker", "See [[pine:alpha]]."),
    );
    engine
        .sync_paths("notes", vec!["linker.md".to_string()])
        .await
        .unwrap();
    let pine = tmp.path().join("pine");
    seed(&pine, "MANIFEST.md", &manifest("pine"));
    seed(
        &pine,
        "alpha.md",
        &engram("Alpha", "alpha", "The pine alpha."),
    );
    let mut rx = engine.changes().subscribe();
    engine
        .domain_add_local(Some("pine"), Some(pine.to_str().unwrap()))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(
        heard
            .iter()
            .any(|c| matches!(c, Change::Domain(d) if d.domain == "notes" && d.audience.is_none())),
        "{heard:?}"
    );
}
