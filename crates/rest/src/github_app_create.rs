//! Creating the instance's GitHub App from the browser, through GitHub's
//! manifest flow ("registering a GitHub App from a manifest").
//!
//! Three hops. `POST /settings/github/app/manifest` (an admin session, under
//! the CSRF check like every other state-changing route) mints a state,
//! remembers who asked and answers the manifest and where to post it. The
//! page posts the manifest as a form to GitHub's new-App page; the admin
//! confirms there and GitHub creates the App, then sends the browser back to
//! `GET /github/app/created` with a code and that state. The callback takes
//! the record the state names, checks that the request comes from the very
//! admin session that asked, converts the code into the App's credentials
//! (`POST /app-manifests/{code}/conversions`) and stores them through the
//! path `crystalline github app set` takes, then sends the browser to the
//! settings page.
//!
//! The callback is a GET that plants a GitHub App, so the binding is the
//! whole point: without it a forged callback could make this instance run
//! somebody else's App. The state is random, kept only in this process,
//! single use (taken, not read) and lives an hour, the time GitHub gives the
//! code ([`MANIFEST_TTL`]). It is bound to a hash of the asking session's
//! CSRF token under a label of this flow's own, so a state minted for the
//! person link can never stand in here. An App already configured is never
//! overwritten: the form is not handed out then, and a callback that arrives
//! after one was stored is refused before the code is converted.
//!
//! No secret leaves the server: the answer to the conversion goes straight
//! into the App store, the code is never logged or echoed, and the settings
//! status names the App without any of its secrets.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use crystalline_remote::github::app::{
    ManifestParams, app_manifest, convert_manifest, manifest_post_url, valid_org_login,
};

use super::auth::{Identity, no_store};
use super::pending::{PendingRecord, PendingStore, StateMiss};
use super::{ApiError, ApiJson, ApiQuery, ProblemDetail, RestState, refuse_read_only};
use crate::engine::{APP_ALREADY_CONFIGURED, EngineError};

/// How long a state handed out with the form stays good: GitHub's own limit
/// for the whole manifest flow.
pub const MANIFEST_TTL: Duration = Duration::from_secs(60 * 60);

/// Where GitHub sends the browser back after creating the App, under
/// `/api/v1`. The manifest's `redirect_url`.
pub const CREATED_PATH: &str = "/github/app/created";

/// Where GitHub sends the browser after the App was installed, under
/// `/api/v1`. The manifest's `setup_url`.
pub const SETUP_PATH: &str = "/github/app/setup";

/// The forms handed out and not yet come back, keyed by state.
pub struct GithubAppManifests {
    pending: Mutex<PendingStore<ManifestPending>>,
}

impl Default for GithubAppManifests {
    fn default() -> GithubAppManifests {
        GithubAppManifests {
            pending: Mutex::new(PendingStore::with_ttl(MANIFEST_TTL)),
        }
    }
}

/// One form in flight: who asked, and from which session.
struct ManifestPending {
    /// The admin account that asked for the form.
    account: String,
    /// The sha256 of the asking session's CSRF token (see [`session_hash`]).
    session_hash: String,
    /// When the form was handed out, on the engine's link clock.
    started: Instant,
}

impl PendingRecord for ManifestPending {
    fn started(&self) -> Instant {
        self.started
    }
}

/// The value a form is bound to: a hash of the session's CSRF token under a
/// label of this flow's own.
fn session_hash(csrf: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"crystalline github app manifest session\0");
    hasher.update(csrf.as_bytes());
    crystalline_index::hex_lower(&hasher.finalize())
}

/// A fresh state: 32 random bytes, lowercase hex.
fn new_state() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG is available");
    crystalline_index::hex_lower(&bytes)
}

/// Whether GitHub can reach `public_url`: its host is neither a loopback
/// nor a private, link-local, multicast, reserved or otherwise local
/// address. Only then does the manifest set up a webhook. A host name is
/// taken as reachable unless it is a single label (`kb`), `localhost`, or
/// under `.localhost`, `.local`, `.internal`, `.lan` or `.home.arpa`; what
/// DNS makes of the others is not this check's to know.
pub(crate) fn webhook_public(public_url: &str) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    fn v4_public(ip: Ipv4Addr) -> bool {
        let [a, b, ..] = ip.octets();
        let shared = a == 100 && (64..128).contains(&b);
        // 240.0.0.0/4 is reserved, and holds the broadcast address too.
        let reserved = a >= 240;
        !(ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_multicast()
            || reserved
            || shared)
    }
    let Ok(url) = url::Url::parse(public_url.trim()) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            const LOCAL: [&str; 5] = [".localhost", ".local", ".internal", ".lan", ".home.arpa"];
            name.contains('.') && !LOCAL.iter().any(|suffix| name.ends_with(suffix))
        }
        Some(url::Host::Ipv4(ip)) => v4_public(ip),
        Some(url::Host::Ipv6(ip)) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return v4_public(v4);
            }
            let first = ip.segments()[0];
            let unique_local = first & 0xfe00 == 0xfc00;
            let link_local = first & 0xffc0 == 0xfe80;
            !(IpAddr::V6(ip).is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || unique_local
                || link_local)
        }
        None => false,
    }
}

