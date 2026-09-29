//! The change bus, driven at the engine: every path that lands a change in
//! the store announces it exactly once, a generated listing never does, an
//! edit that composed into an open room does not until the room saves, and
//! a resync collapses above the threshold.

use std::path::Path;
use std::sync::Arc;

use crate::support;
use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::{Store, TursoStore};
use crystalline_service::changes::{
    Change, ChangeKind, DomainAudience, DomainChanged, EngramChanged, Envelope,
};
use crystalline_service::params::*;
use crystalline_service::{Engine, Scope};
use tokio::sync::{Mutex, broadcast};

#[cfg(feature = "postgres")]
fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres virtual-domain leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
                )
            });
            None
        }
    }
}

/// A recycled pid must never adopt a schema a panicking run left behind.
#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "ctv_{}_{}_{:x}",
        std::process::id(),
        n,
        RandomState::new().hash_one(n)
    )
}

/// Run a body against Turso (always) and Postgres (when configured), each with a
/// fresh, isolated store handed to the engine as a trait object.
macro_rules! both_backends {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
                $body(store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let schema = unique_schema();
                    let pg = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .expect("open the postgres test schema");
                    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(pg));
                    $body(store).await;
                    // Drop the schema through a fresh connection (the boxed store
                    // no longer exposes the inherent drop_schema).
                    let cleanup = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .unwrap();
                    cleanup.drop_schema().await.unwrap();
                }
            }
        }
    };
}

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
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    engine_fixture_on(store, review).await
}

/// [`engine_fixture`] over a store the caller picked, so a body runs on both
/// backends.
async fn engine_fixture_on(
    store: Arc<Mutex<dyn Store>>,
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
    let engine = Arc::new(
        Engine::new(store, cfg, None, Some(config_path)).with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine, scratch)
}

