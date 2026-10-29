//! The backup branch.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crystalline_core::config::{GitHubConfig, OriginConfig, ServiceConfig};
use crystalline_remote::reflections_backup::{BACKUP_BRANCH, BackupOutcome};
use crystalline_service::engine::ConfigureAction;
use crystalline_service::reflection_backup::BackupCause;

use crate::docs::{agent, write};
use crate::fixture::{Fx, engram, fixture_with, manifest};
use crate::support::MockProvider;

const HOME: &str = "https://kb.example.com";

/// How the fixture is set up.
struct Home {
    /// Whether `eng` is connected to `acme/kb`.
    origin: bool,
    /// The `reflection_home` the MANIFEST declares, if any.
    home: Option<&'static str>,
    /// Extra MANIFEST lines.
    extra: &'static str,
    /// Whether `github.enabled` is on.
    github: bool,
    /// Whether the mock is injected as the origin provider; without it the
    /// engine reads an empty token store in the temp dir.
    inject: bool,
}

const CONNECTED: Home = Home {
    origin: true,
    home: Some(HOME),
    extra: "",
    github: true,
    inject: true,
};

async fn home_with(mock: Arc<MockProvider>, home: Home) -> Fx {
    let home_line = home
        .home
        .map(|url| format!("reflection_home: {url}\n"))
        .unwrap_or_default();
    let fx = fixture_with(
        &[
            (
                "MANIFEST.md",
                &manifest(&format!("reflection: on\n{home_line}{}", home.extra)),
            ),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        |cfg, _base| {
            if home.origin {
                cfg.domains.get_mut("eng").unwrap().origin = Some(OriginConfig {
                    repo: "acme/kb".into(),
                    path: None,
                    branch: Some("main".into()),
                    poll_secs: None,
                });
            }
            cfg.service = Some(ServiceConfig {
                public_url: Some(HOME.into()),
                ..Default::default()
            });
            if home.github {
                cfg.github = Some(GitHubConfig {
                    enabled: Some(true),
                    ..Default::default()
                });
            }
        },
    )
    .await;
    let engine = Arc::try_unwrap(fx.engine).ok().expect("one handle");
    let engine = if home.inject {
        engine.with_origin_provider(mock)
    } else {
        engine.with_token_store_dir(fx.tmp.path().join("tokens"))
    };
    Fx {
        engine: Arc::new(engine),
        ..fx
    }
}

async fn home(mock: Arc<MockProvider>) -> Fx {
    home_with(mock, CONNECTED).await
}

fn main_commit(mock: &MockProvider) {
    let files = BTreeMap::from([("runbooks/deploy.md".to_string(), b"engram".to_vec())]);
    let head = mock.add_commit(files);
    mock.set_branch("main", &head);
}

#[tokio::test]
async fn a_settled_change_is_committed_once_to_the_backup_branch() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n\n## One\n\nText.\n"), &agent())
        .await
        .unwrap();
    let results = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert_eq!(results.len(), 1, "{results:?}");
    let head = mock
        .branch_commit(BACKUP_BRANCH)
        .expect("the branch was created");
    assert!(
        mock.commit_paths(&head)
            .contains(&"reflections/a.md".to_string())
    );
    assert!(
        mock.commit_parents(&head).unwrap().is_empty(),
        "the first backup has no parent"
    );
    let calls = mock.calls().len();
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty(),
        "nothing marked, nothing done"
    );
    assert_eq!(mock.calls().len(), calls, "no forge call either");
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    let again = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(again[0].1, Ok(BackupOutcome::NothingToBackUp)),
        "{again:?}"
    );
    assert_eq!(
        mock.branch_commit(BACKUP_BRANCH).unwrap(),
        head,
        "no empty commit"
    );
}

#[tokio::test]
async fn the_backup_branch_never_receives_an_engram() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    fx.engine
        .write_reflection(&write("b.md", "# B\n"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    let paths = mock.commit_paths(&head);
    assert!(
        paths.iter().all(|p| p.starts_with("reflections/")),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"reflections/a.md".to_string())
            && paths.contains(&"reflections/b.md".to_string())
    );
    assert_eq!(
        mock.commit_parents(&head).unwrap().len(),
        1,
        "the second backup builds on the first"
    );
}

#[tokio::test]
async fn an_edit_waits_for_sixty_idle_seconds() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/typed.md"), "# Typed in a room\n").unwrap();
    let t0 = Instant::now();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Edit);
    assert!(
        fx.engine
            .reflection_backup_tick(t0 + Duration::from_secs(30))
            .await
            .is_empty()
    );
    assert_eq!(
        fx.engine
            .reflection_backup_tick(t0 + Duration::from_secs(61))
            .await
            .len(),
        1
    );
    assert!(mock.branch_commit(BACKUP_BRANCH).is_some());
}