/// What `POST /settings/github/app/manifest` takes.
#[derive(Debug, Default, serde::Deserialize, utoipa::ToSchema)]
pub struct GithubAppManifestBody {
    /// The GitHub organization the App is created for. Absent: the admin's
    /// own GitHub account.
    #[schema(example = "acme-corp")]
    #[serde(default)]
    pub org: Option<String>,
}

/// What `POST /settings/github/app/manifest` answers: the form to post.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct GithubAppManifestResponse {
    /// Where to post the form: GitHub's new-App page for the account or the
    /// organization, on the GitHub Enterprise Server host on GHES. Carries
    /// the state.
    #[schema(example = "https://github.com/settings/apps/new?state=4f2a...")]
    pub post_url: String,
    /// The manifest. The page posts it JSON-encoded as the form field
    /// `manifest`.
    #[schema(value_type = Object)]
    pub manifest: serde_json::Value,
    /// The state GitHub sends back with the code. Good for one callback,
    /// for an hour, from this session only.
    #[schema(example = "4f2a...")]
    pub state: String,
}

/// `POST /settings/github/app/manifest` - the form that creates the App.
#[utoipa::path(
    post,
    path = "/api/v1/settings/github/app/manifest",
    tag = "settings",
    operation_id = "start_github_app_manifest",
    summary = "The manifest form that creates this instance's GitHub App on GitHub.",
    description = "Admin only, and refused on a read-only instance. Answers \
                   the GitHub App manifest and the url to post it to (as the \
                   form field `manifest`): GitHub's new-App page for the \
                   admin's account or for `org`, on the Enterprise Server \
                   host on GHES. GitHub creates the App there and sends the \
                   browser back to `GET /github/app/created` with a code and \
                   the state, which is good for one callback, for an hour, \
                   from this session only. The manifest asks for Contents and \
                   Pull requests write, Metadata and Members read and nothing \
                   else, and sets up the webhook only when \
                   `service.public_url` is reachable from GitHub.",
    request_body = GithubAppManifestBody,
    responses(
        (status = 200, description = "The form to post.", body = GithubAppManifestResponse),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, a cookie session did \
                           not echo its CSRF token, the request carries no \
                           session to bind the state to, or this instance is \
                           read-only.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 409,
            description = "A GitHub App is already configured (remove it \
                           first with `crystalline github app remove`), or \
                           `service.public_url` is not set. The detail names \
                           the fix.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "`org` is not a GitHub organization name.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 503,
            description = "Too many forms are in flight at once; try again \
                           in a moment.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn manifest(
    State(state): State<RestState>,
    identity: Identity,
    ApiJson(body): ApiJson<GithubAppManifestBody>,
) -> Result<Response, ApiError> {
    identity.require_admin()?;
    let user = identity.require_account()?;
    refuse_read_only(&state)?;
    let csrf = identity.csrf.as_deref().ok_or_else(|| {
        ApiError::forbidden(
            "this request carries no browser session to bind the App's creation to: create the \
             App from the GitHub settings page",
        )
    })?;
    let org = match body.org.as_deref().map(str::trim) {
        None => None,
        Some(org) if valid_org_login(org) => Some(org.to_string()),
        Some(_) => {
            return Err(ApiError::unprocessable(
                "org must be a GitHub organization name: letters, digits and single hyphens, at \
                 most 39 characters",
            ));
        }
    };
    if state.engine.github_app_local_status().configured {
        return Err(ApiError::conflict(APP_ALREADY_CONFIGURED));
    }
    let config = state.engine.config();
    let Some(public) = crate::settings::public_base(&config) else {
        return Err(ApiError::conflict(
            "creating the GitHub App needs service.public_url: GitHub sends the browser back to \
             it, and the App's callback and webhook live under it. Set it with crystalline config \
             set service.public_url <url>",
        ));
    };
    let public_url = public.to_string();
    let params = ManifestParams {
        instance_label: config
            .instance_name()
            .unwrap_or_else(|| "instance".to_string()),
        webhook_public: webhook_public(&public_url),
        public_url,
        org,
    };
    let api_url = config.github.as_ref().and_then(|g| g.api_url.clone());
    let auth_base = crystalline_remote::github::auth::auth_base(api_url.as_deref());
    let flow_state = new_state();
    let now = state.engine.link_clock_now();
    state
        .github_app_manifests
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert_at(
            flow_state.clone(),
            ManifestPending {
                account: user.name.clone(),
                session_hash: session_hash(csrf),
                started: now,
            },
            now,
        )
        .map_err(|_| {
            ApiError::service_unavailable(
                "too many GitHub App forms are in flight at once; try again in a moment",
            )
            .retry_after(30)
        })?;
    let answer = GithubAppManifestResponse {
        post_url: manifest_post_url(&auth_base, params.org.as_deref(), &flow_state),
        manifest: app_manifest(&params),
        state: flow_state,
    };
    Ok((no_store(), Json(answer)).into_response())
}

