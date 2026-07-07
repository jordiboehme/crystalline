//! The status report's activity block: idle by default, and a finished
//! maintenance operation shows up as the last one.

use std::sync::Arc;

use crystalline_core::config::GlobalConfig;
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use tokio::sync::Mutex;

#[tokio::test]
async fn status_report_carries_the_activity_block() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        GlobalConfig::default(),
        None,
        Some(tmp.path().join("config.yaml")),
    );

    let status = eng.status_report().await.unwrap();
    assert_eq!(status["activity"]["now"], serde_json::json!([]));
    assert!(status["activity"]["last"].is_null());
    assert!(status["activity"]["embedding_backlog"].as_u64().is_some());

    // A completed sync becomes the last finished operation.
    eng.sync(None).await.unwrap();
    let status = eng.status_report().await.unwrap();
    assert_eq!(status["activity"]["now"], serde_json::json!([]));
    assert_eq!(status["activity"]["last"]["kind"], "sync");
}