#[tokio::test]
async fn a_file_dropped_by_hand_is_backed_up() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(
        fx.root.join("reflections/hand.md"),
        "# By hand\n\n## Part\n<!-- r:bbbb2222 -->\n\nText.\n",
    )
    .unwrap();
    let t0 = Instant::now();
    fx.engine
        .sync_paths("eng", vec!["reflections/hand.md".into()])
        .await
        .unwrap();
    assert!(
        fx.engine.reflection_backup_tick(t0).await.is_empty(),
        "an edit found by the watcher waits like a room's"
    );
    let done = fx
        .engine
        .reflection_backup_tick(t0 + Duration::from_secs(61))
        .await;
    assert_eq!(done.len(), 1, "{done:?}");
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    assert!(
        mock.commit_paths(&head)
            .contains(&"reflections/hand.md".to_string())
    );
}

#[tokio::test]
async fn an_offline_forge_keeps_the_change_marked_and_the_next_tick_commits_it_once() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    mock.set_offline_writes(true);
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    let t0 = Instant::now();
    let failed = fx.engine.reflection_backup_tick(t0).await;
    assert!(failed[0].1.is_err(), "{failed:?}");
    assert!(mock.branch_commit(BACKUP_BRANCH).is_none());
    mock.set_offline_writes(false);
    assert!(
        fx.engine
            .reflection_backup_tick(t0 + Duration::from_secs(10))
            .await
            .is_empty(),
        "waits before retrying"
    );
    let retried = fx
        .engine
        .reflection_backup_tick(t0 + Duration::from_secs(61))
        .await;
    assert!(retried[0].1.is_ok(), "{retried:?}");
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    assert!(
        mock.commit_paths(&head)
            .contains(&"reflections/a.md".to_string()),
        "nothing was lost"
    );
    assert!(
        fx.engine
            .reflection_backup_tick(t0 + Duration::from_secs(200))
            .await
            .is_empty(),
        "and it is committed once"
    );
}

/// A local Reflection (no origin, no home) is never backed up, with GitHub
/// collaboration on and a forge at hand.
#[tokio::test]
async fn a_local_reflection_has_no_backup() {
    let mock = Arc::new(MockProvider::new());
    let fx = home_with(
        mock.clone(),
        Home {
            origin: false,
            home: None,
            ..CONNECTED
        },
    )
    .await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty(),
        "no origin, no backup"
    );
    assert!(
        mock.calls().is_empty(),
        "and no forge call: {:?}",
        mock.calls()
    );
}

/// Only the home instance backs up: a domain connected to a repository
/// whose Reflection lives on another instance makes no forge call here.
#[tokio::test]
async fn a_reflection_homed_elsewhere_is_not_backed_up() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home_with(
        mock.clone(),
        Home {
            home: Some("https://other.example.com"),
            ..CONNECTED
        },
    )
    .await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/a.md"), "# A\n").unwrap();
    let calls = mock.calls().len();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty()
    );
    assert_eq!(mock.calls().len(), calls, "no forge call");
    assert!(mock.branch_commit(BACKUP_BRANCH).is_none());
}

/// With GitHub collaboration off nothing goes out and the marks wait: the
/// first tick after it is switched on commits what was marked before.
#[tokio::test]
async fn with_github_off_the_backup_waits_until_it_is_on() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home_with(
        mock.clone(),
        Home {
            github: false,
            ..CONNECTED
        },
    )
    .await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    let calls = mock.calls().len();
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty()
    );
    assert_eq!(mock.calls().len(), calls, "no forge call while it is off");
    fx.engine
        .configure(&ConfigureAction::Set {
            key: "github.enabled".into(),
            value: "true".into(),
        })
        .await
        .unwrap();
    let done = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(done[..], [(_, Ok(BackupOutcome::Committed { .. }))]),
        "{done:?}"
    );
}

/// Without a GitHub connection the backup waits quietly, like the origin
/// poller: no failed attempt, no forge call, the mark kept.
#[tokio::test]
async fn without_a_github_connection_the_backup_waits_quietly() {
    let mock = Arc::new(MockProvider::new());
    let fx = home_with(
        mock.clone(),
        Home {
            inject: false,
            ..CONNECTED
        },
    )
    .await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    let t0 = Instant::now();
    assert!(fx.engine.reflection_backup_tick(t0).await.is_empty());
    assert!(
        fx.engine
            .reflection_backup_tick(t0 + Duration::from_secs(3600))
            .await
            .is_empty()
    );
}