/// Everything announced since the receiver subscribed, in order.
/// The fail-closed audience: the machine owner only.
fn nobody() -> DomainAudience {
    DomainAudience::Accounts(std::collections::HashSet::new())
}

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
    let (tmp, engine, _scratch) = engine_fixture(false).await;
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
    let landed = std::fs::read(tmp.path().join("notes/topics/alpha.md")).unwrap();
    assert_eq!(
        moved.checksum.as_deref(),
        Some(support::sha256_hex(&landed).as_str()),
        "the checksum is the stored file's, the rewritten permalink line included"
    );

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
    let stored = std::fs::read(tmp.path().join("oak/beta.md")).unwrap();
    assert_eq!(
        landed.checksum.as_deref(),
        Some(support::sha256_hex(&stored).as_str())
    );
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
    // Ruled 2026-09-27: only the old name's audience is
    // captured, since it is the one leaving the privacy records; the new
    // name is re-keyed, not destroyed, so it keeps the ordinary lazy check.
    // This engine has no `DomainAccess` installed, so the capture fails
    // closed: nobody but the machine owner. The rename is
    // not vetoed for it. The member-set case is
    // `a_private_domains_rename_and_removal_carry_the_captured_audience`.
    let old = domains.iter().find(|d| d.domain == "oak").unwrap();
    let new = domains.iter().find(|d| d.domain == "elm").unwrap();
    assert_eq!(old.audience, Some(nobody()), "{old:?}");
    assert_eq!(new.audience, None, "{new:?}");

    engine
        .unregister_domain("elm", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(
        heard.iter().any(
            |c| matches!(c, Change::Domain(d) if d.domain == "elm" && d.audience == Some(nobody()))
        ),
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

// --- captured audiences over a real accounts store ------------------------

/// `oak` made private to `keeper` with `mem` as a viewer, `boss` an instance
/// admin and `out` a stranger, on an auth store installed on the engine.
async fn private_oak(engine: &Engine, dir: &Path) {
    use crystalline_service::rest::{AuthStore, MemberLevel, Role};
    let auth = Arc::new(AuthStore::open(&dir.join("web-auth.db")).await.unwrap());
    for (name, role) in [
        ("keeper", Role::Editor),
        ("boss", Role::Admin),
        ("mem", Role::Viewer),
        ("out", Role::Editor),
    ] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("oak", true, "keeper")
        .await
        .unwrap();
    auth.upsert_domain_member("oak", "mem", MemberLevel::Viewer, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
}

fn oak_readers() -> DomainAudience {
    DomainAudience::Accounts(
        ["keeper", "boss", "mem"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
}

fn oak_write(title: &str) -> WriteParams {
    WriteParams {
        domain: "oak".to_string(),
        ..write_params(title, false)
    }
}

/// Ruled 2026-09-27: a private domain's rename captures who could read it
/// before the records move, and every event under the old name carries that
/// snapshot: the domain event, the engram event of the rename's own MANIFEST
/// step, and (the replay path) an event the ring held from before the
/// rename. The new name keeps the ordinary check. Catches a capture that
/// answers `Everyone`, one taken after the records moved, and a ring that
/// replays old entries unstamped.
#[tokio::test]
async fn a_private_domains_rename_and_removal_carry_the_captured_audience() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    private_oak(&engine, tmp.path()).await;
    // An event from before the rename, which the ring keeps for replay.
    engine.write_engram(&oak_write("Acorn")).await.unwrap();
    let before = engine.changes().last_id().unwrap();
    let mut rx = engine.changes().subscribe();
    engine
        .rename_domain("oak", "elm", false, &Scope::Unrestricted)
        .await
        .unwrap();
    let heard = drain(&mut rx);
    let under_old: Vec<&Change> = heard.iter().filter(|c| c.domain() == "oak").collect();
    assert!(
        under_old
            .iter()
            .any(|c| matches!(c, Change::Engram(e) if e.path == "MANIFEST.md")),
        "the MANIFEST step announced under the old name: {heard:?}"
    );
    assert!(
        under_old.iter().any(|c| matches!(c, Change::Domain(_))),
        "{heard:?}"
    );
    for change in &under_old {
        assert_eq!(change.audience(), Some(&oak_readers()), "{change:?}");
    }
    for change in heard.iter().filter(|c| c.domain() == "elm") {
        assert_eq!(
            change.audience(),
            None,
            "the new name is re-keyed: {change:?}"
        );
    }

    // Replay: the entry written before the capture is stamped too.
    let replayed = match engine
        .changes()
        .replay_after(crystalline_service::changes::EventId {
            epoch: before.epoch,
            seq: before.seq - 1,
        }) {
        crystalline_service::changes::Replay::Events(events) => events,
        other => panic!("{other:?}"),
    };
    let acorn = replayed
        .iter()
        .find(|e| matches!(&e.change, Change::Engram(c) if c.path == "acorn.md"))
        .expect("the earlier write is still in the ring");
    assert_eq!(acorn.change.audience(), Some(&oak_readers()));

    // A removal captures the same way: `elm` carries oak's records now.
    let mut rx = engine.changes().subscribe();
    engine
        .unregister_domain("elm", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    let heard = drain(&mut rx);
    let gone = heard
        .iter()
        .find(|c| matches!(c, Change::Domain(d) if d.domain == "elm"))
        .expect("the removal announced");
    assert_eq!(gone.audience(), Some(&oak_readers()));

    // The capture ended with the change: a later event under the old name
    // is back on the ordinary check.
    engine.changes().announce(Change::Domain(DomainChanged {
        domain: "oak".to_string(),
        actor: None,
        audience: None,
    }));
    let after = drain(&mut rx);
    assert_eq!(after.last().unwrap().audience(), None);
}

/// A capture whose record read fails answers nobody, never `Everyone`, and
/// never vetoes the removal. `oak` is private, and the membership table is
/// replaced by one the store cannot read before the store opens (the breaker
/// `mcp_auth.rs` uses for the visibility table), so the capture's member read
/// errs while the removal's own steps, which swallow a record error, run on.
#[tokio::test]
async fn a_capture_that_cannot_read_the_records_fails_closed_and_the_removal_proceeds() {
    use crystalline_service::rest::{AuthStore, Role};
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let path = tmp.path().join("broken-auth.db");
    {
        let auth = AuthStore::open(&path).await.unwrap();
        auth.add_user("keeper", "keeper", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        auth.set_domain_visibility("oak", true, "keeper")
            .await
            .unwrap();
    }
    // Sequential, like `domain_admin.rs`'s breaker: the store is closed
    // before this connection opens, and this one before the store re-opens.
    {
        let name = path.to_string_lossy().to_string();
        let db = match turso::Builder::new_local(&name)
            .experimental_multiprocess_wal(true)
            .build()
            .await
        {
            Ok(db) => db,
            Err(_) => turso::Builder::new_local(&name).build().await.unwrap(),
        };
        let conn = db.connect().unwrap();
        conn.execute_batch("ALTER TABLE domain_member RENAME COLUMN principal TO junk;")
            .await
            .unwrap();
    }
    let broken = Arc::new(AuthStore::open(&path).await.unwrap());
    engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(broken)));
    let mut rx = engine.changes().subscribe();
    engine
        .unregister_domain("oak", &Scope::Unrestricted, false, &[])
        .await
        .expect("a notification never vetoes the removal");
    let heard = drain(&mut rx);
    let gone = heard
        .iter()
        .find(|c| matches!(c, Change::Domain(d) if d.domain == "oak"))
        .expect("the removal announced");
    assert_eq!(gone.audience(), Some(&nobody()));
}

/// An `overwrite` write whose title nobody held creates an engram, so it is
/// `added` (the switcher's counts follow); only a replacement is `modified`.
#[tokio::test]
async fn an_overwrite_that_creates_is_added_and_a_replacement_is_modified() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .write_engram(&write_params("Gamma", true))
        .await
        .unwrap();
    assert_eq!(engram_of(&drain(&mut rx)[0]).kind, ChangeKind::Added);
    engine
        .write_engram(&write_params("Gamma", true))
        .await
        .unwrap();
    assert_eq!(engram_of(&drain(&mut rx)[0]).kind, ChangeKind::Modified);
}

/// Retag is a feed point: every rewritten engram is announced, and a merge's
/// alias recording announces the MANIFEST it rewrote.
#[tokio::test]
async fn a_retag_merge_announces_every_rewritten_engram_and_the_manifest() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    engine
        .retag("t", "manifest", Some("notes"), true, false, true)
        .await
        .unwrap();
    let mut paths: Vec<String> = drain(&mut rx)
        .iter()
        .map(|c| {
            let e = engram_of(c);
            assert_eq!(e.kind, ChangeKind::Modified);
            assert!(e.checksum.is_some());
            e.path.clone()
        })
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["MANIFEST.md", "alpha.md", "beta.md"]);
}

/// A restore is a feed point: `added`, with the restored bytes' checksum.
#[tokio::test]
async fn a_restore_announces_added() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    let content = engram("Gamma", "gamma", "Restored.");
    engine
        .restore_engram("notes", "gamma.md", &content, &Scope::Unrestricted)
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(
        (change.kind, change.permalink.as_str()),
        (ChangeKind::Added, "gamma")
    );
    assert_eq!(
        change.checksum.as_deref(),
        Some(support::sha256_hex(content.as_bytes()).as_str())
    );
}