/// The query GitHub sends the browser back with. Every field optional, so a
/// malformed callback is answered by this module rather than by the
/// extractor. No `Debug`: it carries the code.
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CreatedQuery {
    /// The code GitHub converts into the App's credentials, once.
    code: Option<String>,
    /// The state the form was handed out with.
    state: Option<String>,
}

/// What a state that cannot be used says, whatever the reason. The reason
/// goes to the log only.
const STATE_REFUSED: &str = "This link back from GitHub is not valid: it was already used, it is \
     more than an hour old, or it was started from another browser session. GitHub may have \
     created the App already: delete it in your GitHub developer settings, then create it again \
     from the GitHub settings page here.";

/// What a callback without a signed-in session says.
const NO_SESSION: &str = "Sign in as the admin who started creating the GitHub App, in the \
     same browser, and open this link again. If the session has ended, delete the new App in \
     your GitHub developer settings and create it again from the GitHub settings page here.";

/// What a callback by an account that is not an admin says.
const NOT_ADMIN: &str = "Only an admin of this instance can create its GitHub App.";

/// A page for the admin, with a way back to the settings. Every text that
/// reaches it is written by this server, and it is escaped all the same.
fn page(state: &RestState, status: StatusCode, text: &str) -> Response {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let back = escape(&format!("{}settings/github", state.base_path.href()));
    let text = escape(text);
    let body = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Create GitHub App</title></head>\
         <body style=\"font-family: system-ui, sans-serif; max-width: 36rem; margin: 4rem auto; \
         padding: 0 1rem; line-height: 1.5\"><h1>Create GitHub App</h1><p>{text}</p>\
         <p><a href=\"{back}\">Back to the GitHub settings</a></p></body></html>\n"
    );
    (
        status,
        no_store(),
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'",
            ),
        ],
        body,
    )
        .into_response()
}

/// A 303 to the settings page under the base path, with `query`.
fn to_settings(state: &RestState, query: &str) -> Response {
    let location = format!("{}settings/github?{query}", state.base_path.href());
    (
        StatusCode::SEE_OTHER,
        no_store(),
        [(header::LOCATION, location)],
    )
        .into_response()
}

