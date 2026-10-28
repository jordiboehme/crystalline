//! The two browser routes of the GitHub App web authorization, the half of the
//! flow a person walks through with their own eyes.
//!
//! Everything about the handshake itself - the nonce, the state, the PKCE pair,
//! the exchange and the stored credential - belongs to the engine
//! ([`crate::engine`]'s `github_web`); this module is the HTTP skin on it: who
//! the browser is, where it is sent next, and what it is shown when the
//! journey ends.
//!
//! - [`connect`] is where a minted link lands. It is the one route that needs
//!   a browser session, and it needs it for a reason the flow would be unsafe
//!   without: the engine refuses a nonce presented by any account but the one
//!   it was minted for, and the session is what names that account. A browser
//!   with no session is sent to sign in and back here, so the link survives
//!   the detour.
//! - [`callback`] is where GitHub sends the browser back. It carries no
//!   session requirement at all, deliberately: what authenticates it is the
//!   single-use state the engine minted and is about to spend, and a flow that
//!   also demanded the cookie would break for a person who finished the
//!   authorization in a different browser profile than the one that holds it.
//!
//! Both answer HTML rather than the problem detail the rest of this surface
//! answers with. They are navigations, never fetched by a script, so a person
//! reads the outcome; the status codes are still the surface's own (403 for a
//! link that belongs to somebody else, 422 for one that is spent or expired).
//! Every page is self-contained - no script, no image, no font, nothing
//! fetched - and says so in a `Content-Security-Policy` that allows nothing
//! but the inline stylesheet, with `no-store` and `no-referrer` beside it so
//! the authorization code in the callback's own url never travels anywhere.

use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use super::auth::{Identity, no_store};
use super::{ApiError, RestState};

/// The route a minted link points at, relative to the `/api/v1` mount. The
/// engine owns the absolute spelling, because it is what builds the link out
/// of `service.public_url`; this is that constant minus the mount, pinned to it
/// by a test below.
pub const CONNECT_PATH: &str = "/auth/github/connect";

/// The route GitHub sends the browser back to, relative to the `/api/v1`
/// mount. The string an operator registers with the GitHub App is this one
/// below `service.public_url`, which is what the engine builds and sends.
pub const CALLBACK_PATH: &str = "/auth/github/callback";

/// The mount every path on this surface hangs below, for the two absolute urls
/// built here: the sign-in detour and the link the detour comes back to.
const API_PREFIX: &str = "/api/v1";

/// What a result page may reach: nothing, except the stylesheet written into
/// the page itself.
const PAGE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; \
                        form-action 'none'; frame-ancestors 'none'";

/// The RFC 3986 unreserved set, as the engine encodes a nonce with: what goes
/// back into a `return_to` comes out spelled exactly as it arrived.
const URL_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The query of [`connect`]: the nonce the link carries.
///
/// Optional rather than required so a truncated link is a page a person can
/// read rather than an extractor rejection in a language they cannot.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ConnectQuery {
    /// The single-use nonce the link was minted with.
    #[serde(default)]
    pub n: Option<String>,
}

/// The query of [`callback`]: what GitHub sends back, whichever way the person
/// answered.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    /// The state this instance minted when it sent the browser to GitHub.
    #[serde(default)]
    pub state: Option<String>,
    /// The authorization code to exchange, on the way back from an approval.
    #[serde(default)]
    pub code: Option<String>,
    /// GitHub's own error code when the person declined, or when the app is
    /// not in a state that can be authorized.
    #[serde(default)]
    pub error: Option<String>,
    /// GitHub's sentence about that error.
    #[serde(default)]
    pub error_description: Option<String>,
}

