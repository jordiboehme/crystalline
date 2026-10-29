//! With reflection: on, no engram verb reaches reflections/.

use std::collections::BTreeMap;
use std::sync::Arc;

use crystalline_core::config::{GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::ShareActor;
use crystalline_service::params::EditParams;
use crystalline_service::{Engine, Scope};
use tokio::sync::Mutex;

use crate::fixture::{delete_params, engram, fixture, manifest, move_params, page, write_params};
use crate::support::MockProvider;

fn refusal(err: impl std::fmt::Display) -> String {
    let text = err.to_string();
    assert!(
        text.contains("Reflection"),
        "a teaching refusal, not a miss: {text}"
    );
    text
}

#[tokio::test]
async fn engram_verbs_refuse_the_reflection_folder() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        ("notes/n.md", &engram("N", "notes/n", "- [fact] n")),
        ("reflections/page.md", &page("Page")),
    ])
    .await;
    let e = &fx.engine;
    assert!(e.reflection_effective("eng").await.unwrap(), "in effect");
    refusal(
        e.write_engram(&write_params("Sneaky", Some("reflections"), "- [fact] x"))
            .await
            .unwrap_err(),
    );
    refusal(
        e.write_engram(&write_params(
            "Sneaky",
            Some("Reflections/deep"),
            "- [fact] x",
        ))
        .await
        .unwrap_err(),
    );
    refusal(
        e.move_engram(
            &move_params("notes/n", "reflections/n.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err(),
    );
    let edit = EditParams {
        domain: "eng".into(),
        identifier: "reflections/page".into(),
        operation: "append".into(),
        content: Some("more".into()),
        ..Default::default()
    };
    refusal(e.edit_engram(&edit).await.unwrap_err());
    refusal(
        e.delete_engram(&delete_params("reflections/page"))
            .await
            .unwrap_err(),
    );
    refusal(
        e.delete_engram(&delete_params("crystalline://eng/reflections/page.md"))
            .await
            .unwrap_err(),
    );
    assert!(
        fx.root.join("reflections/page.md").exists(),
        "nothing was touched"
    );
    assert!(fx.root.join("notes/n.md").exists(), "nothing was moved");
}

#[tokio::test]
async fn with_the_switch_off_the_folder_takes_engrams() {
    let fx = fixture(&[]).await;
    fx.engine
        .write_engram(&write_params(
            "Ordinary",
            Some("reflections"),
            "- [fact] just a folder name here",
        ))
        .await
        .unwrap();
    assert!(fx.root.join("reflections/ordinary.md").exists());
}

#[tokio::test]
async fn a_pending_switch_lets_the_engrams_move_out_and_nothing_move_in() {
    let fx = fixture(&[
        (
            "reflections/kept.md",
            &engram("Kept", "reflections/kept", "- [fact] kept"),
        ),
        ("notes/n.md", &engram("N", "notes/n", "- [fact] n")),
    ])
    .await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        !fx.engine.reflection_effective("eng").await.unwrap(),
        "pending"
    );
    refusal(
        fx.engine
            .move_engram(
                &move_params("notes/n", "reflections/n.md"),
                &Scope::Unrestricted,
            )
            .await
            .unwrap_err(),
    );
    refusal(
        fx.engine
            .write_engram(&write_params("New", Some("reflections"), "- [fact] x"))
            .await
            .unwrap_err(),
    );
    // The engram still there is an engram while the switch waits for it.
    let edit = EditParams {
        domain: "eng".into(),
        identifier: "reflections/kept".into(),
        operation: "append".into(),
        content: Some("- [fact] more".into()),
        ..Default::default()
    };
    fx.engine.edit_engram(&edit).await.unwrap();
    fx.engine
        .move_engram(
            &move_params("reflections/kept", "kept.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        fx.engine.reflection_effective("eng").await.unwrap(),
        "the last engram left"
    );
}

/// The identifier screen answers a miss only: an engram moved out of the
/// folder keeps a custom permalink that still starts with `reflections/`,
/// and it stays reachable by it once the Reflection is in effect.
#[tokio::test]
async fn an_engram_moved_out_keeps_answering_to_its_custom_permalink() {
    let fx = fixture(&[(
        "reflections/a.md",
        &engram("A", "reflections/guide-a", "- [fact] a"),
    )])
    .await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    let moved = fx
        .engine
        .move_engram(
            &move_params("reflections/guide-a", "notes/a.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(moved["to"]["permalink"], "reflections/guide-a", "{moved}");
    assert!(
        fx.engine.reflection_effective("eng").await.unwrap(),
        "in effect"
    );
    let edit = EditParams {
        domain: "eng".into(),
        identifier: "reflections/guide-a".into(),
        operation: "append".into(),
        content: Some("- [fact] more".into()),
        ..Default::default()
    };
    fx.engine.edit_engram(&edit).await.unwrap();
    fx.engine
        .delete_engram(&delete_params("reflections/guide-a"))
        .await
        .unwrap();
    assert!(!fx.root.join("notes/a.md").exists());
}

/// A row the index still holds for a file that became a page since the last
/// sync is not an engram any more: no verb acts on it through that row.
#[tokio::test]
async fn a_stale_row_of_a_page_is_refused_until_the_next_sync() {
    let fx = fixture(&[(
        "reflections/kept.md",
        &engram("Kept", "reflections/kept", "- [fact] kept"),
    )])
    .await;
    std::fs::write(fx.root.join("MANIFEST.md"), manifest("reflection: on\n")).unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        !fx.engine.reflection_effective("eng").await.unwrap(),
        "pending"
    );
    // Out of band, the file becomes a marked page; no sync runs.
    std::fs::write(fx.root.join("reflections/kept.md"), page("Kept")).unwrap();
    assert!(
        fx.engine.reflection_effective("eng").await.unwrap(),
        "in effect"
    );
    let edit = EditParams {
        domain: "eng".into(),
        identifier: "reflections/kept".into(),
        operation: "append".into(),
        content: Some("- [fact] more".into()),
        ..Default::default()
    };
    refusal(fx.engine.edit_engram(&edit).await.unwrap_err());
    refusal(
        fx.engine
            .move_engram(
                &move_params("reflections/kept", "notes/kept.md"),
                &Scope::Unrestricted,
            )
            .await
            .unwrap_err(),
    );
    refusal(
        fx.engine
            .delete_engram(&delete_params("reflections/kept"))
            .await
            .unwrap_err(),
    );
    assert_eq!(
        std::fs::read_to_string(fx.root.join("reflections/kept.md")).unwrap(),
        page("Kept"),
        "the page is untouched"
    );
}

#[tokio::test]
async fn index_files_skip_the_reflection_folder() {
    // The fixture's sync generates the folder listings (`index.files` is on
    // by default), the way crates/service/tests/engine/index_files.rs relies on.
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        ("reflections/guides/page.md", &page("Page")),
        ("notes/n.md", &engram("N", "notes/n", "- [fact] n")),
    ])
    .await;
    assert!(
        fx.root.join("index.md").exists(),
        "the root listing is there"
    );
    assert!(!fx.root.join("reflections/index.md").exists());
    assert!(!fx.root.join("reflections/guides/index.md").exists());
    let root_listing = std::fs::read_to_string(fx.root.join("index.md")).unwrap();
    assert!(!root_listing.contains("reflections/"), "{root_listing}");
}