/// A reindex is a feed point through `DaemonReindexHooks::after_apply`.
#[tokio::test]
async fn a_reindex_announces_what_it_changed() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    let revised = engram("Alpha", "alpha", "Changed on disk, found by a reindex.");
    seed(&tmp.path().join("notes"), "alpha.md", &revised);
    let mut rx = engine.changes().subscribe();
    engine.reindex(false).await.unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(
        (change.kind, change.path.as_str()),
        (ChangeKind::Modified, "alpha.md")
    );
}

/// The virtual-domain writes that skip the verbs: a scaffolded MANIFEST, an
/// archive import's rows and a MANIFEST save on a virtual domain.
#[tokio::test]
async fn a_virtual_domains_scaffold_import_and_manifest_save_announce() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    let mut rx = engine.changes().subscribe();
    // `domain_add_virtual` scaffolds the domain's MANIFEST through
    // `scaffold_virtual_manifest`.
    engine.domain_add_virtual("vault").await.unwrap();
    let heard = drain(&mut rx);
    assert!(
        heard.iter().any(|c| matches!(c, Change::Engram(e)
            if e.domain == "vault" && e.path == "MANIFEST.md" && e.kind == ChangeKind::Added)),
        "{heard:?}"
    );
    // A scaffold that finds one already there announces nothing.
    engine
        .scaffold_virtual_manifest("vault", &manifest("vault"))
        .await
        .unwrap();
    assert!(drain(&mut rx).is_empty());

    engine
        .import_domain_files(
            "vault",
            &[("one.md".to_string(), engram("One", "one", "imported"))],
            false,
            false,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(
        (
            engram_of(&heard[0]).kind,
            engram_of(&heard[0]).path.as_str()
        ),
        (ChangeKind::Added, "one.md")
    );

    let current = engine.manifest_markdown("vault").await.unwrap();
    let revised = format!("{current}\n- one more routing line\n");
    engine
        .save_manifest("vault", &revised, &support::sha256_hex(current.as_bytes()))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(engram_of(&heard[0]).kind, ChangeKind::Modified);
}

