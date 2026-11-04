//! `write_engram`'s `expected_checksum`: an overwrite guarded by the checksum
//! a read handed out, on every storage kind the edit guard covers.
//!
//! Every token here comes from `read_engram` in the same scope the write runs
//! in, never from hashing bytes by hand: the contract is that the token a read
//! hands out is the one the write compares against.
//!
//! The live-editor case sits in `collab/collab_agent.rs`, beside the room
//! helpers it needs.

use std::path::PathBuf;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::{Store, TursoStore};
use crystalline_service::Scope;
use crystalline_service::engine::{Engine, EngineError};
use crystalline_service::params::{ReadParams, WriteParams};
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Engineering\n\n## When to Use\n\n- Route here for eng work\n";

const PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Plan\n\n- [decision] the first plan #eng\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    engine: Arc<Engine>,
}

/// One file domain `eng` with a MANIFEST and the engram `Plan` on disk,
/// synced, optionally in review mode.
async fn file_fixture(review: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("eng");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    std::fs::write(dir.join("plan.md"), PLAN).unwrap();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    let mut entry = DomainEntry::file(dir.clone());
    if review {
        entry.review = Some(ReviewMode::Overlay);
    }
    cfg.domains.insert("eng".to_string(), entry);
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Engine::new(store, cfg, None, None)
        .with_state_dir(root.join("state"))
        .with_origins_dir(root.join("origins"));
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    Fixture {
        _tmp: tmp,
        dir,
        engine,
    }
}

/// An engine over one virtual domain `notes` with `Plan` in it.
async fn virtual_fixture() -> Engine {
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Engine::new(store, cfg, None, None);
    engine
        .write_engram(&write("notes", "Plan", "- [decision] the first plan #eng"))
        .await
        .unwrap();
    engine
}

fn write(domain: &str, title: &str, content: &str) -> WriteParams {
    WriteParams {
        path: None,
        base64: None,
        domain: domain.to_string(),
        title: title.to_string(),
        content: Some(content.to_string()),
        folder: None,
        engram_type: None,
        tags: vec!["eng".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
        expected_checksum: None,
        share_link: None,
        model: None,
    }
}

/// An overwrite of `title` guarded by `checksum`.
fn guarded(domain: &str, title: &str, content: &str, checksum: &str) -> WriteParams {
    WriteParams {
        overwrite: true,
        expected_checksum: Some(checksum.to_string()),
        ..write(domain, title, content)
    }
}

async fn checksum_of(engine: &Engine, domain: &str, identifier: &str, scope: &Scope) -> String {
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
                ..ReadParams::default()
            },
            scope,
        )
        .await
        .unwrap();
    read["checksum"].as_str().unwrap().to_string()
}

async fn content_of(engine: &Engine, domain: &str, identifier: &str, scope: &Scope) -> String {
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: identifier.to_string(),
                domain: Some(domain.to_string()),
                ..ReadParams::default()
            },
            scope,
        )
        .await
        .unwrap();
    read["content"].as_str().unwrap().to_string()
}

fn assert_stale(result: Result<serde_json::Value, EngineError>) {
    match result {
        Err(EngineError::Conflict(message)) => {
            assert!(message.contains("re-read"), "{message}");
        }
        other => panic!("a stale overwrite must be refused as a conflict, got {other:?}"),
    }
}

fn alice() -> Scope {
    Scope::User {
        account: "alice".to_string(),
        admin: false,
    }
}

// --- file domain -------------------------------------------------------------

#[tokio::test]
async fn a_matching_checksum_lets_the_overwrite_land() {
    let f = file_fixture(false).await;
    let token = checksum_of(&f.engine, "eng", "plan", &Scope::Unrestricted).await;
    let receipt = f
        .engine
        .write_engram(&guarded(
            "eng",
            "Plan",
            "- [decision] the second plan #eng",
            &token,
        ))
        .await
        .expect("the token is current, so the overwrite lands");
    assert_eq!(receipt["action"], "written", "{receipt}");
    let on_disk = std::fs::read_to_string(f.dir.join("plan.md")).unwrap();
    assert!(on_disk.contains("the second plan"), "{on_disk}");
}

