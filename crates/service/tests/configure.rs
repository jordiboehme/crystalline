//! Engine-level tests for `Engine::configure`: show, set and unset against a
//! real config file, the read-only refusal and the in-memory config staying
//! coherent for later reads. There is no ctl-level (socket) test harness in
//! this crate yet (`control.rs` has no dedicated test file), so this exercises
//! the engine method directly, one layer below the ctl `configure` command
//! that just forwards to it.

use std::sync::Arc;

use crystalline_core::config::GlobalConfig;
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::engine::ConfigureAction;
use tokio::sync::Mutex;

async fn engine_at(config_path: &std::path::Path, read_only: bool) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        GlobalConfig::default(),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_read_only(read_only)
}

fn settings_of(data: &serde_json::Value) -> Vec<crystalline_service::settings::SettingView> {
    serde_json::from_value(data["settings"].clone()).unwrap()
}

#[tokio::test]
async fn show_lists_every_registry_key_at_its_default() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    let data = engine.configure(&ConfigureAction::Show).await.unwrap();
    let views = settings_of(&data);
    assert_eq!(views.len(), 8);
    assert!(
        views
            .iter()
            .all(|v| v.source == crystalline_service::settings::SettingSource::Default)
    );
    assert_eq!(views[0].key, "github.enabled");
    assert_eq!(views[0].value, "false");
}

#[tokio::test]
async fn set_persists_to_the_config_file_and_updates_the_in_memory_config() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    let data = engine
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "true".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(data["key"], "github.enabled");
    assert_eq!(data["value"], "true");
    assert_eq!(data["source"], "config");

    // The file on disk carries the change...
    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    assert!(on_disk.github_enabled());

    // ...and so does this engine's in-memory config, without reconstructing it.
    assert!(engine.config().github_enabled());
}

#[tokio::test]
async fn unset_returns_to_default_and_the_file_drops_an_emptied_github_block() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    engine
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "true".to_string(),
        })
        .await
        .unwrap();
    let data = engine
        .configure(&ConfigureAction::Unset {
            key: "github.enabled".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(data["value"], "false");
    assert_eq!(data["source"], "default");

    assert!(!engine.config().github_enabled());
    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !raw.contains("github"),
        "an emptied github block must not round-trip into the saved file: {raw}"
    );
}

#[tokio::test]
async fn set_on_an_unknown_key_lists_every_known_key_and_does_not_write_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    let err = engine
        .configure(&ConfigureAction::Set {
            key: "github.bogus".to_string(),
            value: "x".to_string(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("github.enabled"), "{err}");
    assert!(!config_path.exists(), "an invalid key must not touch disk");
}

#[tokio::test]
async fn read_only_refuses_set_and_unset_but_allows_show() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, true).await;

    let show = engine.configure(&ConfigureAction::Show).await;
    assert!(show.is_ok(), "show is always allowed, even read-only");

    let set_err = engine
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "true".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        set_err,
        crystalline_service::EngineError::ReadOnly
    ));

    let unset_err = engine
        .configure(&ConfigureAction::Unset {
            key: "github.enabled".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        unset_err,
        crystalline_service::EngineError::ReadOnly
    ));
    assert!(
        !config_path.exists(),
        "a refused set/unset must never touch disk"
    );
}

#[tokio::test]
async fn set_fails_to_persist_leaves_in_memory_config_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    let before = engine.configure(&ConfigureAction::Show).await.unwrap();
    let before_views = settings_of(&before);
    assert!(
        !before_views
            .iter()
            .any(|v| v.key == "github.enabled" && v.value == "true")
    );

    let unwritable_path = tmp.path().join("subdir");
    std::fs::create_dir(&unwritable_path).unwrap();
    let engine_with_dir_path = {
        let store = TursoStore::open_in_memory().await.unwrap();
        Engine::new(
            Arc::new(Mutex::new(store)),
            GlobalConfig::default(),
            None,
            Some(unwritable_path), // point to a directory, not a file
        )
    };

    let err = engine_with_dir_path
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "true".to_string(),
        })
        .await;
    assert!(
        err.is_err(),
        "persist must fail when config_path is a directory"
    );

    let after = engine_with_dir_path
        .configure(&ConfigureAction::Show)
        .await
        .unwrap();
    let after_views = settings_of(&after);
    assert_eq!(
        after_views
            .iter()
            .find(|v| v.key == "github.enabled")
            .map(|v| v.value.as_str()),
        Some("false"),
        "in-memory config must stay at default after failed persist"
    );
}

#[tokio::test]
async fn unset_fails_to_persist_leaves_in_memory_config_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    engine
        .configure(&ConfigureAction::Set {
            key: "github.enabled".to_string(),
            value: "true".to_string(),
        })
        .await
        .unwrap();

    assert!(engine.config().github_enabled());

    let unwritable_path = tmp.path().join("unset_subdir");
    std::fs::create_dir(&unwritable_path).unwrap();
    let engine_with_dir_path = {
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut config = GlobalConfig::default();
        config.github = Some(crystalline_core::config::GitHubConfig {
            enabled: Some(true),
            ..Default::default()
        });
        Engine::new(
            Arc::new(Mutex::new(store)),
            config,
            None,
            Some(unwritable_path),
        )
    };

    let err = engine_with_dir_path
        .configure(&ConfigureAction::Unset {
            key: "github.enabled".to_string(),
        })
        .await;
    assert!(
        err.is_err(),
        "persist must fail when config_path is a directory"
    );

    let after = engine_with_dir_path
        .configure(&ConfigureAction::Show)
        .await
        .unwrap();
    let after_views = settings_of(&after);
    assert_eq!(
        after_views
            .iter()
            .find(|v| v.key == "github.enabled")
            .map(|v| v.value.as_str()),
        Some("true"),
        "in-memory config must stay at applied value after failed unset persist"
    );
}

#[tokio::test]
async fn set_service_read_only_persists_and_carries_the_startup_effective_note() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.yaml");
    let engine = engine_at(&config_path, false).await;

    let data = engine
        .configure(&ConfigureAction::Set {
            key: "service.read_only".to_string(),
            value: "true".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(data["key"], "service.read_only");
    assert_eq!(data["value"], "true");
    assert_eq!(data["source"], "config");
    assert_eq!(
        data["note"],
        "this setting applies the next time the daemon starts; a running daemon keeps its current value"
    );

    let on_disk: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    assert!(on_disk.read_only());

    let data = engine
        .configure(&ConfigureAction::Unset {
            key: "service.read_only".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(data["value"], "false");
    assert_eq!(data["source"], "default");

    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !raw.contains("service"),
        "an emptied service block must not round-trip into the saved file: {raw}"
    );
}
