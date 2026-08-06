//! The JSON API `serve --http` nests at `/api/v1`, the surface the Fluid UI
//! talks to. Handlers pass the engine's own JSON values through unchanged, so
//! the MCP tools and this API stay one source of truth, and every failure is
//! an [`ApiError`] rendered as RFC 9457 problem detail.

mod auth;
mod auth_store;
mod discovery;
mod domains;
mod engrams;
mod error;
mod graph;
mod users_api;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, patch, post};
use tokio::sync::Semaphore;

pub use auth::{
    AuthCfg, CSRF_HEADER, Caller, Identity, LOGIN_SLOTS, SESSION_COOKIE, SESSION_TTL_SECS,
};
pub use auth_store::*;
pub use error::{ApiError, ApiJson, ApiPath, ApiQuery};

use crate::engine::Engine;

/// What every REST handler is given: the one shared engine the daemon owns,
/// the one auth store this process holds open, and the auth settings resolved
/// at startup. Cheap to clone; axum clones it per request.
#[derive(Clone)]
pub struct RestState {
    /// The engine backing every request, shared with the MCP router.
    pub engine: Arc<Engine>,
    /// The one users-and-sessions store this process holds open. Shared rather
    /// than opened per request: it serializes its own database access, so a
    /// second store would only add handles on one small file.
    pub auth: Arc<AuthStore>,
    /// The auth settings as of startup. See [`AuthCfg`].
    pub auth_cfg: AuthCfg,
    /// Caps how many password verifications run at once. See
    /// [`LOGIN_SLOTS`].
    login_slots: Arc<Semaphore>,
}

impl RestState {
    /// Assemble the state, resolving and validating the auth settings out of
    /// the engine's config. Fails when `auth.trusted_header` is not a usable
    /// HTTP header name: the HTTP surface then refuses to come up, naming the
    /// setting, rather than serving with a header that silently never matches.
    pub fn new(engine: Arc<Engine>, auth: Arc<AuthStore>) -> anyhow::Result<RestState> {
        let auth_cfg = AuthCfg::resolve(&engine.config())?;
        Ok(RestState {
            engine,
            auth,
            auth_cfg,
            login_slots: auth::login_slots(),
        })
    }

    /// Run `work` holding one of the [`LOGIN_SLOTS`] password-work permits.
    ///
    /// The semaphore is deliberately not exposed itself: password work is the
    /// only thing it may gate, and a handler that hashes has to go through here
    /// rather than reach for the field. See [`auth::with_login_slot`] for what
    /// the cap is for and why the admin routes share the login one instead of
    /// getting a second.
    pub(super) async fn with_login_slot<F: std::future::Future>(
        &self,
        work: F,
    ) -> Result<F::Output, ApiError> {
        auth::with_login_slot(&self.login_slots, work).await
    }
}

/// Build the REST router. Mounted with `nest("/api/v1", ...)`, so the paths
/// here are relative to that prefix and the fallback below only ever answers
/// for unknown paths under it.
///
/// [`auth::guard`] is layered over the whole thing, fallback included, so
/// identity resolution, the CSRF check and the closed-by-default rule apply to
/// every path under the mount - including the ones later tasks add, which are
/// guarded the moment they are registered. Every route therefore belongs
/// *above* the `.layer` call: axum only wraps what was declared before it, so a
/// route added below would serve unguarded.
pub fn router(state: RestState) -> Router {
    Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/domains", get(domains::list))
        .route("/domains/{domain}/tree", get(domains::tree))
        .route("/domains/{domain}/manifest", get(domains::manifest))
        .route("/domains/{domain}/engrams", get(engrams::list))
        // A wildcard, not a segment: a permalink is a path, so an engram in a
        // subfolder carries the slashes with it.
        .route(
            "/domains/{domain}/engrams/{*permalink}",
            get(engrams::detail),
        )
        .route("/search", get(discovery::search))
        .route("/vocabulary", get(discovery::vocabulary))
        .route("/context", get(discovery::context))
        .route("/activity", get(discovery::activity))
        .route("/graph", get(graph::graph))
        // Admin only, enforced inside the handlers: the guard below stops at
        // viewer. See [`users_api`] for the three rules these first mutating
        // routes are held to.
        .route("/users", get(users_api::list).post(users_api::create))
        .route(
            "/users/{name}",
            patch(users_api::update).delete(users_api::remove),
        )
        .fallback(unknown_path)
        // Applies to every method router registered above it, so it stays
        // below the routes and above the guard.
        .method_not_allowed_fallback(wrong_method)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::guard,
        ))
        .with_state(state)
}

/// Answer an unknown `/api/v1` path in problem+json rather than letting it
/// fall through to the MCP transport, which would reply in its own shape.
async fn unknown_path() -> ApiError {
    ApiError::not_found("unknown API path")
}

/// Answer a known path asked for with a method it does not serve, in
/// problem+json rather than axum's empty 405.
async fn wrong_method() -> ApiError {
    ApiError::method_not_allowed()
}

/// Split a comma-separated query parameter into the `Vec<String>` the engine's
/// params take, dropping the whitespace and the empties a hand-written URL
/// brings with it: `?tags=a,%20b,` asks for `a` and `b` rather than for a tag
/// that is one space long, and an absent parameter asks for nothing at all.
///
/// Every list-valued parameter on this surface arrives this way rather than as a
/// repeated key: one spelling for a caller to learn, and the same one the engine
/// then sees whichever endpoint it came through.
fn csv(raw: Option<&str>) -> Vec<String> {
    raw.map(|raw| {
        raw.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A [`RestState`] over an empty in-memory engine and a fresh auth
    /// database: enough to exercise the state's own machinery without a domain
    /// on disk behind it.
    async fn test_state() -> (tempfile::TempDir, RestState) {
        let dir = tempfile::tempdir().unwrap();
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            crystalline_core::config::GlobalConfig::default(),
            None,
            None,
        ));
        let auth = Arc::new(
            AuthStore::open(&dir.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        (dir, RestState::new(engine, auth).unwrap())
    }

    /// The cap the admin routes borrow: whatever calls
    /// [`RestState::with_login_slot`] - a login, an account being created, a
    /// password being reset - only [`LOGIN_SLOTS`] of them hold argon2's
    /// working memory at a time. Asserted on the mechanism, by counting how
    /// many bodies are inside at once, rather than on how long anything took.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_login_limiter_caps_every_caller_that_hashes() {
        let (_dir, state) = test_state().await;
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..32 {
            let (state, live, peak) = (state.clone(), live.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                state
                    .with_login_slot(async {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        live.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await
                    .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(live.load(Ordering::SeqCst), 0, "every permit came back");
        let peak = peak.load(Ordering::SeqCst);
        assert!(
            peak <= LOGIN_SLOTS,
            "at most {LOGIN_SLOTS} may hash at once, saw {peak}"
        );
        assert!(peak > 1, "and the limiter must not serialize them either");
    }

    #[test]
    fn a_comma_list_splits_and_drops_the_empties() {
        assert_eq!(csv(Some("a,b")), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            csv(Some(" a , b ")),
            vec!["a".to_string(), "b".to_string()],
            "a hand-written list is not punished for its spaces"
        );
        assert_eq!(csv(Some("a,,")), vec!["a".to_string()]);
        assert!(
            csv(Some("")).is_empty(),
            "no values rather than one empty one"
        );
        assert!(csv(Some(" , ")).is_empty());
        assert!(csv(None).is_empty(), "an absent parameter asks for nothing");
    }
}