/// A rate limit starts the pause the origin poller shares (GitHub limits
/// the token, not the repository): the backup asks nothing until it ends.
#[tokio::test]
async fn a_rate_limit_pauses_the_backup() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    mock.rate_limit_branch_ref(Some(chrono::Utc::now() + chrono::Duration::hours(1)));
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    let t0 = Instant::now();
    let limited = fx.engine.reflection_backup_tick(t0).await;
    assert!(limited[0].1.is_err(), "{limited:?}");
    mock.rate_limit_branch_ref(None);
    let calls = mock.calls().len();
    assert!(
        fx.engine
            .reflection_backup_tick(t0 + Duration::from_secs(600))
            .await
            .is_empty(),
        "paused"
    );
    assert_eq!(mock.calls().len(), calls, "no forge call during the pause");
    assert!(mock.branch_commit(BACKUP_BRANCH).is_none());
}

/// The state file of `eng`'s backup.
fn state_file(fx: &Fx) -> std::path::PathBuf {
    fx.tmp.path().join("origins/eng/reflections-backup.json")
}

/// A crash between the branch move and the state save: the next backup
/// sees a head it did not write and rewrites every file on it, but sends
/// no delete for a path that head no longer holds.
#[tokio::test]
async fn a_crash_before_the_state_save_sends_no_delete_of_nothing() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    mock.refuse_deletes_of_nothing();
    let fx = home(mock.clone()).await;
    for page in ["a.md", "b.md"] {
        fx.engine
            .write_reflection(&write(page, "# Page\n"), &agent())
            .await
            .unwrap();
    }
    assert!(
        fx.engine.reflection_backup_tick(Instant::now()).await[0]
            .1
            .is_ok()
    );
    let before = std::fs::read(state_file(&fx)).unwrap();
    fx.engine
        .write_reflection(
            &crystalline_service::params::WriteReflectionParams {
                action: Some("delete".into()),
                ..write("b.md", "")
            },
            &agent(),
        )
        .await
        .unwrap();
    assert!(
        fx.engine.reflection_backup_tick(Instant::now()).await[0]
            .1
            .is_ok()
    );
    // The crash: the branch moved, the state still names the commit before.
    std::fs::write(state_file(&fx), before).unwrap();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    let after = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(after[0].1.is_ok(), "{after:?}");
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    let paths = mock.commit_paths(&head);
    assert!(paths.contains(&"reflections/a.md".to_string()), "{paths:?}");
    assert!(
        !paths.contains(&"reflections/b.md".to_string()),
        "{paths:?}"
    );
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    assert!(
        matches!(
            fx.engine.reflection_backup_tick(Instant::now()).await[0].1,
            Ok(BackupOutcome::NothingToBackUp)
        ),
        "and the state caught up"
    );
}

/// Two domains of one repository (two subpaths) share the backup branch.
/// A head the other domain moved does not make a domain upload every file
/// again: only what changed in its own Reflection goes out.
#[tokio::test]
async fn two_subpaths_on_one_branch_upload_only_their_own_changes() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = fixture_with(
        &[(
            "MANIFEST.md",
            &manifest(&format!("reflection: on\nreflection_home: {HOME}\n")),
        )],
        |cfg, base| {
            let ops = base.join("ops");
            std::fs::create_dir_all(&ops).unwrap();
            std::fs::write(
                ops.join("MANIFEST.md"),
                manifest(&format!("reflection: on\nreflection_home: {HOME}\n"))
                    .replace("title: eng", "title: ops"),
            )
            .unwrap();
            cfg.domains.insert(
                "ops".into(),
                crystalline_core::config::DomainEntry::file(ops),
            );
            for (name, path) in [("eng", "eng"), ("ops", "ops")] {
                cfg.domains.get_mut(name).unwrap().origin = Some(OriginConfig {
                    repo: "acme/kb".into(),
                    path: Some(path.into()),
                    branch: Some("main".into()),
                    poll_secs: None,
                });
            }
            cfg.service = Some(ServiceConfig {
                public_url: Some(HOME.into()),
                ..Default::default()
            });
            cfg.github = Some(GitHubConfig {
                enabled: Some(true),
                ..Default::default()
            });
        },
    )
    .await;
    let engine = Arc::try_unwrap(fx.engine).ok().expect("one handle");
    let fx = Fx {
        engine: Arc::new(engine.with_origin_provider(mock.clone())),
        ..fx
    };
    let in_domain = |domain: &str, page: &str, text: &str| {
        let mut p = write(page, text);
        p.domain = domain.into();
        p
    };
    for domain in ["eng", "ops"] {
        for page in ["one.md", "two.md", "three.md"] {
            fx.engine
                .write_reflection(&in_domain(domain, page, "# Page\n"), &agent())
                .await
                .unwrap();
        }
    }
    let first = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert_eq!(first.len(), 2, "{first:?}");
    assert!(first.iter().all(|(_, r)| r.is_ok()), "{first:?}");
    let blobs = |mock: &MockProvider| {
        mock.calls()
            .iter()
            .filter(|c| c.starts_with("create_blob:"))
            .count()
    };
    // `ops` moved the branch after `eng`'s backup; `eng` changes one page.
    // An agent's write is reflected, so the page's baseline moves with it.
    let before = blobs(&mock);
    fx.engine
        .write_reflection(&in_domain("eng", "one.md", "# Changed\n"), &agent())
        .await
        .unwrap();
    let again = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(
            again[..],
            [(_, Ok(BackupOutcome::Committed { files: 2, .. }))]
        ),
        "{again:?}"
    );
    assert_eq!(
        blobs(&mock),
        before + 2,
        "the changed page and its baseline alone"
    );
    // And `ops`, whose head `eng` just moved, has nothing to send.
    fx.engine.mark_reflection_dirty("ops", BackupCause::Settled);
    let ops = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(ops[..], [(_, Ok(BackupOutcome::NothingToBackUp))]),
        "{ops:?}"
    );
    assert_eq!(blobs(&mock), before + 2, "no upload for ops");
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    let paths = mock.commit_paths(&head);
    assert!(
        paths.contains(&"eng/reflections/one.md".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"ops/reflections/one.md".to_string()),
        "{paths:?}"
    );
}

