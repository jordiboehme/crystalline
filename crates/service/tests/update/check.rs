//! The daemon's daily check against the fake GitHub: at most one lookup a
//! day, failures cached, `first_seen` kept, the cache written whole, the
//! method detected once, the reasons the check stays off and the ctl
//! answer `status` reads.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::http::{Method, StatusCode};
use crystalline_core::config::UpdateConfig;
use crystalline_core::time::{self, Stamp};
use crystalline_core::update::{
    CheckOutcome, Detection, InstallMethod, Refusal, UPDATE_CACHE_FILE, UPDATE_CACHE_FORMAT,
    UpdateCache,
};
use crystalline_core::version::Precedence;
use crystalline_service::control::{answer_for_test, answer_for_test_with_update};
use crystalline_service::update::{UpdateState, check_suppressed, read_cache, run_check};
use serde_json::json;

use super::{API_PATH, ENV, Fake, WEB_PATH, redirect_to, respond, use_fake};
use crate::fixture::{fixture, fixture_with};

fn t0() -> Stamp {
    Stamp::from_ymd_hms(2026, 11, 18, 9, 0, 0).unwrap()
}

fn later(hours: i64) -> Stamp {
    t0().checked_add(time::hours(hours)).unwrap()
}

/// A release far ahead of this binary, so every test sees a newer one.
fn api_release(tag: &str) -> String {
    json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "published_at": "2026-11-17T10:00:00Z",
        "assets": []
    })
    .to_string()
}

fn published() -> Stamp {
    Stamp::from_ymd_hms(2026, 11, 17, 10, 0, 0).unwrap()
}

fn api_count(fake: &Fake) -> usize {
    fake.seen()
        .iter()
        .filter(|seen| seen.method == Method::GET && seen.path == API_PATH)
        .count()
}

fn sent_etag(fake: &Fake, index: usize) -> Option<String> {
    fake.seen()
        .iter()
        .filter(|seen| seen.method == Method::GET && seen.path == API_PATH)
        .nth(index)?
        .headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn release_outcome(cache: &UpdateCache) -> (String, Option<Stamp>, Stamp) {
    match &cache.outcome {
        CheckOutcome::Release {
            version,
            published_at,
            first_seen,
        } => (version.clone(), *published_at, *first_seen),
        other => panic!("expected a release, got {other:?}"),
    }
}

#[tokio::test]
async fn checks_at_most_once_a_day() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|_, path, _| match path {
        API_PATH => respond(
            StatusCode::OK,
            &[("etag", "\"e1\"")],
            &api_release("v99.0.0"),
        ),
        _ => respond(StatusCode::NOT_FOUND, &[], ""),
    })
    .await;
    use_fake(&fake);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state").join(UPDATE_CACHE_FILE);

    let first = run_check(&path, t0()).await;
    assert_eq!(api_count(&fake), 1);
    assert_eq!(first.checked_at, t0());
    assert_eq!(first.etag.as_deref(), Some("\"e1\""));
    let (version, published_at, first_seen) = release_outcome(&first);
    assert_eq!(version, "v99.0.0", "the tag as the client built it");
    assert_eq!(published_at, Some(published()));
    assert_eq!(first_seen, t0());
    assert_eq!(
        first
            .newer_release(&Precedence::parse(crystalline_core::VERSION).unwrap())
            .map(|v| v.render()),
        Some("99.0.0".to_string())
    );
    assert_eq!(
        read_cache(&path),
        Some(first.clone()),
        "written to the file"
    );

    // Within the day: the cache answers and GitHub is not asked.
    assert_eq!(run_check(&path, later(1)).await, first);
    assert_eq!(run_check(&path, later(23)).await, first);
    assert_eq!(api_count(&fake), 1);

    // A day later it asks again, with the ETag it kept.
    let next = run_check(&path, later(24)).await;
    assert_eq!(api_count(&fake), 2);
    assert_eq!(sent_etag(&fake, 0), None);
    assert_eq!(sent_etag(&fake, 1).as_deref(), Some("\"e1\""));
    assert_eq!(next.checked_at, later(24));

    // A checked_at in the future (a clock set back) is no reason to wait.
    let future = run_check(&path, later(10)).await;
    assert_eq!(api_count(&fake), 3);
    assert_eq!(future.checked_at, later(10));
}

