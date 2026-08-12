//! The JSON API `serve --http` nests at `/api/v1`, the surface the Fluid UI
//! talks to. Handlers pass the engine's own JSON values through unchanged, so
//! the MCP tools and this API stay one source of truth, and every failure is
//! an [`ApiError`] rendered as RFC 9457 problem detail.

mod archive;
mod auth;
mod auth_store;
mod discovery;
mod domains;
mod domains_admin;
mod engrams;
mod error;
mod github_settings;
mod graph;
mod users_api;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
use tokio::sync::Semaphore;

pub use auth::{
    AuthCfg, CSRF_HEADER, Caller, Identity, LOGIN_SLOTS, SESSION_COOKIE, SESSION_TTL_SECS,
};
pub use auth_store::*;
pub use error::{
    ApiError, ApiJson, ApiPath, ApiQuery, ConflictDetail, ProblemDetail, if_match,
    precondition_failed,
};

use crate::engine::Engine;

/// The OpenAPI 3.1 document for this surface, assembled from the
/// `#[utoipa::path]` annotation on every handler.
///
/// `info.version` is the *API* version, pinned to `v1` to match the `/api/v1`
/// mount, rather than the crate version utoipa would otherwise take from
/// `Cargo.toml`. That is what makes the committed snapshot survive a release:
/// bumping the workspace version must not rewrite an artifact the UI's client
/// generator is compiled against, and the crate version is already reported by
/// `GET /auth/me` for the client that wants it.
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Crystalline Fluid API",
        version = "v1",
        description = "The JSON API `crystalline serve --http` mounts at \
                       `/api/v1`, the surface the Fluid UI talks to. Reading is \
                       open to any signed-in viewer; writing content needs an \
                       editor account and the `If-Match` token of the version \
                       being replaced, and account management needs an \
                       admin.\n\nEvery path but `/auth/login`, `/auth/logout` \
                       and `/auth/me` is closed by default: a request that \
                       carries no identity is answered 401 ahead of routing, so \
                       an unauthenticated caller never learns which paths \
                       exist. Every failure is an RFC 9457 problem detail sent \
                       as `application/problem+json`.\n\nThe payloads marked as \
                       generic objects are the engine's own JSON, passed \
                       through unchanged so this API and the MCP tools stay one \
                       source of truth; each carries an example of the shape it \
                       answers with.",
        license(name = "AGPL-3.0-or-later"),
    ),
    tags(
        (name = "meta", description = "The API's description of itself."),
        (name = "auth", description = "Sessions and the capability probe."),
        (name = "domains", description = "Which domains this instance serves and what each holds."),
        (name = "engrams", description = "Listing, reading and writing engrams."),
        (name = "discovery", description = "Search, vocabulary, context and recent activity."),
        (name = "graph", description = "The neighborhood graph around an anchor."),
        (name = "users", description = "Account management. Admin only."),
        (name = "settings", description = "Instance settings. Admin only."),
    ),
    paths(
        openapi_json,
        auth::login,
        auth::logout,
        auth::me,
        domains::list,
        domains_admin::create,
        domains_admin::remove,
        domains_admin::sync_status,
        domains_admin::sync_now,
        archive::download,
        archive::preview,
        archive::import,
        domains::tree,
        domains::manifest,
        domains::save_manifest,
        engrams::list,
        engrams::detail,
        engrams::inbound,
        engrams::create,
        engrams::save,
        engrams::retire,
        engrams::move_action,
        engrams::remove,
        engrams::validate,
        crate::collab::ws::join,
        discovery::search,
        discovery::vocabulary,
        discovery::context,
        discovery::activity,
        graph::graph,
        users_api::list,
        users_api::create,
        users_api::update,
        users_api::reset_password,
        users_api::remove,
        github_settings::status,
        github_settings::connect,
        github_settings::token,
        github_settings::disconnect,
    ),
    components(schemas(
        ProblemDetail,
        ConflictDetail,
        User,
        Role,
        domains::SaveManifestBody,
        domains_admin::CreateDomainBody,
        archive::ArchiveEntryReport,
        archive::ArchiveReport,
        engrams::CreateEngramBody,
        engrams::SaveEngramBody,
        engrams::RetireBody,
        engrams::MoveBody,
        engrams::ValidateBody,
        engrams::ValidateFinding,
        engrams::ValidateResponse,
        auth::LoginBody,
        auth::LoginResponse,
        auth::LogoutResponse,
        auth::MeResponse,
        users_api::CreateBody,
        users_api::PatchBody,
        users_api::PasswordBody,
        users_api::UserResponse,
        users_api::UsersResponse,
        github_settings::TokenBody,
        github_settings::GithubStatusResponse,
        github_settings::GithubPendingView,
    )),
)]
struct ApiDoc;

