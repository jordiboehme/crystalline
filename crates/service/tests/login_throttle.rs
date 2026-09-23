//! What a run of failed sign-ins costs at `POST /api/v1/auth/login`.
//!
//! Its own suite rather than rows in one of the existing auth files, because
//! every test here has to set `auth.login.*` explicitly: every other fixture
//! on this surface turns the throttle off, so that a test which mistypes a
//! password four times does not spend seven seconds of wall clock proving
//! something it is not about.
//!
//! The router is served with `into_make_service_with_connect_info`, the way
//! `run_http` serves it, so the peer address the failed-login log event
//! reports exists on the request.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crystalline_core::config::{
    AuthConfig, GlobalConfig, LoginConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::rest::{AuthStore, RestState, Role, router};
use tokio::sync::Mutex;

/// A served instance with one account and a throttle configured for the test.
struct LoginCtx {
    addr: SocketAddr,
    auth: Arc<AuthStore>,
    _tmp: tempfile::TempDir,
}

/// A response plus how long the request took, for the tests that are about
/// the delay rather than about the status.
struct Timed {
    response: reqwest::Response,
    elapsed: Duration,
}

impl LoginCtx {
    /// Serve an instance whose throttle lets `free` consecutive failures
    /// through and grows the delay to at most `max_delay_secs`.
    async fn start_with_throttle(free: u32, max_delay_secs: u64) -> LoginCtx {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GlobalConfig {
            auth: Some(AuthConfig {
                login: Some(LoginConfig {
                    free_attempts: Some(free),
                    max_delay: Some(max_delay_secs),
                }),
                ..AuthConfig::default()
            }),
            service: Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                ..ServiceConfig::default()
            }),
            ..GlobalConfig::default()
        };
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(Arc::new(Mutex::new(store)), cfg, None, None));
        let auth = Arc::new(
            AuthStore::open(&tmp.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        let state = RestState::new(engine, auth.clone(), &[]).unwrap();
        let app = axum::Router::new().nest("/api/v1", router(state));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        LoginCtx {
            addr,
            auth,
            _tmp: tmp,
        }
    }

    /// Add an account that can sign in.
    async fn create_account(&self, name: &str, password: &str) {
        self.auth
            .add_user(name, name, None, Role::Viewer, password)
            .await
            .unwrap();
    }

    /// One sign-in attempt.
    async fn login(&self, name: &str, password: &str) -> reqwest::Response {
        // Proxy discovery off: the target is loopback, where a system proxy
        // must never be consulted, and the lookup can block for a minute on a
        // managed network configuration.
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://{}/api/v1/auth/login", self.addr))
            .json(&serde_json::json!({"name": name, "password": password}))
            .send()
            .await
            .unwrap()
    }

    /// The same, timed.
    async fn timed_login(&self, name: &str, password: &str) -> Timed {
        let started = Instant::now();
        let response = self.login(name, password).await;
        Timed {
            response,
            elapsed: started.elapsed(),
        }
    }
}

/// **A wrong name and a wrong password escalate identically.**
///
/// The companion to `one_argon2_verification_per_login_attempt`. That test
/// pins the cost of a single attempt; this one pins the cost of a run of them,
/// which is the channel a throttle keyed on account existence would reopen.
#[tokio::test]
async fn an_unknown_name_escalates_exactly_as_a_wrong_password_does() {
    let ctx = LoginCtx::start_with_throttle(1, 2).await;
    ctx.create_account("ada", "correct horse").await;

    // Past the one free attempt both are delayed, and the delay is the same.
    for name in ["ada", "nobody-at-all"] {
        let first = ctx.login(name, "wrong").await;
        assert_eq!(first.status(), 401, "the free attempt is not delayed");
        let second = ctx.timed_login(name, "wrong").await;
        assert_eq!(second.response.status(), 401);
        assert!(
            second.elapsed >= Duration::from_secs(1),
            "{name} was not delayed: {:?}",
            second.elapsed
        );
    }
}

/// **Past the ceiling the answer is a 429 that says when to come back**, and
/// it is sent without reaching argon2 or the store at all.
#[tokio::test]
async fn past_the_ceiling_the_answer_is_a_refusal() {
    // One free attempt and a one second ceiling: failure 1 is free, failure 2
    // costs the base second, and a third would want two seconds, which is past
    // the ceiling, so it refuses.
    let ctx = LoginCtx::start_with_throttle(1, 1).await;
    ctx.create_account("ada", "correct horse").await;
    assert_eq!(ctx.login("ada", "wrong").await.status(), 401);
    assert_eq!(ctx.login("ada", "wrong").await.status(), 401);
    let refused = ctx.timed_login("ada", "wrong").await;
    assert_eq!(refused.response.status(), 429);
    assert!(
        refused.elapsed < Duration::from_millis(500),
        "a refusal is immediate rather than a longer nap: {:?}",
        refused.elapsed
    );
    let retry: u64 = refused
        .response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .expect("a 429 says when to come back")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (1..=900).contains(&retry),
        "the wait is the rest of the window: {retry}"
    );
}

/// **The 401 body is the same at every step**, so nothing about the schedule
/// tells a caller anything the plain refusal did not.
#[tokio::test]
async fn the_refusal_body_never_changes() {
    let ctx = LoginCtx::start_with_throttle(1, 2).await;
    ctx.create_account("ada", "correct horse").await;
    let first = ctx.login("ada", "wrong").await.text().await.unwrap();
    let second = ctx.login("ada", "wrong").await.text().await.unwrap();
    assert_eq!(first, second);
    assert!(first.contains("the name or password is wrong"), "{first}");
}

/// **A correct password works immediately after failures**, with no residual
/// delay, which is what somebody who mistyped twice then got it right does.
#[tokio::test]
async fn a_success_after_failures_is_not_delayed_and_clears_the_slate() {
    let ctx = LoginCtx::start_with_throttle(0, 8).await;
    ctx.create_account("ada", "correct horse").await;
    ctx.login("ada", "wrong").await;
    let ok = ctx.timed_login("ada", "correct horse").await;
    assert_eq!(ok.response.status(), 200);
    // The next wrong attempt is a first failure again, so it is not delayed.
    let after = ctx.timed_login("ada", "wrong").await;
    assert_eq!(after.response.status(), 401);
    assert!(
        after.elapsed < Duration::from_millis(500),
        "the slate was not cleared: {:?}",
        after.elapsed
    );
}

/// **A zero ceiling is off**, which is what every other fixture on this
/// surface relies on: a run of failures costs nothing at all.
#[tokio::test]
async fn a_zero_ceiling_serves_a_run_of_failures_at_full_speed() {
    let ctx = LoginCtx::start_with_throttle(0, 0).await;
    ctx.create_account("ada", "correct horse").await;
    let started = Instant::now();
    for _ in 0..6 {
        assert_eq!(ctx.login("ada", "wrong").await.status(), 401);
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a disabled throttle must not delay anything: {:?}",
        started.elapsed()
    );
}