/// `GET /auth/github/connect` - spend a minted link and send the browser on to
/// GitHub.
///
/// Public by path, like every other route a browser reaches before it can be
/// anybody: the handler, not the guard, decides what a session-less request
/// gets, and what it gets is the way to make one rather than a 401 nobody can
/// act on. With single sign-on configured that is the provider, with
/// `return_to` naming this same link so the person lands back on it; without
/// one it is a page saying to sign in and open the link again, because a
/// password sign-in is a screen in the UI rather than a route to redirect to.
#[utoipa::path(
    get,
    path = "/api/v1/auth/github/connect",
    tag = "auth",
    operation_id = "github_web_connect",
    params(ConnectQuery),
    summary = "Spend a GitHub connection link and redirect to GitHub.",
    description = "Where a link minted by `POST /me/github-identity/connect` \
                   (or by the `configure` MCP tool) lands. Needs a browser \
                   session as the same account the link was minted for: a \
                   request with none is sent to sign in and back here, and one \
                   signed in as another account is refused 403, which is what \
                   stops a link forwarded to somebody else from connecting \
                   their GitHub account to the sender's. The nonce is spent \
                   whatever the outcome. Answers HTML, never a problem detail: \
                   a person reads this, not a script.",
    responses(
        (
            status = 302,
            description = "Follow `location`: GitHub's authorization endpoint \
                           when the link was good, this instance's sign-in \
                           when the browser carried no session.",
            headers(("location" = String, description = "Where the browser goes next.")),
        ),
        (status = 400, description = "The link carries no nonce.", content_type = "text/html"),
        (
            status = 401,
            description = "No session, and no single sign-on to send the \
                           browser to: sign in and open the link again.",
            content_type = "text/html",
        ),
        (
            status = 403,
            description = "The session names another account than the link was \
                           minted for.",
            content_type = "text/html",
        ),
        (
            status = 422,
            description = "The link is spent or expired, or this instance is \
                           not set up for the browser flow.",
            content_type = "text/html",
        ),
    ),
)]
pub async fn connect(
    State(state): State<RestState>,
    identity: Identity,
    Query(query): Query<ConnectQuery>,
) -> Response {
    let Some(nonce) = query.n.as_deref().map(str::trim).filter(|n| !n.is_empty()) else {
        return page(
            StatusCode::BAD_REQUEST,
            "This connection link is incomplete",
            "The link is missing the part that says which connection it is for. Start the \
             connection again from your client to get a fresh one.",
        );
    };
    let Ok(user) = identity.require_account() else {
        return sign_in_first(&state, nonce);
    };
    match state.engine.github_web_authorize(&user.name, nonce) {
        Ok(url) => (no_store(), found(&url)).into_response(),
        Err(e) => refusal(e.into()),
    }
}

/// `GET /auth/github/callback` - finish the authorization GitHub is sending the
/// browser back from.
///
/// Public by path and by design: the state it carries is single use, was minted
/// by this process a few minutes ago and names the account the credential is
/// stored for, so nothing here is decided by who the browser says it is. The
/// state is spent whatever the outcome.
#[utoipa::path(
    get,
    path = "/api/v1/auth/github/callback",
    tag = "auth",
    operation_id = "github_web_callback",
    params(CallbackQuery),
    summary = "Finish a GitHub authorization and store the credential.",
    description = "The address registered with the GitHub App, below \
                   `service.public_url`. Exchanges the code for the account \
                   named by the single-use state, validates the token against \
                   GitHub and stores it as that account's own credential; the \
                   token is never echoed and never leaves the server. Answers \
                   a small self-contained HTML page saying whether the \
                   connection stands, with `no-store` and `no-referrer` so the \
                   code in this url travels nowhere.",
    responses(
        (status = 200, description = "Connected. The page names the GitHub login.", content_type = "text/html"),
        (
            status = 400,
            description = "The person declined at GitHub, or the callback \
                           carries neither a code nor an error.",
            content_type = "text/html",
        ),
        (
            status = 403,
            description = "This instance is read-only, so no credential could \
                           be stored.",
            content_type = "text/html",
        ),
        (
            status = 422,
            description = "The authorization is spent or expired, or GitHub \
                           refused the exchange.",
            content_type = "text/html",
        ),
    ),
)]
pub async fn callback(
    State(state): State<RestState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if let Some(error) = query
        .error
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        let reason = query
            .error_description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(error);
        return page(
            StatusCode::BAD_REQUEST,
            "GitHub did not authorize this connection",
            &format!(
                "GitHub answered: {reason}. Nothing was connected. Start the connection again \
                 from your client if you meant to approve it."
            ),
        );
    }
    let (Some(handshake), Some(code)) = (
        query.state.as_deref().filter(|s| !s.is_empty()),
        query.code.as_deref().filter(|c| !c.is_empty()),
    ) else {
        return page(
            StatusCode::BAD_REQUEST,
            "This authorization is incomplete",
            "GitHub sent the browser back without an authorization to finish. Start the \
             connection again from your client.",
        );
    };
    match state
        .engine
        .complete_github_web_connect(handshake, code)
        .await
    {
        Ok(identity) => page(
            StatusCode::OK,
            "Connected to GitHub",
            &format!(
                "The account '{}' is connected to GitHub as '{}'. You can close this tab and \
                 return to your client.",
                identity.account,
                identity.login.as_deref().unwrap_or("unknown"),
            ),
        ),
        Err(e) => refusal(e.into()),
    }
}

