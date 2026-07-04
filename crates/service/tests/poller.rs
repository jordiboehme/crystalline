//! Service-level tests for the background origin poller
//! (`Engine::origin_poll_tick`, spawned by `crate::daemon::run_origin_poller`
//! on a heartbeat): it reuses `origin_update` for the actual pull, so these
//! tests exercise the scheduling and gating decisions the daemon wiring
//! itself is too thin to need its own tests for.
//!
//! Every test injects `support::MockProvider` via `Engine::with_origin_provider`
//! and points origin state and the GitHub token store at tempdirs via
//! `Engine::with_origins_dir` and `Engine::with_token_store_dir`, so nothing
//! here reaches a network, a real GitHub repository or the real machine's
//! config, state directory or OS keychain.
//!
//! `poll_secs`' 60-second floor makes waiting out real intervals
//! impractical, so every test drives `Engine::origin_poll_tick` directly
//! with explicit `Instant`/`DateTime<Utc>` values instead of sleeping: a
//! domain never scheduled before is always due on its first tick, and later
//! ticks pass whatever synthetic time is needed to prove due/not-due and
//! backoff behavior, with no real waiting at all.

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use crystalline_core::config::{GitHubConfig, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_remote::{StoredToken, TokenStore};
use crystalline_service::Engine;
use crystalline_service::engine::ConfigureAction;
use support::MockProvider;
use tokio::sync::Mutex;

fn config(poll_secs: Option<u64>) -> GlobalConfig {
    GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            poll_secs,
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    }
}

/// An engine wired the way the poller tests need: a mock provider for every
/// origin call, origin state and the GitHub token store both under tempdirs.
/// `token_dir` starts empty (no saved token); call [`write_fake_token`] to
/// simulate a landed `connect`.
async fn engine_with(
    config_path: &Path,
    origins_dir: &Path,
    token_dir: &Path,
    provider: Arc<MockProvider>,
    poll_secs: Option<u64>,
) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(Mutex::new(store)),
        config(poll_secs),
        None,
        Some(config_path.to_path_buf()),
    )
    .with_origin_provider(provider)
    .with_origins_dir(origins_dir.to_path_buf())
    .with_token_store_dir(token_dir.to_path_buf())
}

/// Writes a fake GitHub token straight into `dir` as `TokenStore::File`
/// would, simulating a `connect` having landed, with no real device flow, no
/// network and no OS keychain.
fn write_fake_token(dir: &Path) {
    let store = TokenStore::File {
        path: dir.join("github-token.json"),
    };
    store
        .save(&StoredToken {
            access_token: "test-token".to_string(),
            host: "github.com".to_string(),
            user: "mock-user".to_string(),
            created_at: Utc::now(),
        })
        .unwrap();
}

fn manifest() -> Vec<u8> {
    b"---\ntype: manifest\ntitle: Team\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Team\n\n## Scope\n\n- shared knowledge\n\n## When to Use\n\n- always\n".to_vec()
}

fn engram(title: &str, permalink: &str, body: &str) -> Vec<u8> {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - test\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n{body}\n"
    )
    .into_bytes()
}

fn commit_files(pairs: &[(&str, Vec<u8>)]) -> BTreeMap<String, Vec<u8>> {
    pairs
        .iter()
        .map(|(p, c)| (p.to_string(), c.clone()))
        .collect()
}

// --- happy path --------------------------------------------------------------

#[tokio::test]
async fn poller_applies_an_upstream_edit_without_a_user_call() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        ("notes/alpha.md", engram("Alpha", "alpha", "version one")),
    ]));
    mock.set_branch("main", &c1);

    let token_dir = tmp.path().join("token");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        &token_dir,
        mock.clone(),
        Some(60),
    )
    .await;
    write_fake_token(&token_dir);

    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let c2 = mock.add_commit(commit_files(&[
        ("MANIFEST.md", manifest()),
        (
            "notes/alpha.md",
            engram("Alpha", "alpha", "version two, revised upstream"),
        ),
    ]));
    mock.set_branch("main", &c2);

    // No `origin_update` call: only the poller's own tick function runs. The
    // domain has never been scheduled before, so it is due on this first
    // tick regardless of the configured 60-second floor.
    eng.origin_poll_tick(Instant::now(), Utc::now()).await;

    let content = std::fs::read_to_string(root.join("notes/alpha.md")).unwrap();
    assert!(content.contains("version two"), "{content}");

    let status = eng.status_report().await.unwrap();
    let domains = status["origins"]["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
    assert_eq!(domains[0]["last_result"]["outcome"], "applied");
    assert_eq!(domains[0]["last_result"]["applied"], 1);
    assert!(!domains[0]["next_due"].is_null());
}