/// A home with no repository has nowhere its Reflection may go: the tick
/// drops the mark and asks the forge nothing.
#[tokio::test]
async fn a_home_without_an_origin_makes_no_forge_call() {
    let mock = Arc::new(MockProvider::new());
    let fx = home_with(
        mock.clone(),
        Home {
            origin: false,
            ..CONNECTED
        },
    )
    .await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/a.md"), "# A\n").unwrap();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty()
    );
    assert!(mock.calls().is_empty(), "{:?}", mock.calls());
}

#[tokio::test]
async fn a_read_only_domain_is_not_backed_up() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home_with(
        mock.clone(),
        Home {
            extra: "read_only: on\n",
            ..CONNECTED
        },
    )
    .await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/a.md"), "# A\n").unwrap();
    let calls = mock.calls().len();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty()
    );
    assert_eq!(mock.calls().len(), calls, "no forge call");
    assert!(mock.branch_commit(BACKUP_BRANCH).is_none());
}

/// The same on a branch that already exists: the move of the branch fails,
/// and the retry still carries the change (the state is written only after
/// the branch moved).
#[tokio::test]
async fn a_failed_move_of_the_branch_loses_nothing() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    let t0 = Instant::now();
    assert!(fx.engine.reflection_backup_tick(t0).await[0].1.is_ok());
    let first = mock.branch_commit(BACKUP_BRANCH).unwrap();
    mock.set_offline_writes(true);
    fx.engine
        .write_reflection(&write("b.md", "# B\n"), &agent())
        .await
        .unwrap();
    let t1 = Instant::now();
    let failed = fx.engine.reflection_backup_tick(t1).await;
    assert!(failed[0].1.is_err(), "{failed:?}");
    assert_eq!(mock.branch_commit(BACKUP_BRANCH).unwrap(), first);
    mock.set_offline_writes(false);
    let retried = fx
        .engine
        .reflection_backup_tick(t1 + Duration::from_secs(61))
        .await;
    assert!(
        matches!(retried[0].1, Ok(BackupOutcome::Committed { .. })),
        "{retried:?}"
    );
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    assert!(
        mock.commit_paths(&head)
            .contains(&"reflections/b.md".to_string()),
        "nothing was lost"
    );
    assert_eq!(mock.commit_parents(&head).unwrap(), vec![first]);
    assert!(
        fx.engine
            .reflection_backup_tick(t1 + Duration::from_secs(200))
            .await
            .is_empty(),
        "and it is committed once"
    );
}

/// A backup still pending when its domain is renamed goes along under the
/// new name, with the backup state that moved with the domain.
#[tokio::test]
async fn a_pending_backup_follows_a_domain_rename() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n"), &agent())
        .await
        .unwrap();
    fx.engine
        .rename_domain(
            "eng",
            "platform",
            false,
            &crystalline_service::Scope::Unrestricted,
        )
        .await
        .unwrap();
    let done = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(&done[..], [(name, Ok(BackupOutcome::Committed { .. }))] if name == "platform"),
        "{done:?}"
    );
}

/// The daemon's start marks every Reflection once, so a change the marks
/// lost in a restart is backed up without waiting for the next one.
#[tokio::test]
async fn the_start_marks_every_reflection_once() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    std::fs::create_dir_all(fx.root.join("reflections")).unwrap();
    std::fs::write(fx.root.join("reflections/before.md"), "# Before\n").unwrap();
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty(),
        "nothing marked yet"
    );
    fx.engine.mark_every_reflection_for_backup();
    let done = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(done[..], [(_, Ok(BackupOutcome::Committed { .. }))]),
        "{done:?}"
    );
}