/// Where a browser with no session is sent, and how it finds its way back.
///
/// Through the provider when there is one: the sign-in carries this same link
/// as its `return_to`, so the nonce - still unspent, because nothing has judged
/// it yet - is presented again by a browser that is now somebody. With no
/// provider there is nowhere to redirect to, since a password sign-in is a
/// screen in the UI rather than a route, so the page says what to do in one
/// sentence.
fn sign_in_first(state: &RestState, nonce: &str) -> Response {
    let link = format!(
        "{API_PREFIX}{CONNECT_PATH}?n={}",
        utf8_percent_encode(nonce, URL_COMPONENT)
    );
    if state.oidc.is_some() {
        let to = format!(
            "{API_PREFIX}{}?return_to={}",
            super::oidc::LOGIN_PATH,
            utf8_percent_encode(&link, URL_COMPONENT)
        );
        return (no_store(), found(&to)).into_response();
    }
    page(
        StatusCode::UNAUTHORIZED,
        "Sign in first",
        "This browser is not signed in to Crystalline. Sign in, then open the connection link \
         again - it stays usable for five minutes.",
    )
}

/// One refused outcome as a page, keeping the status this surface would have
/// answered with and the engine's own sentence about what to do next.
fn refusal(error: ApiError) -> Response {
    page(
        error.status,
        "This connection could not be made",
        &error.detail,
    )
}

/// A 302 to `location`, the shape the single sign-on routes redirect with.
fn found(location: &str) -> Response {
    (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response()
}

/// One self-contained page: a heading, a sentence, and nothing to fetch.
fn page(status: StatusCode, title: &str, message: &str) -> Response {
    let body = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>body{{font:16px/1.5 system-ui,sans-serif;margin:0;\
         display:flex;min-height:100vh;align-items:center;justify-content:center;\
         background:#f6f7f9;color:#1b1f23}}main{{max-width:34rem;padding:2rem;background:#fff;\
         border-radius:.75rem;box-shadow:0 1px 3px rgba(0,0,0,.12)}}h1{{font-size:1.25rem;\
         margin:0 0 .75rem}}p{{margin:0}}</style></head><body><main><h1>{title}</h1>\
         <p>{message}</p></main></body></html>\n",
        title = escape(title),
        message = escape(message),
    );
    (
        status,
        no_store(),
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, PAGE_CSP),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response()
}

/// Every character that could end the text it sits in, spelled as an entity.
/// GitHub's words and an account name both reach a page through here.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_are_the_engine_s_own_below_this_mount() {
        assert_eq!(
            format!("{API_PREFIX}{CONNECT_PATH}"),
            crate::engine::GITHUB_CONNECT_PATH
        );
        assert_eq!(
            format!("{API_PREFIX}{CALLBACK_PATH}"),
            crate::engine::GITHUB_CALLBACK_PATH
        );
    }

    #[test]
    fn everything_that_could_end_the_markup_is_escaped() {
        assert_eq!(
            escape("<script>alert(\"x\" & 'y')</script>"),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#x27;y&#x27;)&lt;/script&gt;"
        );
    }

    #[test]
    fn a_page_carries_no_fetchable_resource_and_no_store() {
        let response = page(StatusCode::OK, "Connected", "all good");
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            PAGE_CSP
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    }
}