#[tokio::test]
async fn a_failed_check_is_cached() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|_, _, _| respond(StatusCode::BAD_GATEWAY, &[], "down")).await;
    use_fake(&fake);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(UPDATE_CACHE_FILE);

    let failed = run_check(&path, t0()).await;
    assert_eq!(
        failed.outcome,
        CheckOutcome::Failed {
            reason: "GitHub answered with status 502".to_string()
        }
    );
    assert_eq!(failed.etag, None);
    assert_eq!(read_cache(&path), Some(failed.clone()));
    assert_eq!(run_check(&path, later(12)).await, failed);
    assert_eq!(
        api_count(&fake),
        1,
        "a failure holds the next check off too"
    );

    run_check(&path, later(24)).await;
    assert_eq!(api_count(&fake), 2);
    assert_eq!(sent_etag(&fake, 1), None, "no ETag after a failure");
}

#[tokio::test]
async fn a_connection_failure_caches_no_error_text() {
    let _env = ENV.lock().await;
    // A port nothing listens on: bind one, then let it go.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let fake = Fake::start(|_, _, _| respond(StatusCode::OK, &[], "")).await;
    use_fake(&fake);
    unsafe { std::env::set_var("CRYSTALLINE_TEST_UPDATE_API", &base) };
    let dir = tempfile::tempdir().unwrap();
    let cache = run_check(&dir.path().join(UPDATE_CACHE_FILE), t0()).await;
    assert_eq!(
        cache.outcome,
        CheckOutcome::Failed {
            reason: "cannot reach GitHub".to_string()
        }
    );
}

#[tokio::test]
async fn first_seen_survives_a_recheck() {
    let _env = ENV.lock().await;
    // 0: the API answers v99.0.0; 1: rate limited, the fallback names the
    // same tag; 2: the API answers v99.0.1.
    let phase = Arc::new(AtomicUsize::new(0));
    let answering = phase.clone();
    let fake = Fake::start(move |method, path, _| {
        match (answering.load(Ordering::SeqCst), method, path) {
            (0, &Method::GET, API_PATH) => respond(
                StatusCode::OK,
                &[("etag", "\"e1\"")],
                &api_release("v99.0.0"),
            ),
            (1, &Method::GET, API_PATH) => respond(
                StatusCode::FORBIDDEN,
                &[("x-ratelimit-remaining", "0")],
                "{}",
            ),
            (1, &Method::HEAD, WEB_PATH) => redirect_to("v99.0.0"),
            (2, &Method::GET, API_PATH) => respond(
                StatusCode::OK,
                &[("etag", "\"e2\"")],
                &api_release("v99.0.1"),
            ),
            _ => respond(StatusCode::NOT_FOUND, &[], ""),
        }
    })
    .await;
    use_fake(&fake);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(UPDATE_CACHE_FILE);

    run_check(&path, t0()).await;
    phase.store(1, Ordering::SeqCst);
    let fallback = run_check(&path, later(25)).await;
    assert_eq!(fallback.checked_at, later(25));
    assert_eq!(
        release_outcome(&fallback),
        ("v99.0.0".to_string(), Some(published()), t0()),
        "first_seen and the publish time are kept"
    );
    assert_eq!(
        fallback.etag.as_deref(),
        Some("\"e1\""),
        "the fallback has no ETag, so the stored one stays"
    );

    phase.store(2, Ordering::SeqCst);
    let newer = run_check(&path, later(50)).await;
    assert_eq!(
        release_outcome(&newer),
        ("v99.0.1".to_string(), Some(published()), later(50)),
        "a new version is first seen now"
    );
    assert_eq!(newer.etag.as_deref(), Some("\"e2\""));
}