/// An upload, a move and a removal of a Reflection's file are settled
/// changes: each reaches the backup branch byte for byte, and the old path
/// leaves it.
#[tokio::test]
async fn the_backup_branch_carries_the_reflection_files_byte_for_byte() {
    // A file change marks the backup and never the evolve sweep's state
    // file; held so a regression that writes it stays out of the
    // developer's state directory.
    let _state = crate::support::ScratchStateDir::acquire();
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let fx = home(mock.clone()).await;
    let png = crate::assets::PNG;
    fx.engine
        .reflection_asset_write("eng", "assets/diagrams/flow.png", png.to_vec(), &agent())
        .await
        .unwrap();
    let results = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert_eq!(
        results.len(),
        1,
        "an upload is a settled change: {results:?}"
    );
    let head = mock.branch_commit(BACKUP_BRANCH).unwrap();
    assert_eq!(
        mock.commit_file(&head, "reflections/assets/diagrams/flow.png")
            .as_deref(),
        Some(png)
    );
    fx.engine
        .move_reflection_asset(
            "eng",
            "assets/diagrams/flow.png",
            "assets/flow.png",
            &agent(),
        )
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let paths = mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(
        paths.contains(&"reflections/assets/flow.png".to_string()),
        "{paths:?}"
    );
    assert!(
        !paths.contains(&"reflections/assets/diagrams/flow.png".to_string()),
        "{paths:?}"
    );
    fx.engine
        .reflection_asset_delete("eng", "assets/flow.png", &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let paths = mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(
        !paths.iter().any(|p| p.starts_with("reflections/assets/")),
        "{paths:?}"
    );
}

/// [`home`] beside a shared `harbor` with one engram, keeper as an
/// account, and the resolver installed. With `harbor_on_github`, `harbor`
/// is connected to its own repository too.
async fn home_beside_harbor(
    mock: Arc<MockProvider>,
    harbor_on_github: bool,
) -> (Fx, Arc<crystalline_service::rest::AuthStore>) {
    let fx = fixture_with(
        &[
            (
                "MANIFEST.md",
                &manifest(&format!("reflection: on\nreflection_home: {HOME}\n")),
            ),
            (
                "runbooks/deploy.md",
                &engram("Deploy", "runbooks/deploy", "- [fact] deploys run at night"),
            ),
        ],
        |cfg, base| {
            cfg.domains.get_mut("eng").unwrap().origin = Some(OriginConfig {
                repo: "acme/kb".into(),
                path: None,
                branch: Some("main".into()),
                poll_secs: None,
            });
            cfg.service = Some(ServiceConfig {
                public_url: Some(HOME.into()),
                ..Default::default()
            });
            cfg.github = Some(GitHubConfig {
                enabled: Some(true),
                ..Default::default()
            });
            let root = base.join("harbor");
            std::fs::create_dir_all(root.join("team")).unwrap();
            std::fs::write(
                root.join("MANIFEST.md"),
                crystalline_core::manifest_template("harbor", "2026-01-01"),
            )
            .unwrap();
            std::fs::write(
                root.join("team/harbor-team.md"),
                engram("Harbor team", "team/harbor-team", "- [fact] five people"),
            )
            .unwrap();
            let mut entry = crystalline_core::config::DomainEntry::file(root);
            if harbor_on_github {
                entry.origin = Some(OriginConfig {
                    repo: "acme/harbor".into(),
                    path: None,
                    branch: Some("main".into()),
                    poll_secs: None,
                });
            }
            cfg.domains.insert("harbor".into(), entry);
        },
    )
    .await;
    let engine = Arc::try_unwrap(fx.engine).ok().expect("one handle");
    let engine = engine.with_origin_provider(mock);
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&fx.tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user(
        "keeper",
        "keeper",
        None,
        crystalline_service::rest::Role::Editor,
        "pw12345678",
    )
    .await
    .unwrap();
    engine.set_domain_access(Arc::new(crystalline_service::DomainAccess::new(
        auth.clone(),
    )));
    (
        Fx {
            engine: Arc::new(engine),
            ..fx
        },
        auth,
    )
}

/// A page that cites a domain fewer people may read than its own is held
/// back from the backup branch (with its baseline), and the branch head
/// loses the copy it had; the hold is said. Once a curator takes the source
/// out, the page goes to the branch again.
#[tokio::test]
async fn a_page_with_a_too_wide_source_is_held_back_until_it_is_settled() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    let page = "# Wide\n\nFor people.\n\n## Team\n\nFive people.\n";
    let mut wide = write("wide.md", page);
    wide.sources = BTreeMap::from([(
        "Team".to_string(),
        vec!["crystalline://harbor/team/harbor-team".to_string()],
    )]);
    fx.engine.write_reflection(&wide, &agent()).await.unwrap();
    fx.engine
        .write_reflection(&write("a.md", "# A\n\n## One\n\nText.\n"), &agent())
        .await
        .unwrap();
    let head_paths = || mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(head_paths().contains(&"reflections/wide.md".to_string()));
    assert!(fx.engine.reflection_backup_held_back("eng").is_empty());

    // harbor narrows: the next backup leaves the page and its baseline out.
    auth.set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    let now = Instant::now();
    let held = fx.engine.reflection_backup_tick(now).await;
    assert_eq!(held.len(), 1, "{held:?}");
    let paths = head_paths();
    assert!(!paths.iter().any(|p| p.contains("wide.md")), "{paths:?}");
    assert!(paths.contains(&"reflections/a.md".to_string()), "{paths:?}");
    assert_eq!(
        fx.engine.reflection_backup_held_back("eng"),
        vec!["wide.md".to_string()]
    );
    // The hold is looked at again later without a new change.
    assert!(fx.engine.reflection_backup_tick(now).await.is_empty());
    assert_eq!(
        fx.engine
            .reflection_backup_tick(now + Duration::from_secs(3600))
            .await
            .len(),
        1
    );

    // Settled: the source and its text are out, and the page goes back.
    let mut settled = write("wide.md", "# Wide\n\nFor people.\n\n## Team\n\nA team.\n");
    settled.sources = BTreeMap::from([("Team".to_string(), vec!["runbooks/deploy".to_string()])]);
    fx.engine
        .write_reflection(&settled, &agent())
        .await
        .unwrap();
    let done = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(
        matches!(done[..], [(_, Ok(BackupOutcome::Committed { .. }))]),
        "{done:?}"
    );
    assert!(head_paths().contains(&"reflections/wide.md".to_string()));
    assert!(fx.engine.reflection_backup_held_back("eng").is_empty());
}