/// While the switch is pending the engrams under `reflections/` are listed
/// in folder index files there; once the last one moves out and the switch
/// takes effect, those generated listings go too.
#[tokio::test]
async fn the_listings_of_a_pending_folder_go_once_the_switch_takes_effect() {
    let fx = fixture(&[
        ("MANIFEST.md", &manifest("reflection: on\n")),
        (
            "reflections/guides/old.md",
            &engram("Old", "reflections/guides/old", "- [fact] old"),
        ),
        ("notes/n.md", &engram("N", "notes/n", "- [fact] n")),
    ])
    .await;
    assert!(
        !fx.engine.reflection_effective("eng").await.unwrap(),
        "pending"
    );
    assert!(fx.root.join("reflections/index.md").exists());
    assert!(fx.root.join("reflections/guides/index.md").exists());
    fx.engine
        .move_engram(
            &move_params("reflections/guides/old", "notes/old.md"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    fx.engine.sync(None).await.unwrap();
    assert!(
        fx.engine.reflection_effective("eng").await.unwrap(),
        "in effect"
    );
    assert!(!fx.root.join("reflections/guides/index.md").exists());
    assert!(!fx.root.join("reflections/index.md").exists());
    let root_listing = std::fs::read_to_string(fx.root.join("index.md")).unwrap();
    assert!(!root_listing.contains("reflections/"), "{root_listing}");
}

#[tokio::test]
async fn a_restore_never_lands_an_engram_in_the_reflection_folder() {
    let fx = fixture(&[("MANIFEST.md", &manifest("reflection: on\n"))]).await;
    let err = fx
        .engine
        .restore_engram(
            "eng",
            "reflections/back.md",
            &engram("Back", "reflections/back", "- [fact] b"),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    refusal(err);
    assert!(!fx.root.join("reflections/back.md").exists());
}

/// A team MANIFEST that names the domain `eng`, with extra frontmatter lines.
fn team_manifest(extra: &str) -> Vec<u8> {
    format!(
        "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\ndomain_name: eng\n{extra}---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n"
    )
    .into_bytes()
}

/// The paths of `eng`'s unshared changes.
async fn change_paths(eng: &Engine) -> Vec<String> {
    let listed = eng.local_changes("eng", &ShareActor::Owner).await.unwrap();
    listed["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect()
}

/// A team domain `eng` whose origin holds the engram
/// `reflections/kept.md`, with the switch arrived out of band and the
/// engram edited since: the switch is pending, so the edit is a change.
async fn pending_team_domain(tmp: &std::path::Path) -> (Engine, std::path::PathBuf) {
    let mock = Arc::new(MockProvider::new());
    let tree: BTreeMap<String, Vec<u8>> = [
        ("MANIFEST.md".to_string(), team_manifest("")),
        (
            "reflections/kept.md".to_string(),
            engram("Kept", "reflections/kept", "- [fact] kept").into_bytes(),
        ),
    ]
    .into_iter()
    .collect();
    let commit = mock.add_commit(tree);
    mock.set_branch("main", &commit);
    let domains_root = tmp.join("domains");
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        domains_root: Some(domains_root.clone()),
        ..GlobalConfig::default()
    };
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(tmp.join("config.yaml")),
    )
    .with_origin_provider(mock)
    .with_origins_dir(tmp.join("origins"))
    .with_state_dir(tmp.to_path_buf());
    eng.origin_add("acme/eng-knowledge", None, None, None, None, None)
        .await
        .unwrap();
    let root = domains_root.join("eng");

    // The switch arrives out of band while the engram still lives there, and
    // the engram is edited: the switch is pending, so the edit is a change.
    std::fs::write(root.join("MANIFEST.md"), team_manifest("reflection: on\n")).unwrap();
    std::fs::write(
        root.join("reflections/kept.md"),
        engram("Kept", "reflections/kept", "- [fact] kept, and edited"),
    )
    .unwrap();
    eng.sync(None).await.unwrap();
    (eng, root)
}

/// Share changes work with the switch in effect, not the declared one: while
/// the switch waits for the engrams under `reflections/` to move out, they
/// are still engrams and still offered for a share; once it is in effect,
/// the Reflection's files are not.
#[tokio::test]
async fn a_pending_switch_still_offers_its_engrams_for_a_share() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, root) = pending_team_domain(tmp.path()).await;
    assert!(!eng.reflection_effective("eng").await.unwrap(), "pending");
    let pending = change_paths(&eng).await;
    assert!(
        pending.contains(&"reflections/kept.md".to_string()),
        "{pending:?}"
    );
    // The status count and its detail agree with the change list.
    let status = eng
        .origin_status(Some("eng"), true, false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &status["domains"][0];
    assert_eq!(
        entry["local_changes"].as_u64().unwrap() as usize,
        pending.len(),
        "{status}"
    );
    assert!(
        entry["detail"]["modified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "reflections/kept.md"),
        "{status}"
    );

    // The share carries it, and withdrawing that share with a revert puts
    // the engram back the way the team has it.
    let kept = vec!["reflections/kept.md".to_string()];
    let shared = eng
        .origin_share("eng", None, None, None, Some(&kept), ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(shared["updated"], serde_json::json!(kept), "{shared}");
    let number = shared["number"].as_u64().unwrap();
    let withdrawn = eng
        .origin_withdraw("eng", Some(number), true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(
        withdrawn["restored"],
        serde_json::json!(kept),
        "{withdrawn}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("reflections/kept.md")).unwrap(),
        engram("Kept", "reflections/kept", "- [fact] kept"),
    );

    // Moved out, the switch takes effect: a page there is the Reflection's.
    eng.move_engram(
        &move_params("reflections/kept", "kept.md"),
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    std::fs::write(root.join("reflections/new.md"), page("New")).unwrap();
    eng.sync(None).await.unwrap();
    assert!(eng.reflection_effective("eng").await.unwrap(), "in effect");
    let effective = change_paths(&eng).await;
    assert!(effective.contains(&"kept.md".to_string()), "{effective:?}");
    assert!(
        effective.iter().all(|p| !p.starts_with("reflections/")),
        "{effective:?}"
    );
}

/// A share made while the switch was pending, withdrawn with a revert once
/// the engram moved out and the switch took effect: the path under
/// `reflections/` is the Reflection's now, so the revert leaves it alone and
/// says so under `skipped_reflection`, not as a file that diverged.
#[tokio::test]
async fn a_withdraw_after_the_switch_took_effect_names_the_reflection_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (eng, root) = pending_team_domain(tmp.path()).await;
    let kept = vec!["reflections/kept.md".to_string()];
    let shared = eng
        .origin_share("eng", None, None, None, Some(&kept), ShareActor::Owner)
        .await
        .unwrap();
    let number = shared["number"].as_u64().unwrap();
    eng.move_engram(
        &move_params("reflections/kept", "kept.md"),
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    eng.sync(None).await.unwrap();
    assert!(eng.reflection_effective("eng").await.unwrap(), "in effect");
    let withdrawn = eng
        .origin_withdraw("eng", Some(number), true, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(
        withdrawn["skipped_diverged"],
        serde_json::json!([]),
        "{withdrawn}"
    );
    assert_eq!(
        withdrawn["skipped_reflection"],
        serde_json::json!(kept),
        "{withdrawn}"
    );
    assert_eq!(withdrawn["restored"], serde_json::json!([]), "{withdrawn}");
    assert!(
        !root.join("reflections/kept.md").exists(),
        "nothing is written into the Reflection's folder"
    );
    assert!(root.join("kept.md").exists());
}

/// Adding a domain from its origin over a folder that already holds one
/// counts the local changes with the Reflection mode in effect: the MANIFEST
/// says `reflection: on`, but an engram still lives under `reflections/`, so
/// the switch is pending and that engram is a change like any other.
#[tokio::test]
async fn adopting_a_folder_counts_a_pending_engram_as_a_change() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let tree: BTreeMap<String, Vec<u8>> =
        [("MANIFEST.md".to_string(), team_manifest("reflection: on\n"))]
            .into_iter()
            .collect();
    let commit = mock.add_commit(tree);
    mock.set_branch("main", &commit);
    let domains_root = tmp.path().join("domains");
    let root = domains_root.join("eng");
    std::fs::create_dir_all(root.join("reflections")).unwrap();
    std::fs::write(root.join("MANIFEST.md"), team_manifest("reflection: on\n")).unwrap();
    std::fs::write(
        root.join("reflections/note.md"),
        engram("Note", "reflections/note", "- [fact] mine"),
    )
    .unwrap();
    let cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        domains_root: Some(domains_root.clone()),
        ..GlobalConfig::default()
    };
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(tmp.path().join("config.yaml")),
    )
    .with_origin_provider(mock)
    .with_origins_dir(tmp.path().join("origins"))
    .with_state_dir(tmp.path().to_path_buf());
    let added = eng
        .origin_add("acme/eng-knowledge", None, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(added["adopted"], true, "{added}");
    assert_eq!(added["local_changes"], 1, "{added}");
}