#[tokio::test]
async fn a_stale_checksum_refuses_the_overwrite_and_writes_nothing() {
    let f = file_fixture(false).await;
    let token = checksum_of(&f.engine, "eng", "plan", &Scope::Unrestricted).await;
    // Somebody else rewrites the engram after the read.
    f.engine
        .write_engram(&WriteParams {
            overwrite: true,
            ..write("eng", "Plan", "- [decision] another writer's plan #eng")
        })
        .await
        .unwrap();
    let before = std::fs::read_to_string(f.dir.join("plan.md")).unwrap();

    assert_stale(
        f.engine
            .write_engram(&guarded(
                "eng",
                "Plan",
                "- [decision] the stale plan #eng",
                &token,
            ))
            .await,
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.join("plan.md")).unwrap(),
        before,
        "nothing was written"
    );
}

#[tokio::test]
async fn a_checksum_for_an_engram_that_is_gone_is_a_conflict_and_creates_nothing() {
    let f = file_fixture(false).await;
    let token = checksum_of(&f.engine, "eng", "plan", &Scope::Unrestricted).await;
    std::fs::remove_file(f.dir.join("plan.md")).unwrap();
    f.engine.sync(None).await.unwrap();

    for overwrite in [true, false] {
        let result = f
            .engine
            .write_engram(&WriteParams {
                overwrite,
                ..guarded(
                    "eng",
                    "Plan",
                    "- [decision] a plan from an old read #eng",
                    &token,
                )
            })
            .await;
        match result {
            Err(EngineError::Conflict(message)) => {
                assert!(message.contains("gone"), "{message}");
            }
            other => panic!("overwrite={overwrite}: expected a conflict, got {other:?}"),
        }
        assert!(
            !f.dir.join("plan.md").exists(),
            "overwrite={overwrite}: nothing was created"
        );
    }
}

#[tokio::test]
async fn a_checksum_without_overwrite_still_needs_overwrite() {
    let f = file_fixture(false).await;
    let token = checksum_of(&f.engine, "eng", "plan", &Scope::Unrestricted).await;
    let err = f
        .engine
        .write_engram(&WriteParams {
            overwrite: false,
            ..guarded("eng", "Plan", "- [decision] the second plan #eng", &token)
        })
        .await
        .expect_err("an existing engram is replaced only with overwrite");
    assert!(err.to_string().contains("overwrite"), "{err}");
    let on_disk = std::fs::read_to_string(f.dir.join("plan.md")).unwrap();
    assert!(on_disk.contains("the first plan"), "{on_disk}");
}

#[tokio::test]
async fn without_a_checksum_an_overwrite_is_last_write_wins() {
    let f = file_fixture(false).await;
    for body in ["the second plan", "the third plan"] {
        f.engine
            .write_engram(&WriteParams {
                overwrite: true,
                ..write("eng", "Plan", &format!("- [decision] {body} #eng"))
            })
            .await
            .unwrap();
    }
    let on_disk = std::fs::read_to_string(f.dir.join("plan.md")).unwrap();
    assert!(on_disk.contains("the third plan"), "{on_disk}");
    // And a plain create of a new title still creates.
    let created = f
        .engine
        .write_engram(&write("eng", "Fresh", "- [idea] new #eng"))
        .await
        .unwrap();
    assert_eq!(created["action"], "created", "{created}");
}

#[tokio::test]
async fn a_checksum_on_a_file_upload_is_refused() {
    let f = file_fixture(false).await;
    let err = f
        .engine
        .upload_attachment(
            &WriteParams {
                path: Some("assets/notes.txt".to_string()),
                title: String::new(),
                tags: Vec::new(),
                expected_checksum: Some("0".repeat(64)),
                ..write("eng", "", "plain text")
            },
            &Scope::Unrestricted,
            None,
        )
        .await
        .expect_err("a checksum guards engram markdown only");
    assert!(
        matches!(err, EngineError::Invalid(_)),
        "a request error, not a conflict: {err:?}"
    );
    assert!(err.to_string().contains("engram markdown"), "{err}");
    assert!(!f.dir.join("assets/notes.txt").exists(), "nothing uploaded");
}