/// `GET /github/app/created` - where GitHub sends the browser back after
/// creating the App.
#[utoipa::path(
    get,
    path = "/api/v1/github/app/created",
    tag = "settings",
    operation_id = "finish_github_app_manifest",
    summary = "Where GitHub sends the browser back after creating the GitHub App.",
    description = "Needs the admin session that asked for the manifest form \
                   (the session cookie is SameSite=Lax, so it rides GitHub's \
                   redirect). The state must name a form this session was \
                   handed in the last hour and is spent by this call. Then \
                   the code is converted into the App's credentials, which \
                   are stored as `crystalline github app set` stores them, \
                   and the browser is sent to `/settings/github?app=created`. \
                   Refused without converting the code when a GitHub App is \
                   already configured. It is a browser landing, so every \
                   refusal is an HTML page, never JSON, and no answer \
                   carries a secret.",
    params(CreatedQuery),
    responses(
        (
            status = 303,
            description = "The App is stored: to `/settings/github?app=created`.",
        ),
        (
            status = 401,
            description = "An HTML page: no signed-in session. The state is \
                           not spent.",
            content_type = "text/html",
        ),
        (
            status = 403,
            description = "An HTML page: the caller is not an admin, this \
                           instance is read-only, or the state is missing, \
                           unknown, used, expired or another session's.",
            content_type = "text/html",
        ),
        (
            status = 409,
            description = "An HTML page: a GitHub App is already configured, \
                           and the code was not converted.",
            content_type = "text/html",
        ),
        (
            status = 502,
            description = "An HTML page: GitHub did not convert the code, or \
                           the App it answered could not be stored. The text \
                           says why, with the GitHub Enterprise Server \
                           version when the server lacks the endpoint.",
            content_type = "text/html",
        ),
    ),
)]
pub async fn created(
    State(state): State<RestState>,
    identity: Identity,
    ApiQuery(query): ApiQuery<CreatedQuery>,
) -> Response {
    // A browser landing: no session and no admin are pages, and neither
    // spends the state, since nothing proved who sent it.
    let user = match identity.require_account() {
        Ok(user) => user,
        Err(_) => {
            tracing::warn!("a GitHub App creation came back without a signed-in session");
            return page(&state, StatusCode::UNAUTHORIZED, NO_SESSION);
        }
    };
    if identity.require_admin().is_err() {
        tracing::warn!(
            "a GitHub App creation came back to {}, who is not an admin",
            user.name
        );
        return page(&state, StatusCode::FORBIDDEN, NOT_ADMIN);
    }
    if let Err(e) = refuse_read_only(&state) {
        return page(&state, e.status, &e.detail);
    }
    let refused = |reason: &str| {
        tracing::warn!(
            "a GitHub App creation that came back to {} was refused: {reason}",
            user.name
        );
        page(&state, StatusCode::FORBIDDEN, STATE_REFUSED)
    };
    let Some(flow_state) = query.state.as_deref() else {
        return refused("no state");
    };
    let now = state.engine.link_clock_now();
    // Taken before anything is compared: a state is spent by the first
    // admin callback that names it, whoever sends it.
    let taken = state
        .github_app_manifests
        .pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_at(flow_state, now);
    let pending = match taken {
        Ok(pending) => pending,
        Err(StateMiss::Unknown) => return refused("unknown or already used state"),
        Err(StateMiss::Expired) => return refused("the form was handed out more than an hour ago"),
    };
    let same_session = identity
        .csrf
        .as_deref()
        .map(session_hash)
        .is_some_and(|hash| {
            super::auth::constant_time_eq(hash.as_bytes(), pending.session_hash.as_bytes())
        });
    if !same_session || pending.account != user.name {
        return refused("the state belongs to another session");
    }
    // Before the code is converted: a configured App is never overwritten,
    // and its secrets need not even be fetched.
    if state.engine.github_app_local_status().configured {
        tracing::warn!(
            "a GitHub App creation by {} was refused: an App is already configured",
            user.name
        );
        return page(&state, StatusCode::CONFLICT, &already_configured_text());
    }
    let Some(code) = query.code.as_deref().filter(|code| !code.is_empty()) else {
        tracing::warn!(
            "the GitHub App creation by {} could not finish: GitHub sent no code",
            user.name
        );
        return orphan_page(
            &state,
            "GitHub sent no code back, so the App's credentials cannot be fetched",
        );
    };
    let api_url = state.engine.config().github.and_then(|g| g.api_url);
    let credentials = match convert_manifest(api_url.as_deref(), code).await {
        Ok(credentials) => credentials,
        Err(e) => {
            tracing::warn!(
                "GitHub did not convert the code of the GitHub App created by {}: {e}",
                user.name
            );
            return orphan_page(&state, &e.to_string());
        }
    };
    match state.engine.create_github_app(credentials).await {
        Ok(_) => {}
        Err(EngineError::Conflict(text)) if text == APP_ALREADY_CONFIGURED => {
            tracing::warn!(
                "a GitHub App creation by {} was refused: an App is already configured",
                user.name
            );
            return page(&state, StatusCode::CONFLICT, &already_configured_text());
        }
        Err(e) => {
            tracing::warn!(
                "the GitHub App created by {} could not be stored: {e}",
                user.name
            );
            let e = ApiError::from(e);
            if e.status == StatusCode::FORBIDDEN {
                return page(&state, e.status, &e.detail);
            }
            return orphan_page(&state, &e.detail);
        }
    }
    tracing::info!("{} created this instance's GitHub App", user.name);
    // Creating the App is the intent to use GitHub, as a connect is.
    if !state.engine.github_enabled()
        && let Err(e) = state
            .engine
            .configure(&crate::engine::ConfigureAction::Set {
                key: "github.enabled".to_string(),
                value: "true".to_string(),
            })
            .await
    {
        tracing::warn!("github.enabled could not be switched on after the App was created: {e}");
    }
    to_settings(&state, "app=created")
}