/// The page of `eng` (connected to GitHub) whose `Team` section cites
/// harbor's engram.
fn citing_harbor(path: &str) -> crystalline_service::params::WriteReflectionParams {
    let mut p = write(path, "# Wide\n\nFor people.\n\n## Team\n\nFive people.\n");
    p.sources = BTreeMap::from([(
        "Team".to_string(),
        vec!["crystalline://harbor/team/harbor-team".to_string()],
    )]);
    p
}

/// A page of a domain backed up to a GitHub repository may not cite a
/// domain that is not connected to GitHub, even when the accounts allow it
/// (both domains are shared here): personal knowledge never reaches a
/// repository.
#[tokio::test]
async fn a_github_page_cannot_cite_a_local_domain() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, _) = home_beside_harbor(mock, false).await;
    let err = fx
        .engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("this source is in a domain that is not shared through GitHub, and this page is backed up to a GitHub repository"),
        "{err}"
    );
    assert!(!fx.root.join("reflections/wide.md").exists());
}

#[tokio::test]
async fn a_github_page_can_cite_another_github_domain() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, _) = home_beside_harbor(mock, true).await;
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
}

/// A page that cites a local domain from a GitHub domain anyway (here
/// edited on disk) is named in the queue and held back from the backup.
#[tokio::test]
async fn an_existing_github_page_citing_a_local_domain_is_named_and_held_back() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, _) = home_beside_harbor(mock.clone(), false).await;
    fx.engine
        .write_reflection(&write("a.md", "# A\n\n## One\n\nText.\n"), &agent())
        .await
        .unwrap();
    std::fs::write(
        fx.root.join("reflections/forged.md"),
        "---\nreflection:\n  aaaa1111:\n    sources:\n    - domain: harbor\n      engram: team/harbor-team\n      checksum: abc\n---\n\n# Forged\n\n## Team\n<!-- r:aaaa1111 -->\n\nFive.\n",
    )
    .unwrap();
    fx.engine.sync(None).await.unwrap();
    let queue = fx
        .engine
        .reflect(
            &crystalline_service::params::ReflectParams {
                domains: vec!["eng".into()],
                limit: Some(100),
                ..Default::default()
            },
            &agent(),
        )
        .await
        .unwrap();
    let items: Vec<&serde_json::Value> = queue["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["what"] == "source_too_wide")
        .collect();
    assert_eq!(items.len(), 1, "{queue}");
    assert_eq!(items[0]["path"], "forged.md", "{queue}");

    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let paths = mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(paths.contains(&"reflections/a.md".to_string()), "{paths:?}");
    assert!(!paths.iter().any(|p| p.contains("forged.md")), "{paths:?}");
    assert_eq!(
        fx.engine.reflection_backup_held_back("eng"),
        vec!["forged.md".to_string()]
    );
}