/// Dropping a draft that stands over nothing (`clear_row`) announces a
/// draft-scoped `deleted` under the row's own permalink.
#[tokio::test]
async fn dropping_a_draft_only_engram_announces_deleted_with_its_permalink() {
    let (_tmp, engine, _scratch) = engine_fixture(true).await;
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    engine
        .write_engram_as(&write_params("Gamma", false), None, &ada)
        .await
        .unwrap();
    let mut rx = engine.changes().subscribe();
    engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "gamma".to_string(),
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
    let change = engram_of(&heard[0]);
    assert_eq!(
        (
            change.kind,
            change.permalink.as_str(),
            change.draft_of.as_deref()
        ),
        (ChangeKind::Deleted, "gamma", Some("ada"))
    );
}

/// The late cross-domain resolve at the end of a multi-domain sync binds a
/// reference into a domain synced later in the same run, and announces the
/// domain holding it.
#[tokio::test]
async fn a_late_cross_domain_resolve_announces_the_referring_domain() {
    let (tmp, engine, _scratch) = engine_fixture(false).await;
    seed(
        &tmp.path().join("notes"),
        "pointer.md",
        &engram("Pointer", "pointer", "- relates_to [[oak:acorn]]"),
    );
    seed(
        &tmp.path().join("oak"),
        "acorn.md",
        &engram("Acorn", "acorn", "An acorn."),
    );
    let mut rx = engine.changes().subscribe();
    engine.sync(None).await.unwrap();
    let heard = drain(&mut rx);
    assert!(
        heard
            .iter()
            .any(|c| matches!(c, Change::Domain(d) if d.domain == "notes")),
        "{heard:?}"
    );
}