/// Two overwrites built from the same read, lined up behind the file's write
/// lock: exactly one lands and the other is refused, so the check runs under
/// the lock with the write rather than before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_overwrites_from_one_read_the_second_is_refused() {
    let f = file_fixture(false).await;
    let token = checksum_of(&f.engine, "eng", "plan", &Scope::Unrestricted).await;
    let lock = f.engine.file_write_lock(&f.dir.join("plan.md"));
    let held = lock.clone().lock_owned().await;
    let spawn = |body: &'static str| {
        let engine = f.engine.clone();
        let token = token.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&guarded(
                    "eng",
                    "Plan",
                    &format!("- [decision] {body} #eng"),
                    &token,
                ))
                .await
        })
    };
    let first = spawn("plan from writer one");
    let second = spawn("plan from writer two");
    // Both reach the lock before it is let go.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(held);
    let results = [first.await.unwrap(), second.await.unwrap()];
    let landed: Vec<usize> = (0..2).filter(|i| results[*i].is_ok()).collect();
    assert_eq!(landed.len(), 1, "exactly one lands: {results:?}");
    let refused = results.iter().find(|r| r.is_err()).unwrap();
    assert!(
        matches!(refused, Err(EngineError::Conflict(m)) if m.contains("re-read")),
        "{refused:?}"
    );
    let winner = if landed[0] == 0 {
        "writer one"
    } else {
        "writer two"
    };
    let on_disk = std::fs::read_to_string(f.dir.join("plan.md")).unwrap();
    assert!(
        on_disk.contains(winner),
        "the winner's text stands: {on_disk}"
    );
}

// --- virtual domain ----------------------------------------------------------

#[tokio::test]
async fn a_virtual_overwrite_is_guarded_by_the_rows_checksum() {
    let engine = virtual_fixture().await;
    let scope = Scope::Unrestricted;
    let token = checksum_of(&engine, "notes", "plan", &scope).await;
    engine
        .write_engram(&guarded(
            "notes",
            "Plan",
            "- [decision] the second plan #eng",
            &token,
        ))
        .await
        .expect("the token is current");
    assert!(
        content_of(&engine, "notes", "plan", &scope)
            .await
            .contains("the second plan")
    );

    // The token from before that write is stale now.
    assert_stale(
        engine
            .write_engram(&guarded(
                "notes",
                "Plan",
                "- [decision] the stale plan #eng",
                &token,
            ))
            .await,
    );
    assert!(
        content_of(&engine, "notes", "plan", &scope)
            .await
            .contains("the second plan")
    );

    // A row that is gone is a conflict and is not created again.
    let fresh = checksum_of(&engine, "notes", "plan", &scope).await;
    engine
        .delete_engram(&crystalline_service::params::DeleteParams {
            identifier: "plan".to_string(),
            domain: "notes".to_string(),
            expected_checksum: None,
        })
        .await
        .unwrap();
    match engine
        .write_engram(&guarded("notes", "Plan", "- [decision] back #eng", &fresh))
        .await
    {
        Err(EngineError::Conflict(message)) => assert!(message.contains("gone"), "{message}"),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

// --- review overlay ----------------------------------------------------------

/// In a domain that reviews changes the write lands in the caller's own
/// draft, so the token is the one the caller's own read hands out: the base
/// file's before the first draft, the draft's after it.
#[tokio::test]
async fn a_draft_overwrite_is_guarded_by_the_callers_own_view() {
    let f = file_fixture(true).await;
    let alice = alice();
    let base = std::fs::read_to_string(f.dir.join("plan.md")).unwrap();

    // Before any draft: the base text, read in alice's view.
    let token = checksum_of(&f.engine, "eng", "plan", &alice).await;
    let receipt = f
        .engine
        .write_engram_as(
            &guarded(
                "eng",
                "Plan",
                "- [decision] alice's first draft #eng",
                &token,
            ),
            None,
            &alice,
        )
        .await
        .expect("the token matches the base alice sees");
    assert_eq!(receipt["draft"], true, "{receipt}");

    // The base token is stale now: alice's view holds her draft.
    assert_stale(
        f.engine
            .write_engram_as(
                &guarded("eng", "Plan", "- [decision] from the old read #eng", &token),
                None,
                &alice,
            )
            .await,
    );
    assert!(
        content_of(&f.engine, "eng", "plan", &alice)
            .await
            .contains("alice's first draft")
    );

    // The draft's own token lets the next overwrite land.
    let draft_token = checksum_of(&f.engine, "eng", "plan", &alice).await;
    f.engine
        .write_engram_as(
            &guarded(
                "eng",
                "Plan",
                "- [decision] alice's second draft #eng",
                &draft_token,
            ),
            None,
            &alice,
        )
        .await
        .expect("the draft's token is current");
    assert!(
        content_of(&f.engine, "eng", "plan", &alice)
            .await
            .contains("alice's second draft")
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.join("plan.md")).unwrap(),
        base,
        "the folder still says what the team reviewed"
    );
}