/// A narrowing marks nothing in the Reflection, so the daemon's periodic
/// recount (and every accounts change through the JSON API) looks for pages
/// that newly cite a domain fewer people may read and schedules a backup,
/// which drops them off the branch head at once.
#[tokio::test]
async fn a_narrowing_schedules_a_backup_that_drops_the_page() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let head_paths = || mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(head_paths().contains(&"reflections/wide.md".to_string()));

    auth.set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    assert!(
        fx.engine
            .reflection_backup_tick(Instant::now())
            .await
            .is_empty(),
        "nothing marked by the narrowing itself"
    );
    assert_eq!(
        fx.engine.schedule_too_wide_backups().await,
        vec!["eng".to_string()]
    );
    let done = fx.engine.reflection_backup_tick(Instant::now()).await;
    assert_eq!(done.len(), 1, "{done:?}");
    assert!(!head_paths().iter().any(|p| p.contains("wide.md")));
    // Held now: the next look schedules nothing new.
    assert!(fx.engine.schedule_too_wide_backups().await.is_empty());

    // The periodic recount runs the same look.
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    auth.set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    fx.engine.record_reflect_counts().await.unwrap();
    assert_eq!(
        fx.engine.reflection_backup_tick(Instant::now()).await.len(),
        1
    );
}

/// The files only a held page links are held back with it and go out again
/// with it; a file a page that is not held links too stays on the branch.
#[tokio::test]
async fn a_held_page_holds_back_the_files_only_it_links() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    for asset in ["assets/only.png", "assets/shared.png"] {
        fx.engine
            .reflection_asset_write("eng", asset, crate::assets::PNG.to_vec(), &agent())
            .await
            .unwrap();
    }
    let mut wide = write(
        "wide.md",
        "# Wide\n\nFor people.\n\n## Team\n\nFive people.\n\n![only](assets/only.png)\n\n![shared](assets/shared.png)\n",
    );
    wide.sources = BTreeMap::from([(
        "Team".to_string(),
        vec!["crystalline://harbor/team/harbor-team".to_string()],
    )]);
    fx.engine.write_reflection(&wide, &agent()).await.unwrap();
    fx.engine
        .write_reflection(
            &write("a.md", "# A\n\n## One\n\n![shared](assets/shared.png)\n"),
            &agent(),
        )
        .await
        .unwrap();
    let head_paths = || mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(head_paths().contains(&"reflections/assets/only.png".to_string()));

    auth.set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    fx.engine.mark_reflection_dirty("eng", BackupCause::Settled);
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let paths = head_paths();
    assert!(
        !paths.contains(&"reflections/assets/only.png".to_string()),
        "{paths:?}"
    );
    assert!(
        paths.contains(&"reflections/assets/shared.png".to_string()),
        "{paths:?}"
    );

    let mut settled = write(
        "wide.md",
        "# Wide\n\nFor people.\n\n## Team\n\nA team.\n\n![only](assets/only.png)\n",
    );
    settled.sources = BTreeMap::from([("Team".to_string(), vec!["runbooks/deploy".to_string()])]);
    fx.engine
        .write_reflection(&settled, &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    assert!(head_paths().contains(&"reflections/assets/only.png".to_string()));
}

/// Serve `fx`'s engine over HTTP with `auth`, adding `boss`, an instance
/// admin: the JSON API whose accounts routes run the narrowing hook.
async fn serve_backup_home(
    fx: &Fx,
    auth: Arc<crystalline_service::rest::AuthStore>,
) -> std::net::SocketAddr {
    auth.add_user(
        "boss",
        "boss",
        None,
        crystalline_service::rest::Role::Admin,
        "pw12345678",
    )
    .await
    .unwrap();
    let router = crystalline_service::daemon::http_router(
        fx.engine.clone(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        &[],
        auth,
        None,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// Tick until a backup ran (the hook schedules it off the request), for at
/// most five seconds; the paths of the branch head then.
async fn backed_up_after_hook(fx: &Fx, mock: &MockProvider) -> Vec<String> {
    let start = Instant::now();
    while fx
        .engine
        .reflection_backup_tick(Instant::now())
        .await
        .is_empty()
    {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "no backup was scheduled"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap())
}

/// A domain made private through the JSON API schedules the backup that
/// drops a page citing it from the branch head, with no edit and no wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_visibility_change_through_the_api_drops_the_page() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let addr = serve_backup_home(&fx, auth).await;
    let boss = crate::rest::Session::login(addr, "boss").await;
    let (status, body) = boss
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/harbor/visibility",
            Some(serde_json::json!({ "private": true })),
        )
        .await;
    assert_eq!(status, 204, "{body}");
    let paths = backed_up_after_hook(&fx, &mock).await;
    assert!(!paths.iter().any(|p| p.contains("wide.md")), "{paths:?}");
}

/// A member added through the JSON API to the page's private domain, who
/// may not read the cited domain, schedules the backup that drops the page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_membership_change_through_the_api_drops_the_page() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    auth.add_user(
        "sam",
        "sam",
        None,
        crystalline_service::rest::Role::Viewer,
        "pw12345678",
    )
    .await
    .unwrap();
    for domain in ["eng", "harbor"] {
        auth.set_domain_visibility(domain, true, "keeper")
            .await
            .unwrap();
    }
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let addr = serve_backup_home(&fx, auth).await;
    let keeper = crate::rest::Session::login(addr, "keeper").await;
    let (status, body) = keeper
        .send(
            reqwest::Method::PUT,
            "/api/v1/domains/eng/members/sam",
            Some(serde_json::json!({ "level": "viewer" })),
        )
        .await;
    assert_eq!(status, 204, "{body}");
    let paths = backed_up_after_hook(&fx, &mock).await;
    assert!(!paths.iter().any(|p| p.contains("wide.md")), "{paths:?}");
}