#[tokio::test]
async fn poller_does_not_repoll_a_domain_before_its_next_due_instant() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let token_dir = tmp.path().join("token");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        &token_dir,
        mock.clone(),
        Some(300),
    )
    .await;
    write_fake_token(&token_dir);

    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    let calls_after_add = mock.branch_head_calls();

    let now = Instant::now();
    eng.origin_poll_tick(now, Utc::now()).await;
    let calls_after_first_tick = mock.branch_head_calls();
    assert!(
        calls_after_first_tick > calls_after_add,
        "the first tick should have probed the origin"
    );

    // A second tick one second later is nowhere near the 300-second (floored,
    // jittered) interval just scheduled, so it must not touch the provider
    // again.
    eng.origin_poll_tick(now + std::time::Duration::from_secs(1), Utc::now())
        .await;
    assert_eq!(
        mock.branch_head_calls(),
        calls_after_first_tick,
        "a not-yet-due domain must not be polled again"
    );
}

// --- disabled / enabling mid-run ----------------------------------------------

#[tokio::test]
async fn disabled_poller_makes_no_calls_and_enabling_mid_run_starts_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &c1);

    let token_dir = tmp.path().join("token");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        &token_dir,
        mock.clone(),
        Some(60),
    )
    .await;
    write_fake_token(&token_dir);

    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    // Disable collaboration after the domain is connected, mirroring
    // `configure set github.enabled false`.
    eng.configure(&ConfigureAction::Set {
        key: "github.enabled".to_string(),
        value: "false".to_string(),
    })
    .await
    .unwrap();

    let calls_before = mock.branch_head_calls();
    let now = Instant::now();
    for i in 0..5u64 {
        eng.origin_poll_tick(now + std::time::Duration::from_secs(i * 10), Utc::now())
            .await;
    }
    assert_eq!(
        mock.branch_head_calls(),
        calls_before,
        "a disabled poller must never call the provider"
    );

    // Re-enable, mirroring `configure set github.enabled true`: the very next
    // tick starts polling again, no daemon restart involved.
    eng.configure(&ConfigureAction::Set {
        key: "github.enabled".to_string(),
        value: "true".to_string(),
    })
    .await
    .unwrap();
    eng.origin_poll_tick(now + std::time::Duration::from_secs(60), Utc::now())
        .await;
    assert!(
        mock.branch_head_calls() > calls_before,
        "enabling mid-run should have let the next tick poll"
    );
}

// --- unauthenticated / a token landing -----------------------------------------

#[tokio::test]
async fn unauthenticated_poller_makes_no_calls_and_a_landed_token_starts_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let c1 = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &c1);

    // The domain is connected up front (origin_add always uses the injected
    // mock provider, regardless of the token store), then the token store is
    // emptied to simulate a machine that has since lost its connection, or
    // never had one when the daemon started.
    let token_dir = tmp.path().join("token");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        &token_dir,
        mock.clone(),
        Some(60),
    )
    .await;
    write_fake_token(&token_dir);
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();
    std::fs::remove_file(token_dir.join("github-token.json")).unwrap();

    let calls_before = mock.branch_head_calls();
    let now = Instant::now();
    for i in 0..5u64 {
        eng.origin_poll_tick(now + std::time::Duration::from_secs(i * 10), Utc::now())
            .await;
    }
    assert_eq!(
        mock.branch_head_calls(),
        calls_before,
        "an unauthenticated poller must never call the provider"
    );
    let status = eng.status_report().await.unwrap();
    assert_eq!(status["origins"]["connected"], false);

    // A token lands: the very next tick resumes polling automatically.
    write_fake_token(&token_dir);
    eng.origin_poll_tick(now + std::time::Duration::from_secs(60), Utc::now())
        .await;
    assert!(
        mock.branch_head_calls() > calls_before,
        "a landed token should have let the next tick poll"
    );
    let status = eng.status_report().await.unwrap();
    assert_eq!(status["origins"]["connected"], true);
}

// --- rate limiting -------------------------------------------------------------