#[tokio::test]
async fn the_cache_is_written_atomically() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|_, path, _| match path {
        API_PATH => respond(StatusCode::OK, &[], &api_release("v99.0.0")),
        _ => respond(StatusCode::NOT_FOUND, &[], ""),
    })
    .await;
    use_fake(&fake);
    // The rename is forced to fail: a folder sits where the cache goes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(UPDATE_CACHE_FILE);
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), "x").unwrap();

    let cache = run_check(&path, t0()).await;
    assert_eq!(release_outcome(&cache).0, "v99.0.0", "still answered");
    assert!(path.is_dir(), "nothing replaced what stood there");
    assert_eq!(names(dir.path()), vec![UPDATE_CACHE_FILE.to_string()]);

    // A rename that works leaves only the cache, never a temporary file.
    let other = tempfile::tempdir().unwrap();
    let good = other.path().join(UPDATE_CACHE_FILE);
    run_check(&good, t0()).await;
    assert_eq!(names(other.path()), vec![UPDATE_CACHE_FILE.to_string()]);
    assert!(read_cache(&good).is_some());
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_state_folder_without_the_cache_reads_none() {
    let _env = ENV.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(UPDATE_CACHE_FILE);
    assert_eq!(read_cache(&path), None, "no file");
    assert_eq!(
        read_cache(&dir.path().join("missing").join(UPDATE_CACHE_FILE)),
        None
    );
    std::fs::write(&path, "{\"v\":1,\"checked_at\":").unwrap();
    assert_eq!(read_cache(&path), None, "a file that does not parse");
    std::fs::write(
        &path,
        json!({"v": UPDATE_CACHE_FORMAT + 1, "checked_at": "2026-11-18T09:00:00Z",
               "outcome": {"kind": "failed", "reason": "x"}})
        .to_string(),
    )
    .unwrap();
    assert_eq!(read_cache(&path), None, "another format");

    // The next check ignores it and writes a good one.
    let fake = Fake::start(|_, path, _| match path {
        API_PATH => respond(StatusCode::OK, &[], &api_release("v99.0.0")),
        _ => respond(StatusCode::NOT_FOUND, &[], ""),
    })
    .await;
    use_fake(&fake);
    let cache = run_check(&path, t0()).await;
    assert_eq!(api_count(&fake), 1);
    assert_eq!(read_cache(&path), Some(cache));
}

#[test]
fn suppressed_by_no_update_notifier_ci_docker_mcpb() {
    let tarball = Detection {
        method: InstallMethod::Tarball,
        refusal: None,
    };
    let env_of = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };
    assert_eq!(check_suppressed(&env_of(&[]), &tarball), None);
    assert_eq!(
        check_suppressed(&env_of(&[("NO_UPDATE_NOTIFIER", "1")]), &tarball),
        Some("NO_UPDATE_NOTIFIER is set")
    );
    assert_eq!(
        check_suppressed(&env_of(&[("CI", "true")]), &tarball),
        Some("CI is set")
    );
    assert_eq!(
        check_suppressed(&env_of(&[("CI", ""), ("NO_UPDATE_NOTIFIER", "")]), &tarball),
        None,
        "an empty value is not set"
    );
    // The method decides, also when an unofficial build replaced the refusal.
    for refusal in [Some(Refusal::Docker), Some(Refusal::NotOfficial)] {
        let docker = Detection {
            method: InstallMethod::Docker,
            refusal,
        };
        assert_eq!(check_suppressed(&env_of(&[]), &docker), Some("Docker"));
    }
    let mcpb = Detection {
        method: InstallMethod::Mcpb,
        refusal: Some(Refusal::Mcpb),
    };
    assert_eq!(
        check_suppressed(&env_of(&[]), &mcpb),
        Some("Claude Desktop extension")
    );
    let state = UpdateState::detect(
        Path::new("/nowhere/crystalline"),
        &env_of(&[("CRYSTALLINE_CHANNEL", "mcpb")]),
        None,
    );
    assert_eq!(state.facts(true)["check"], "off: Claude Desktop extension");
}