/// The CLI's accounts commands run in their own process and tell a running
/// daemon over the control socket (`readers_changed`), which schedules the
/// backup that drops the page.
#[tokio::test]
async fn the_control_socket_hook_drops_the_page() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    auth.set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    let answer = crystalline_service::control::answer_for_test(
        fx.engine.clone(),
        &serde_json::json!({ "v": 1, "cmd": "readers_changed" }),
    )
    .await;
    assert_eq!(answer["ok"], true, "{answer}");
    let paths = backed_up_after_hook(&fx, &mock).await;
    assert!(!paths.iter().any(|p| p.contains("wide.md")), "{paths:?}");
}

/// A held page is looked at again after the recheck wait with no change
/// marked: once its source domain is shared again it goes back.
#[tokio::test]
async fn a_held_page_goes_back_after_the_recheck_once_nothing_holds_it() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, auth) = home_beside_harbor(mock.clone(), true).await;
    fx.engine
        .write_reflection(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap();
    fx.engine
        .write_reflection(&write("a.md", "# A\n\n## One\n\nText.\n"), &agent())
        .await
        .unwrap();
    auth.set_domain_visibility("harbor", true, "keeper")
        .await
        .unwrap();
    let now = Instant::now();
    fx.engine.reflection_backup_tick(now).await;
    let head_paths = || mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(!head_paths().iter().any(|p| p.contains("wide.md")));
    auth.set_domain_visibility("harbor", false, "keeper")
        .await
        .unwrap();
    let recheck = crystalline_service::reflection_backup::HELD_RECHECK;
    assert!(
        fx.engine
            .reflection_backup_tick(now + recheck - Duration::from_secs(1))
            .await
            .is_empty(),
        "not before the wait"
    );
    assert_eq!(
        fx.engine.reflection_backup_tick(now + recheck).await.len(),
        1
    );
    assert!(head_paths().contains(&"reflections/wide.md".to_string()));
    assert!(fx.engine.reflection_backup_held_back("eng").is_empty());
}

/// A local domain's text on a page backed up to GitHub, shared on purpose:
/// the question names the repository the page is backed up to, and once a
/// person confirmed it the page is backed up like any other.
#[tokio::test]
async fn a_confirmed_local_source_names_the_repository_and_is_backed_up() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, _) = home_beside_harbor(mock.clone(), false).await;
    let ask = fx
        .engine
        .reflection_disclosure_question(&citing_harbor("wide.md"), &agent())
        .await
        .unwrap()
        .expect("a local source on a GitHub page is a question");
    assert!(
        ask.question.contains("GitHub repository acme/kb"),
        "{}",
        ask.question
    );
    let mut confirmed = citing_harbor("wide.md");
    confirmed.disclosed = Some(ask.basis);
    fx.engine
        .write_reflection(&confirmed, &agent())
        .await
        .unwrap();
    fx.engine.reflection_backup_tick(Instant::now()).await;
    let head = mock.commit_paths(&mock.branch_commit(BACKUP_BRANCH).unwrap());
    assert!(
        head.contains(&"reflections/wide.md".to_string()),
        "{head:?}"
    );
    assert!(fx.engine.reflection_backup_held_back("eng").is_empty());
}

/// A run on a domain backed up to GitHub reads no domain that is not
/// connected to GitHub, though the accounts would let it: its pages go to a
/// repository whose readers are not the accounts here.
#[tokio::test]
async fn a_run_on_a_github_domain_reads_no_local_domain() {
    let mock = Arc::new(MockProvider::new());
    main_commit(&mock);
    let (fx, _) = home_beside_harbor(mock, false).await;
    let owner = crystalline_service::Scope::Unrestricted;
    let limit = fx
        .engine
        .read_limit_for(&owner, true, Some("eng"))
        .await
        .unwrap();
    assert!(limit.contains("harbor"), "{limit:?}");
    assert!(!limit.contains("eng"), "{limit:?}");
}