/// What every page after GitHub created the App and nothing was stored ends
/// with.
const ORPHAN: &str = "GitHub has created the App already: delete it in your GitHub developer \
     settings, then create it again from the GitHub settings page here.";

/// A 502 page: `text`, then what to do with the App GitHub created.
fn orphan_page(state: &RestState, text: &str) -> Response {
    let text = text.trim_end_matches('.');
    page(state, StatusCode::BAD_GATEWAY, &format!("{text}. {ORPHAN}"))
}

/// The sentence a creation refused over an App already configured says.
fn already_configured_text() -> String {
    format!(
        "The App was not stored: {APP_ALREADY_CONFIGURED}. GitHub has created the new App \
         already: delete it in your GitHub developer settings if you do not need it."
    )
}

/// `GET /github/app/setup` - where GitHub sends the browser after the App
/// was installed.
#[utoipa::path(
    get,
    path = "/api/v1/github/app/setup",
    tag = "settings",
    operation_id = "github_app_installed",
    summary = "Where GitHub sends the browser after the GitHub App was installed.",
    description = "The manifest's `setup_url`. Changes nothing and reads \
                   nothing GitHub sent: it sends the browser to \
                   `/settings/github?app=installed`.",
    responses(
        (
            status = 303,
            description = "To `/settings/github?app=installed`.",
        ),
    ),
)]
pub async fn setup(State(state): State<RestState>) -> Response {
    to_settings(&state, "app=installed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_address_github_can_reach_gets_a_webhook() {
        // 203.0.113.0/24 and 2001:db8::/32 are documentation ranges: they
        // stand in for public addresses here, which is what the check reads
        // them as, since it only knows the local and private ranges.
        for public in [
            "https://kb.example.com",
            "https://kb.example.com/fluid",
            "http://203.0.113.9:7411",
            "https://[2001:db8::1]",
            "https://8.8.8.8",
        ] {
            assert!(webhook_public(public), "{public}");
        }
        for local in [
            "http://localhost:7411",
            "http://kb.localhost",
            "http://kb.local",
            "http://127.0.0.1:7411",
            "http://10.1.2.3",
            "http://172.16.0.1",
            "http://172.31.255.1",
            "http://192.168.1.10",
            "http://169.254.0.5",
            "http://100.64.0.1",
            "http://0.0.0.0",
            "http://[::1]",
            "http://[::]",
            "http://[fd00::1]",
            "http://[fe80::1]",
            "http://[::ffff:192.168.1.1]",
            "http://kb",
            "http://kb.internal",
            "http://kb.lan",
            "http://kb.home.arpa",
            "http://224.0.0.1",
            "http://239.1.2.3",
            "http://240.0.0.1",
            "http://255.255.255.255",
            "http://[ff02::1]",
            "not a url",
        ] {
            assert!(!webhook_public(local), "{local}");
        }
    }

    #[test]
    fn the_session_hash_is_not_the_link_flows() {
        // The same CSRF token must not yield the value the person link
        // binds to, so a state of one flow never stands in for the other.
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, b"crystalline github link session\0");
        sha2::Digest::update(&mut hasher, b"token");
        let link = crystalline_index::hex_lower(&sha2::Digest::finalize(hasher));
        assert_ne!(session_hash("token"), link);
    }

    /// The manifest's urls are this router's paths: the callback is the
    /// link flow's, the webhook the webhook route's, and the two landings
    /// are this module's.
    #[test]
    fn the_manifests_paths_are_the_ones_this_router_serves() {
        use crystalline_remote::github::app as remote;
        assert_eq!(
            format!("/api/v1{}", super::super::github_link::CALLBACK_PATH),
            remote::LINK_CALLBACK_PATH
        );
        assert_eq!(
            format!("/api/v1{}", super::super::github_webhook::PATH),
            remote::WEBHOOK_PATH
        );
        assert_eq!(
            format!("/api/v1{CREATED_PATH}"),
            remote::MANIFEST_CREATED_PATH
        );
        assert_eq!(format!("/api/v1{SETUP_PATH}"), remote::MANIFEST_SETUP_PATH);
    }
}