#[test]
fn method_is_detected_once_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join(if cfg!(windows) {
        "crystalline.exe"
    } else {
        "crystalline"
    });
    std::fs::write(&exe, "binary").unwrap();
    let none = |_: &str| None;
    let state = UpdateState::detect(&exe, &none, None);
    let expected = if cfg!(windows) {
        InstallMethod::Zip
    } else {
        InstallMethod::Tarball
    };
    assert_eq!(state.detection().method, expected);

    // A package upgrade removes the running exe: a new detection would say
    // unknown, the daemon's stays.
    std::fs::remove_file(&exe).unwrap();
    assert_eq!(
        crystalline_core::update::detect(&exe, &none, None)
            .detection
            .method,
        InstallMethod::Unknown
    );
    assert_eq!(state.detection().method, expected);
    assert_eq!(state.facts(false)["method"], expected.label());
}

#[tokio::test]
async fn update_facts_over_ctl() {
    let request = json!({"v": 1, "cmd": "update_facts"});

    // A daemon with no cache.
    let fx = fixture(&[]).await;
    let answer = answer_for_test(fx.engine.clone(), &request).await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["data"]["cache"], serde_json::Value::Null, "{answer}");
    assert_eq!(answer["data"]["check"], "off: not turned on", "{answer}");
    assert_eq!(answer["data"]["official"], false, "a test build");

    // A daemon that loaded a cache at start answers it without its ETag.
    let dir = tempfile::tempdir().unwrap();
    let cache_path = dir.path().join(UPDATE_CACHE_FILE);
    let cache = UpdateCache {
        v: UPDATE_CACHE_FORMAT,
        checked_at: t0(),
        outcome: CheckOutcome::Release {
            version: "v99.0.0".to_string(),
            published_at: Some(published()),
            first_seen: t0(),
        },
        etag: Some("\"secret-etag\"".to_string()),
    };
    std::fs::write(&cache_path, serde_json::to_string(&cache).unwrap()).unwrap();
    let exe = dir.path().join("crystalline");
    std::fs::write(&exe, "binary").unwrap();
    let none = |_: &str| None;
    let state = Arc::new(UpdateState::detect(&exe, &none, Some(cache_path.clone())));
    let answer = answer_for_test_with_update(fx.engine.clone(), state.clone(), &request).await;
    assert_eq!(answer["ok"], true, "{answer}");
    let data = &answer["data"];
    assert_eq!(data["method"], state.detection().method.label());
    assert_eq!(data["check"], "off: not turned on");
    assert_eq!(data["cache"]["v"], 1);
    assert_eq!(data["cache"]["checked_at"], "2026-11-18T09:00:00Z");
    assert_eq!(data["cache"]["outcome"]["kind"], "release");
    assert_eq!(data["cache"]["outcome"]["version"], "v99.0.0");
    assert_eq!(data["cache"]["etag"], serde_json::Value::Null, "{answer}");
    assert!(!answer.to_string().contains("secret-etag"), "{answer}");
    let parsed: UpdateCache = serde_json::from_value(data["cache"].clone()).unwrap();
    assert_eq!(
        parsed,
        UpdateCache {
            etag: None,
            ..cache
        }
    );

    // The setting is read live; the environment's reasons win over it.
    let on = fixture_with(&[], |cfg, _| {
        cfg.update = Some(UpdateConfig {
            check: Some(true),
            min_age_hours: None,
        });
    })
    .await;
    let answer = answer_for_test_with_update(on.engine.clone(), state, &request).await;
    assert_eq!(answer["data"]["check"], "on", "{answer}");
    let quiet = |name: &str| (name == "NO_UPDATE_NOTIFIER").then(|| "1".to_string());
    let suppressed = Arc::new(UpdateState::detect(&exe, &quiet, None));
    let answer = answer_for_test_with_update(on.engine.clone(), suppressed, &request).await;
    assert_eq!(
        answer["data"]["check"], "off: NO_UPDATE_NOTIFIER is set",
        "{answer}"
    );
}