/// This surface's OpenAPI document.
///
/// One definition with two consumers: the [`openapi_json`] route serves it, and
/// `tests/openapi_snapshot.rs` compares it against the committed
/// `openapi/fluid-v1.json` the UI generates its client types from. Neither can
/// drift from the annotations without the other noticing.
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    <ApiDoc as utoipa::OpenApi>::openapi()
}

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
    /// The open co-editing sessions, one registry for this process: the
    /// collab upgrade route joins rooms in it, and every save it makes goes
    /// back through the engine above.
    pub collab: Arc<crate::collab::session::CollabSessions>,
    /// Caps how many password verifications run at once. See
    /// [`LOGIN_SLOTS`].
    login_slots: Arc<Semaphore>,
    /// Serializes domain create against domain unregister, process-wide. See
    /// [`RestState::domain_admin`].
    domain_admin: Arc<tokio::sync::Mutex<()>>,
    /// The fence an unregistration raises against new co-editing joins. See
    /// [`RestState::fence_joins`].
    join_fence: Arc<tokio::sync::RwLock<()>>,
}

impl RestState {
    /// Assemble the state, resolving and validating the auth settings out of
    /// the engine's config. Fails when `auth.trusted_header` is not a usable
    /// HTTP header name: the HTTP surface then refuses to come up, naming the
    /// setting, rather than serving with a header that silently never matches.
    pub fn new(engine: Arc<Engine>, auth: Arc<AuthStore>) -> anyhow::Result<RestState> {
        let auth_cfg = AuthCfg::resolve(&engine.config())?;
        Ok(RestState {
            collab: crate::collab::session::CollabSessions::new(engine.clone()),
            engine,
            auth,
            auth_cfg,
            login_slots: auth::login_slots(),
            domain_admin: Arc::new(tokio::sync::Mutex::new(())),
            join_fence: Arc::new(tokio::sync::RwLock::new(())),
        })
    }

    /// Hold the domain-admin lock for the whole of a create or an unregister.
    ///
    /// The engine has no serialization of its own across a same-name
    /// `domain_add_*` and `domain_remove` (its `domain_remove` doc comment
    /// records the race and points here): the remove persists the config,
    /// releases both config locks and only then forgets the watcher entry and
    /// clears the index rows, so an add of the same name landing inside that
    /// window has its fresh registration and its freshly-indexed rows wiped
    /// by the remove's tail. One lock over both handlers closes it for this
    /// surface, which is the layer that has more than one caller.
    ///
    /// Deliberately NOT the join fence below. A team-domain create downloads
    /// and indexes a repository inside the request, which can run for
    /// minutes, and fencing co-editing joins for that long would hang every
    /// editor on the instance over a registration that closes no rooms. A
    /// create never sweeps anything, so it has no join window to close.
    ///
    /// Serializing "for this surface" is the whole claim: the mutex lives in
    /// `RestState`, so it does NOT serialize the other callers of the same
    /// engine verbs - MCP's `add_domain` (and the CLI) reach
    /// `domain_add_local`/`domain_add_virtual` with no REST state in hand and
    /// can still race a REST unregister into the engine window above. That is
    /// accepted: closing it needs the per-name lock inside the engine, and
    /// the REST surface is the one with more than one concurrent caller.
    ///
    /// Lock order where both are taken (unregister): this one, then
    /// [`RestState::fence_joins`]. Nothing else takes both.
    pub(super) async fn domain_admin(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.domain_admin.lock().await
    }