#[tokio::test]
async fn rate_limited_poller_pauses_every_domain_until_the_reported_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let token_dir = tmp.path().join("token");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        &token_dir,
        mock.clone(),
        Some(60),
    )
    .await;
    write_fake_token(&token_dir);
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    let now = Instant::now();
    let wall_now = Utc::now();
    let reset = wall_now + chrono::Duration::minutes(10);
    mock.fail_branch_head_rate_limited("main", Some(reset));

    // The domain is due on this first real poll attempt; it trips the rate
    // limit, which must pause every domain, not just this one.
    eng.origin_poll_tick(now, wall_now).await;
    let calls_after_limit = mock.branch_head_calls();
    assert!(calls_after_limit > 0);

    let status = eng.status_report().await.unwrap();
    assert_eq!(
        status["origins"]["rate_limit_wait_until"],
        serde_json::to_value(reset).unwrap()
    );

    // Still within the pause window: no further provider calls at all, even
    // though the mock would still refuse if asked.
    eng.origin_poll_tick(now + std::time::Duration::from_secs(30), wall_now)
        .await;
    assert_eq!(mock.branch_head_calls(), calls_after_limit);

    // Past the reset: the pause lifts and polling resumes. The mock's own
    // injected failure is cleared first, mirroring GitHub's rate limit
    // window actually resetting.
    mock.clear_branch_head_rate_limited("main");
    let past_reset = wall_now + chrono::Duration::minutes(11);
    eng.origin_poll_tick(now + std::time::Duration::from_secs(700), past_reset)
        .await;
    assert!(
        mock.branch_head_calls() > calls_after_limit,
        "polling should resume once the rate limit window has passed"
    );
    let status = eng.status_report().await.unwrap();
    assert!(status["origins"]["rate_limit_wait_until"].is_null());
}

// --- status origins block ------------------------------------------------------

#[tokio::test]
async fn status_report_omits_origins_when_github_is_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let store = TursoStore::open_in_memory().await.unwrap();
    let eng = Engine::new(
        Arc::new(Mutex::new(store)),
        GlobalConfig::default(),
        None,
        Some(tmp.path().join("config.yaml")),
    )
    .with_origin_provider(mock)
    .with_origins_dir(tmp.path().join("origins"))
    .with_token_store_dir(tmp.path().join("token"));

    let status = eng.status_report().await.unwrap();
    assert!(
        status.as_object().unwrap().get("origins").is_none(),
        "the origins block must be entirely absent when github.enabled is false: {status}"
    );
}

#[tokio::test]
async fn status_report_origins_block_shape_when_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockProvider::new());
    let commit = mock.add_commit(commit_files(&[("MANIFEST.md", manifest())]));
    mock.set_branch("main", &commit);

    let token_dir = tmp.path().join("token");
    let eng = engine_with(
        &tmp.path().join("config.yaml"),
        &tmp.path().join("origins"),
        &token_dir,
        mock.clone(),
        Some(60),
    )
    .await;
    write_fake_token(&token_dir);
    let root = tmp.path().join("brand-knowledge");
    eng.origin_add(
        "acme/brand-knowledge",
        Some("brand"),
        None,
        None,
        Some(root.to_str().unwrap()),
    )
    .await
    .unwrap();

    // Before any tick: the domain is present but has never been scheduled or
    // polled, so both are null rather than absent.
    let status = eng.status_report().await.unwrap();
    let origins = &status["origins"];
    assert_eq!(origins["enabled"], true);
    assert_eq!(origins["connected"], true);
    assert_eq!(origins["token_store"], "file");
    assert!(origins["rate_limit_wait_until"].is_null());
    let domains = origins["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0]["domain"], "brand");
    assert_eq!(domains[0]["repo"], "acme/brand-knowledge");
    assert_eq!(domains[0]["branch"], "main");
    assert_eq!(domains[0]["open_proposals"], 0);
    assert_eq!(domains[0]["declined_proposals"], 0);
    assert_eq!(domains[0]["conflicts"], 0);
    assert!(domains[0]["next_due"].is_null());
    assert!(domains[0]["last_result"].is_null());

    // After a tick, the schedule and last result are filled in.
    eng.origin_poll_tick(Instant::now(), Utc::now()).await;
    let status = eng.status_report().await.unwrap();
    let domains = status["origins"]["domains"].as_array().unwrap();
    assert!(!domains[0]["next_due"].is_null());
    assert_eq!(domains[0]["last_result"]["outcome"], "up_to_date");
}