/// Leaving review mode drops every draft, and each drop names the address
/// the owner's page is keyed on: the draft's own permalink, even where it
/// differs from the path's slug.
#[tokio::test]
async fn leaving_review_mode_announces_each_dropped_draft_under_its_own_permalink() {
    let (tmp, engine, _scratch) = engine_fixture(true).await;
    seed(
        &tmp.path().join("notes"),
        "odd.md",
        &engram("Odd", "not-the-slug", "A permalink of its own."),
    );
    engine
        .sync_paths("notes", vec!["odd.md".to_string()])
        .await
        .unwrap();
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    engine
        .edit_engram_as(&append_edit("not-the-slug", "ada's line"), None, &ada)
        .await
        .unwrap();
    let mut rx = engine.changes().subscribe();
    engine
        .set_review_mode(
            "notes",
            None,
            crystalline_service::ReviewModeConfirm::Confirmed {
                folds: vec![("ada".to_string(), crystalline_service::FoldChoice::Discard)],
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    let dropped = heard
        .iter()
        .find(|c| matches!(c, Change::Engram(e) if e.path == "odd.md"))
        .unwrap_or_else(|| panic!("{heard:?}"));
    let dropped = engram_of(dropped);
    assert_eq!(
        (
            dropped.kind,
            dropped.permalink.as_str(),
            dropped.draft_of.as_deref()
        ),
        (ChangeKind::Deleted, "not-the-slug", Some("ada"))
    );
}

/// The same rule on a virtual domain, whose write reads the row rather than
/// a file to tell a creation from a replacement.
#[tokio::test]
async fn a_virtual_overwrite_that_creates_is_added_and_a_replacement_is_modified() {
    let (_tmp, engine, _scratch) = engine_fixture(false).await;
    engine.domain_add_virtual("vault").await.unwrap();
    let vault = |title: &str| WriteParams {
        domain: "vault".to_string(),
        ..write_params(title, true)
    };
    let mut rx = engine.changes().subscribe();
    engine.write_engram(&vault("Gamma")).await.unwrap();
    assert_eq!(engram_of(&drain(&mut rx)[0]).kind, ChangeKind::Added);
    engine.write_engram(&vault("Gamma")).await.unwrap();
    assert_eq!(engram_of(&drain(&mut rx)[0]).kind, ChangeKind::Modified);
}

/// A write made inside somebody else's draft through a join names the
/// writer, while `draft_of` stays the draft's owner, whose sessions alone
/// hear it. Catches a joined view that labels the change with the owner.
#[tokio::test]
async fn a_joined_edit_names_the_writer_and_stays_the_owners_draft() {
    let (_tmp, engine, _scratch) = engine_fixture(true).await;
    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    engine
        .edit_engram_as(&append_edit("alpha", "ada's line"), None, &ada)
        .await
        .unwrap();
    let join = crystalline_service::Join {
        account: "bob".to_string(),
        holder: crystalline_service::Holder::Process(1),
        domain: "notes".to_string(),
        path: "alpha.md".to_string(),
        owner: "ada".to_string(),
        expires_at: None,
    };
    let bob = Scope::User {
        account: "bob".to_string(),
        admin: false,
    };
    let mut rx = engine.changes().subscribe();
    engine
        .edit_engram_joined(&append_edit("alpha", "bob's line"), None, &bob, Some(&join))
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert_eq!(heard.len(), 1, "{heard:?}");
    let change = engram_of(&heard[0]);
    assert_eq!(change.actor.as_deref(), Some("bob"));
    assert_eq!(change.draft_of.as_deref(), Some("ada"));
}

/// Item 12: a MANIFEST that starts declaring a name rebinds a link in
/// another domain, and that domain hears it without a refetch.
async fn a_domain_name_change_announces_the_linking_domain_body(store: Arc<Mutex<dyn Store>>) {
    let (tmp, engine, _scratch) = engine_fixture_on(store, false).await;
    seed(
        &tmp.path().join("notes"),
        "guide.md",
        &engram("Guide", "guide", "See [[platform:runbook]] first."),
    );
    seed(
        &tmp.path().join("oak"),
        "runbook.md",
        &engram("Runbook", "runbook", "Restart it."),
    );
    engine.sync(None).await.unwrap();
    let mut rx = engine.changes().subscribe();
    seed(
        &tmp.path().join("oak"),
        "MANIFEST.md",
        &manifest("oak").replacen(
            "status: stable\n",
            "status: stable\ndomain_name: platform\n",
            1,
        ),
    );
    engine
        .sync_paths("oak", vec!["MANIFEST.md".to_string()])
        .await
        .unwrap();
    engine.refresh_names().await;
    let heard = drain(&mut rx);
    assert!(
        heard.iter().any(
            |c| matches!(c, Change::Domain(DomainChanged { domain, .. }) if domain == "notes")
        ),
        "{heard:?}"
    );
}
both_backends!(
    a_domain_name_change_announces_the_linking_domain,
    a_domain_name_change_announces_the_linking_domain_body
);

/// Item 12: a retag that writes two engrams and then fails on the MANIFEST's
/// alias still announces the two it wrote.
#[cfg(unix)]
async fn a_retag_that_fails_halfway_announces_what_it_wrote_body(store: Arc<Mutex<dyn Store>>) {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, engine, _scratch) = engine_fixture_on(store, false).await;
    let notes = tmp.path().join("notes");
    let tagged = |title: &str, permalink: &str| {
        engram(title, permalink, "A rule.").replacen("  - t\n", "  - moving\n", 1)
    };
    seed(&notes, "topic/gamma.md", &tagged("Gamma", "topic/gamma"));
    seed(&notes, "topic/delta.md", &tagged("Delta", "topic/delta"));
    engine.sync(None).await.unwrap();
    let mut rx = engine.changes().subscribe();
    // The engrams sit in a folder that stays writable; the MANIFEST's alias
    // write needs a temp file beside it, in the root, which no longer takes one.
    let mode =
        |bits| std::fs::set_permissions(&notes, std::fs::Permissions::from_mode(bits)).unwrap();
    mode(0o555);
    let result = engine
        .retag("moving", "manifest", Some("notes"), true, false, true)
        .await;
    mode(0o755);
    let Err(_) = result else {
        eprintln!(
            "skipped: the MANIFEST could be written through a read-only folder (running as root?)"
        );
        return;
    };
    let mut heard: Vec<String> = drain(&mut rx)
        .iter()
        .map(|change| engram_of(change).path.clone())
        .collect();
    heard.sort();
    assert_eq!(
        heard,
        vec!["topic/delta.md".to_string(), "topic/gamma.md".to_string()]
    );
}
#[cfg(unix)]
both_backends!(
    a_retag_that_fails_halfway_announces_what_it_wrote,
    a_retag_that_fails_halfway_announces_what_it_wrote_body
);

/// Item 13: writing the engram a link in another domain waits for binds that
/// link at once, and that domain hears it.
async fn a_new_engram_binds_a_waiting_link_in_another_domain_and_announces_it_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (tmp, engine, _scratch) = engine_fixture_on(store, false).await;
    seed(
        &tmp.path().join("notes"),
        "guide.md",
        &engram("Guide", "guide", "- relates_to [[oak:Restart runbook]]"),
    );
    engine.sync(None).await.unwrap();
    let mut rx = engine.changes().subscribe();
    engine
        .write_engram(&WriteParams {
            domain: "oak".to_string(),
            ..write_params("Restart runbook", false)
        })
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(
        heard.iter().any(
            |c| matches!(c, Change::Domain(DomainChanged { domain, .. }) if domain == "notes")
        ),
        "{heard:?}"
    );
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "guide".to_string(),
                domain: Some("notes".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(read["relations"][0]["resolved"], true, "{read}");
}
both_backends!(
    a_new_engram_binds_a_waiting_link_in_another_domain_and_announces_it,
    a_new_engram_binds_a_waiting_link_in_another_domain_and_announces_it_body
);

/// A draft is one actor's: another domain's links never bind to it.
/// The store, not the call site, keeps a draft from being bound to: the
/// bind reads the base candidates alone (`ReferenceCandidates::Base`).
async fn a_draft_write_binds_nothing_elsewhere_body(store: Arc<Mutex<dyn Store>>) {
    let (tmp, engine, _scratch) = engine_fixture_on(store, true).await;
    seed(
        &tmp.path().join("oak"),
        "guide.md",
        &engram("Guide", "guide", "- relates_to [[notes:Restart runbook]]"),
    );
    engine.sync(None).await.unwrap();
    let mut rx = engine.changes().subscribe();
    let receipt = engine
        .write_engram_as(
            &write_params("Restart runbook", false),
            None,
            &Scope::User {
                account: "ada".to_string(),
                admin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(receipt["draft"], true, "{receipt}");
    let heard = drain(&mut rx);
    assert!(
        !heard
            .iter()
            .any(|c| matches!(c, Change::Domain(DomainChanged { domain, .. }) if domain == "oak")),
        "{heard:?}"
    );
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "guide".to_string(),
                domain: Some("oak".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(read["relations"][0]["resolved"], false, "{read}");
}
both_backends!(
    a_draft_write_binds_nothing_elsewhere,
    a_draft_write_binds_nothing_elsewhere_body
);

/// Whether `notes/guide`'s one relation is bound, as a read reports it.
async fn guide_resolved(engine: &Engine) -> serde_json::Value {
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "guide".to_string(),
                domain: Some("notes".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    read["relations"][0]["resolved"].clone()
}

/// Whether `heard` holds a `domain` frame for `notes`.
fn notes_heard(heard: &[Change]) -> bool {
    heard
        .iter()
        .any(|c| matches!(c, Change::Domain(DomainChanged { domain, .. }) if domain == "notes"))
}

/// `notes/guide` waits for `[[oak:<target>]]`, and `oak/runbook` (Runbook)
/// stands beside it, both synced.
async fn waiting_for(
    store: Arc<Mutex<dyn Store>>,
    target: &str,
) -> (tempfile::TempDir, Arc<Engine>, support::ScratchStateDir) {
    let (tmp, engine, scratch) = engine_fixture_on(store, false).await;
    seed(
        &tmp.path().join("notes"),
        "guide.md",
        &engram("Guide", "guide", &format!("- relates_to [[oak:{target}]]")),
    );
    seed(
        &tmp.path().join("oak"),
        "runbook.md",
        &engram("Runbook", "runbook", "The steps."),
    );
    engine.sync(None).await.unwrap();
    assert_eq!(guide_resolved(&engine).await, false, "the link waits");
    (tmp, engine, scratch)
}

/// An edit that retitles an engram binds a link elsewhere that waited for
/// the new title, and that domain hears it.
async fn an_edit_that_retitles_binds_a_waiting_link_in_another_domain_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (_tmp, engine, _scratch) = waiting_for(store, "Restart runbook").await;
    let mut rx = engine.changes().subscribe();
    engine
        .edit_engram(&EditParams {
            identifier: "runbook".to_string(),
            domain: "oak".to_string(),
            operation: "find_replace".to_string(),
            find_text: Some("title: Runbook".to_string()),
            content: Some("title: Restart runbook".to_string()),
            ..EditParams::default()
        })
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(notes_heard(&heard), "{heard:?}");
    assert_eq!(guide_resolved(&engine).await, true);
}
both_backends!(
    an_edit_that_retitles_binds_a_waiting_link_in_another_domain,
    an_edit_that_retitles_binds_a_waiting_link_in_another_domain_body
);

/// A save (the path a co-editing room's text lands through) that retitles
/// an engram binds a link elsewhere that waited for the new title.
async fn a_save_that_retitles_binds_a_waiting_link_in_another_domain_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (_tmp, engine, _scratch) = waiting_for(store, "Restart runbook").await;
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "runbook".to_string(),
                domain: Some("oak".to_string()),
                share_link: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let mut rx = engine.changes().subscribe();
    engine
        .save_engram(
            &SaveParams {
                domain: "oak".to_string(),
                identifier: "runbook".to_string(),
                content: engram("Restart runbook", "runbook", "The steps."),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(notes_heard(&heard), "{heard:?}");
    assert_eq!(guide_resolved(&engine).await, true);
}
both_backends!(
    a_save_that_retitles_binds_a_waiting_link_in_another_domain,
    a_save_that_retitles_binds_a_waiting_link_in_another_domain_body
);

/// A move to the permalink a link elsewhere waited for binds that link.
async fn a_move_to_a_waited_for_permalink_binds_a_link_in_another_domain_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (_tmp, engine, _scratch) = waiting_for(store, "restart-runbook").await;
    let mut rx = engine.changes().subscribe();
    engine
        .move_engram(
            &MoveParams {
                identifier: "runbook".to_string(),
                domain: "oak".to_string(),
                destination: "restart-runbook".to_string(),
                destination_domain: None,
                permalink: None,
                update_links: None,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(notes_heard(&heard), "{heard:?}");
    assert_eq!(guide_resolved(&engine).await, true);
}
both_backends!(
    a_move_to_a_waited_for_permalink_binds_a_link_in_another_domain,
    a_move_to_a_waited_for_permalink_binds_a_link_in_another_domain_body
);

/// A restore brings a deleted engram back, and a link elsewhere that lost
/// it binds again at once.
async fn a_restore_binds_a_waiting_link_in_another_domain_body(store: Arc<Mutex<dyn Store>>) {
    let (tmp, engine, _scratch) = waiting_for(store, "gamma").await;
    let content = engram("Gamma", "gamma", "Restored.");
    engine
        .restore_engram("oak", "gamma.md", &content, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        guide_resolved(&engine).await,
        true,
        "bound before the delete"
    );
    engine
        .delete_engram(&DeleteParams {
            identifier: "gamma".to_string(),
            domain: "oak".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    // The guide is written again while gamma is gone, so its link is
    // indexed afresh and waits.
    seed(
        &tmp.path().join("notes"),
        "guide.md",
        &engram(
            "Guide",
            "guide",
            "- relates_to [[oak:gamma]]\n\nStill waiting.",
        ),
    );
    engine.sync(None).await.unwrap();
    assert_eq!(guide_resolved(&engine).await, false, "the link waits");
    let mut rx = engine.changes().subscribe();
    engine
        .restore_engram("oak", "gamma.md", &content, &Scope::Unrestricted)
        .await
        .unwrap();
    let heard = drain(&mut rx);
    assert!(notes_heard(&heard), "{heard:?}");
    assert_eq!(guide_resolved(&engine).await, true);
}
both_backends!(
    a_restore_binds_a_waiting_link_in_another_domain,
    a_restore_binds_a_waiting_link_in_another_domain_body
);