    /// Raise the join fence: while this guard lives, no collab upgrade may
    /// open a room, because [`RestState::join_pass`] is what the upgrade
    /// route waits on before it joins.
    ///
    /// Held by an unregistration across its sweep and the engine's
    /// `domain_remove`, which is what makes the sweep final rather than a
    /// snapshot: a join already in flight finishes and inserts its room
    /// before the guard is granted (so the sweep collects it), and a join
    /// that arrives afterwards waits, then finds a domain that no longer
    /// exists and is refused 404. The fence is process-wide rather than
    /// per-domain because an unregistration is short and a second primitive
    /// per domain name would buy nothing measurable.
    pub(super) async fn fence_joins(&self) -> tokio::sync::RwLockWriteGuard<'_, ()> {
        self.join_fence.write().await
    }

    /// The pass a collab upgrade holds across its join, so a join and an
    /// unregistration of the same domain cannot interleave. See
    /// [`RestState::fence_joins`] for the argument this half completes; the
    /// guard is dropped as soon as the join returns, never held across the
    /// socket's life.
    pub(crate) async fn join_pass(&self) -> tokio::sync::RwLockReadGuard<'_, ()> {
        self.join_fence.read().await
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

/// The largest request body this API accepts, in bytes.
///
/// Set explicitly rather than left to axum's 2 MiB default, and set generously,
/// because the body that matters here is one engram's markdown: a domain in the
/// wild holds documents far past a megabyte (the semantic-search spill this
/// project chased was provoked by exactly those), and a default that let such
/// an engram be read but not saved back would fail at the worst moment - after
/// its author had edited it. Ten mebibytes is comfortably past the largest
/// document anyone writes by hand and still small enough that a hostile body
/// cannot make this process reserve serious memory.
///
/// A body over the limit is refused with 413 before a handler runs, in
/// problem+json like every other failure here: `ApiJson` re-renders axum's
/// rejection and keeps the status it chose.
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

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
        .route("/openapi.json", get(openapi_json))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/domains", get(domains::list).post(domains_admin::create))
        // Admin only, enforced in the handler like every other admin route
        // here. Registered before the domain sub-paths for readability only;
        // axum's router is order-independent.
        .route("/domains/{domain}", delete(domains_admin::remove))
        // Admin only as well. The GET is a pure read and stays served on a
        // read-only instance; the POST is a pull that writes, and does not.
        .route(
            "/domains/{domain}/sync",
            get(domains_admin::sync_status).post(domains_admin::sync_now),
        )
        // Admin only as well, and a pure read: the archive download is the
        // backup story of a read-only mirror, so it stays served there.
        .route("/domains/{domain}/archive", get(archive::download))
        // The upload half, and the exception to the line above: both are
        // writes (a preview is the first half of one), so both are admin-only
        // AND refused on a read-only instance.
        .route("/domains/{domain}/archive/preview", post(archive::preview))
        .route("/domains/{domain}/archive/import", post(archive::import))
        .route("/domains/{domain}/tree", get(domains::tree))
        .route(
            "/domains/{domain}/manifest",
            get(domains::manifest).put(domains::save_manifest),
        )
        .route(
            "/domains/{domain}/engrams",
            get(engrams::list).post(engrams::create),
        )
        // A wildcard, not a segment: a permalink is a path, so an engram in a
        // subfolder carries the slashes with it.
        .route(
            "/domains/{domain}/engrams/{*permalink}",
            get(engrams::detail)
                .put(engrams::save)
                .delete(engrams::remove),
        )
        // What points at one engram: a read, so it sits beside the detail
        // route rather than among the actions below. The permalink rides last
        // because a wildcard is terminal - the same shape, for the same
        // reason, as the collab upgrade further down.
        .route(
            "/domains/{domain}/inbound/{*permalink}",
            get(engrams::inbound),
        )
        // Actions rather than sub-paths of `/engrams/{*permalink}`, whose
        // wildcard cannot be followed by another segment: the permalink of
        // the engram being retired or moved rides in the body instead.
        // The collab upgrade: a GET, so the guard's CSRF exemption applies by
        // method; role, read_only and Origin are enforced in the handler,
        // before upgrade. The wildcard is terminal, so nested permalinks ride
        // the path exactly as the engram detail route takes them.
        .route(
            "/collab/{domain}/{*permalink}",
            get(crate::collab::ws::join),
        )
        .route("/domains/{domain}/retire", post(engrams::retire))
        .route("/domains/{domain}/move", post(engrams::move_action))
        // Not domain-scoped like the routes above it: the document being
        // validated may not exist yet, so there is no path segment to name a
        // domain with. It lives here rather than beside `/search` and its
        // siblings because what it checks is engram markdown.
        .route("/validate", post(engrams::validate))
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
        .route("/users/{name}/password", post(users_api::reset_password))
        // Admin only as well, and enforced the same way. The GET is a pure
        // read and is served on a read-only instance; the three mutations
        // are refused there, like every other write on this surface.
        .route(
            "/settings/github",
            get(github_settings::status).delete(github_settings::disconnect),
        )
        .route("/settings/github/connect", post(github_settings::connect))
        .route("/settings/github/token", post(github_settings::token))
        .fallback(unknown_path)
        // Applies to every method router registered above it, so it stays
        // below the routes and above the guard.
        .method_not_allowed_fallback(wrong_method)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::guard,
        ))
        // Outermost, so an oversized body is refused before the guard reads a
        // cookie or the store is touched. See [`MAX_BODY_BYTES`].
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// `GET /openapi.json` - this API's own OpenAPI 3.1 document.
///
/// Served *behind* the viewer guard, with no [`auth`] `PUBLIC_PATHS` exception:
/// the description of a closed API is part of what being closed by default
/// protects, and an unauthenticated caller learning every path and parameter
/// would undo what the guard's answering 401 ahead of routing is for. Nothing
/// is lost by that. The document is a committed artifact at
/// `crates/service/openapi/fluid-v1.json`, and the UI's client generator reads
/// the file rather than this route, so tooling never needs a running server -
/// let alone an unauthenticated one.
#[utoipa::path(
    get,
    path = "/api/v1/openapi.json",
    tag = "meta",
    operation_id = "get_openapi_document",
    summary = "This API's own OpenAPI 3.1 document.",
    description = "Served behind the viewer guard like every other data route: \
                   the description of a closed API is part of what being closed \
                   by default protects. Tooling does not need this route, since \
                   the document is a committed artifact in the repository at \
                   `crates/service/openapi/fluid-v1.json`.",
    responses(
        (
            status = 200,
            description = "This document. Behind the viewer guard like every \
                           other data route.",
            body = Object,
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
async fn openapi_json() -> axum::Json<utoipa::openapi::OpenApi> {
    axum::Json(openapi_document())
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

/// Refuse a mutation on a read-only instance, ahead of every other check that
/// would otherwise touch the store, the config or a credential.
///
/// One spelling for every admin module rather than one per module, so a
/// read-only instance answers the same way whichever settings surface was
/// asked. The `crystalline` CLI on the server that holds the data is the
/// recovery path - there is no flag that reopens this surface, on purpose
/// (resolved ambiguity 7 in the plan).
pub(super) fn refuse_read_only(state: &RestState) -> Result<(), ApiError> {
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only; changes are disabled here - use the \
             `crystalline` CLI on the server that holds the data",
        ));
    }
    Ok(())
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
