//! The resource-server half of OAuth for MCP clients: what this instance calls
//! itself, and the two documents a client discovers it through.
//!
//! An MCP server is an OAuth 2.1 resource server, and a hosted client finds its
//! way in by reading metadata rather than by being told: the `401` at the
//! transport carries a pointer to the protected-resource document, that
//! document names the authorization server, and the authorization-server
//! document names the three endpoints the flow runs against. All of it is one
//! fact spelled several ways - the origin this instance is reached at - which
//! is why the rule for deriving that origin lives here beside the documents
//! rather than in each of them.
//!
//! # Why the origin is the resource identifier
//!
//! RFC 8707 makes an access token an audience-bound credential: the client asks
//! for a token for a named resource, the server records which resource it
//! minted for, and every check compares the two. The name has to be something
//! both sides arrive at without agreeing on anything first, and for an MCP
//! server that is the address the client was pointed at - scheme, host, port,
//! no path. So the resource is derived from the request, exactly as the single
//! sign-on callback address is ([`super::auth::request_origin`]), with the same
//! override for the deployment whose `Host` is rewritten in front of this
//! process.
//!
//! The one tolerance is a trailing slash, because a person typing an address
//! into a client adds or omits one without meaning anything by it. That
//! tolerance is [`super::auth_store::normalize_resource`]'s, called here rather
//! than re-implemented: two spellings of one rule is how an audience check
//! develops a hole.
//!
//! # Why the documents exist unconditionally
//!
//! Both paths are declared whether or not `auth.oauth` is on, and answer `404`
//! while it is off, for the reason the single sign-on routes do: the answer
//! must not depend on configuration the caller cannot see. Mounting them only
//! when the setting is on would leave the paths to whatever is behind them -
//! the app shell for a browser's `Accept`, the MCP gate's own `401` for an API
//! client's - and a discovery probe would read either as something other than
//! "there is no OAuth here".

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use crystalline_core::config::GlobalConfig;
use serde_json::{Value, json};

use super::ApiError;
use super::auth::request_origin;
use super::auth_store::normalize_resource;

/// RFC 9728 protected resource metadata, at the origin root. A client reads it
/// from the `resource_metadata` parameter of the gate's `401`, and falls back
/// to this path at the root when the challenge names none.
pub const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";

/// RFC 8414 authorization server metadata, at the origin root: the first
/// address a client tries for an issuer that carries no path, which this one
/// does not.
pub const AUTHORIZATION_SERVER_PATH: &str = "/.well-known/oauth-authorization-server";

/// Where a browser is sent to consent, relative to the API mount.
pub const AUTHORIZE_PATH: &str = "/oauth/authorize";

/// Where a code or a refresh token is exchanged, relative to the API mount.
pub const TOKEN_PATH: &str = "/oauth/token";

/// Where a client registers itself, relative to the API mount.
pub const REGISTER_PATH: &str = "/oauth/register";

/// The mount the JSON API is nested at, which the three endpoint paths above
/// are relative to.
///
/// Spelled here because the authorization-server document publishes absolute
/// urls: a client sent to `<origin>/oauth/authorize` would land on the MCP
/// transport, which is the fallback for everything the declared routes do not
/// claim. The two well-known paths are NOT under it - they are root documents
/// by specification - so they are never built through this.
const API_PREFIX: &str = "/api/v1";

/// The `resource_name` the protected-resource document carries: what a consent
/// screen in somebody else's client calls this server.
const RESOURCE_NAME: &str = "Crystalline";

/// How this instance names itself, per request.
///
/// Either the origin of a configured `auth.oidc.redirect_uri` - the one place
/// an operator behind a Host-rewriting proxy has already written the public
/// address - or the origin the request itself says it arrived at. Held by the
/// MCP gate and by the two well-known handlers, so the address a client is told
/// to use, the audience a token is minted for and the audience the gate checks
/// are one answer rather than three.
#[derive(Clone, Debug)]
pub struct OriginRule {
    /// The configured public origin, already parsed down to scheme, host and
    /// port. `None` means derive it from each request.
    override_origin: Option<String>,
}

impl OriginRule {
    /// Read the rule out of `config`.
    ///
    /// The override is read from the `auth.oidc` block directly rather than
    /// through [`super::OidcSettings`], because that resolves to `None` for a
    /// provider that is missing an issuer or a client, and the address in this
    /// key is still the operator's answer to "what is this instance called"
    /// whether or not single sign-on is switched on beside it.
    ///
    /// A value that is not an absolute `http` or `https` url leaves the rule
    /// deriving from the request. It is a fail-safe rather than tolerance: the
    /// settings layer already refuses anything but an absolute https (or
    /// loopback http) url ending at the callback path, so the only way another
    /// value arrives here is through the environment overlay, and refusing to
    /// serve at all would take the instance down over a key that has a
    /// perfectly good default behaviour. The warning is logged once, at
    /// startup, where an operator is still watching.
    ///
    /// **The scheme is what decides, not the presence of a host.**
    /// `Url::origin` answers a tuple origin for a short list of schemes
    /// (`http`, `https`, `ws`, `wss`, `ftp`, `blob`) and an opaque origin for
    /// every other one, and an opaque origin serializes to the literal
    /// `"null"`. A scheme that carries an authority (`foo://kb.example/...`,
    /// `ftp://kb.example/...`) therefore has a host and would still publish
    /// `"null"` as this instance's `resource` and `issuer`, in both documents
    /// and in the gate's challenge. Only the two schemes this surface is ever
    /// served over are accepted, so nothing an operator can mistype reaches a
    /// document.
    pub fn from_config(config: &GlobalConfig) -> OriginRule {
        let configured = config
            .auth_oidc()
            .and_then(|oidc| oidc.redirect_uri.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let override_origin =
            configured.and_then(|value| match openidconnect::url::Url::parse(value) {
                Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
                    Some(url.origin().ascii_serialization())
                }
                _ => {
                    tracing::warn!(
                        "auth.oidc.redirect_uri is not an absolute http or https url, so the \
                         OAuth resource identifier is derived from each request's Host instead"
                    );
                    None
                }
            });
        OriginRule { override_origin }
    }

    /// The origin a request arrived at, in the spelling everything else
    /// compares.
    ///
    /// [`normalize_resource`] is applied once, here, on both branches: the
    /// derived one never carries a trailing slash by construction, so the call
    /// is a no-op there, and one exit means the two branches cannot come to
    /// disagree about the spelling a token's audience is stored in.
    pub fn origin(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let raw = match &self.override_origin {
            Some(configured) => configured.clone(),
            None => request_origin(headers)?,
        };
        Ok(normalize_resource(&raw))
    }

    /// Whether two resource identifiers name the same resource: equal once at
    /// most one trailing slash has come off each.
    ///
    /// For comparing what a client *asked* for against what this instance calls
    /// itself. The gate does not use it - it hands the normalized origin to
    /// [`super::AuthStore::oauth_access_user`] and lets the audience condition
    /// in the statement decide, so no branch in Rust can forget the check.
    pub fn same_resource(a: &str, b: &str) -> bool {
        normalize_resource(a) == normalize_resource(b)
    }
}

/// Everything the OAuth surface needs that is resolved once, at startup.
///
/// `None` from [`OauthServer::new`] is an instance with `auth.oauth` off, which
/// is every instance until an operator turns it on; the rest of the surface
/// reads the option rather than the setting, so a running daemon serves the
/// tier it came up in like every other `auth.*` key.
pub struct OauthServer {
    /// What this instance calls itself. See [`OriginRule`].
    pub origin: OriginRule,
}

impl OauthServer {
    /// The server, or `None` while `auth.oauth` is off.
    pub fn new(config: &GlobalConfig) -> Option<Arc<OauthServer>> {
        config.auth_oauth().then(|| {
            Arc::new(OauthServer {
                origin: OriginRule::from_config(config),
            })
        })
    }
}

/// The url of the protected-resource document on `origin`, which is what a
/// `401` points a client at.
///
/// Deliberately not built through the same helper as the three endpoint urls:
/// this one is a root document and those three live under the API mount, and a
/// shared helper is how one of them would silently acquire the other's prefix.
pub fn resource_metadata_url(origin: &str) -> String {
    format!("{origin}{PROTECTED_RESOURCE_PATH}")
}

/// The absolute url of an endpoint that lives under the API mount.
fn api_url(origin: &str, path: &str) -> String {
    format!("{origin}{API_PREFIX}{path}")
}

/// The two root documents, mounted beside `/health` on the root router.
///
/// `None` is `auth.oauth` off: the paths still exist and answer `404`, so a
/// probe gets one answer whatever it accepts and whatever else the router
/// serves. See the module documentation.
pub fn well_known_routes(oauth: Option<OriginRule>) -> Router {
    Router::new()
        .route(PROTECTED_RESOURCE_PATH, get(protected_resource))
        .route(AUTHORIZATION_SERVER_PATH, get(authorization_server))
        .with_state(oauth)
}

/// `GET /.well-known/oauth-protected-resource`.
async fn protected_resource(
    State(oauth): State<Option<OriginRule>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let origin = enabled(&oauth)?.origin(&headers)?;
    Ok(Json(protected_resource_document(&origin)))
}

/// `GET /.well-known/oauth-authorization-server`.
async fn authorization_server(
    State(oauth): State<Option<OriginRule>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let origin = enabled(&oauth)?.origin(&headers)?;
    Ok(Json(authorization_server_document(&origin)))
}

/// The rule, or the refusal for an instance that serves no OAuth.
///
/// A `404` rather than a `501`: the paths exist unconditionally so the answer
/// does not depend on configuration a caller cannot see, and "there is no
/// authorization server here" is the honest shape of it. The detail names the
/// two settings, because the operator reading it is the one who can change the
/// answer.
fn enabled(oauth: &Option<OriginRule>) -> Result<&OriginRule, ApiError> {
    oauth.as_ref().ok_or_else(|| {
        ApiError::not_found(
            "this instance does not serve OAuth for MCP clients - an administrator turns it on \
             with auth.oauth, which needs auth.mcp beside it, and agents authenticate with a \
             personal MCP token until then",
        )
    })
}

/// RFC 9728 protected resource metadata for `origin`.
///
/// Four members and no `scopes_supported`: a grant here is the whole account's
/// rights until it is revoked, and advertising scopes this server does not
/// narrow by would be a promise nothing keeps.
fn protected_resource_document(origin: &str) -> Value {
    json!({
        "resource": origin,
        // This server is its own authorization server, so the one entry is the
        // origin again. A client takes the first entry.
        "authorization_servers": [origin],
        "bearer_methods_supported": ["header"],
        "resource_name": RESOURCE_NAME,
    })
}

/// RFC 8414 authorization server metadata for `origin`.
///
/// `code_challenge_methods_supported` is not optional in practice: a client
/// that cannot see S256 advertised refuses to start the flow at all.
/// `token_endpoint_auth_methods_supported` is `["none"]` because every client
/// here is a public one - there is no secret to present - and
/// `authorization_response_iss_parameter_supported` says the authorization
/// response carries `iss`, which is how a client detects a mix-up attack
/// between two authorization servers it talks to.
fn authorization_server_document(origin: &str) -> Value {
    json!({
        "issuer": origin,
        "authorization_endpoint": api_url(origin, AUTHORIZE_PATH),
        "token_endpoint": api_url(origin, TOKEN_PATH),
        "registration_endpoint": api_url(origin, REGISTER_PATH),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use crystalline_core::config::{AuthConfig, OidcConfig};

    /// A config carrying `auth.oauth` on and whatever `redirect_uri` says.
    fn config_with(redirect_uri: Option<&str>) -> GlobalConfig {
        GlobalConfig {
            auth: Some(AuthConfig {
                mcp: Some(true),
                oauth: Some(true),
                oidc: redirect_uri.map(|uri| OidcConfig {
                    redirect_uri: Some(uri.to_string()),
                    ..OidcConfig::default()
                }),
                ..AuthConfig::default()
            }),
            ..GlobalConfig::default()
        }
    }

    fn headers_with(host: &str, forwarded: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        if let Some(proto) = forwarded {
            headers.insert("x-forwarded-proto", proto.parse().unwrap());
        }
        headers
    }

    /// The resource identifier is the callback address's own origin: the same
    /// `Host` and the same forwarded-scheme rule, with the path cut off. If the
    /// two ever diverged, an operator would have one public address for
    /// sign-in and another for tokens.
    #[test]
    fn an_origin_is_derived_like_the_callback_address() {
        let rule = OriginRule::from_config(&config_with(None));
        assert_eq!(
            rule.origin(&headers_with("127.0.0.1:7411", None)).unwrap(),
            "http://127.0.0.1:7411",
            "a loopback development server is plain http"
        );
        assert_eq!(
            rule.origin(&headers_with("knowledge.example", Some("https")))
                .unwrap(),
            "https://knowledge.example"
        );
        assert_eq!(
            rule.origin(&headers_with("knowledge.example", None))
                .unwrap(),
            "https://knowledge.example",
            "a non-loopback Host is https with or without a forwarded scheme"
        );
        assert!(
            rule.origin(&HeaderMap::new()).is_err(),
            "a request with no Host names no resource"
        );
        assert!(
            rule.origin(&headers_with("evil.test/x", None)).is_err(),
            "and a Host that could open a path is not interpolated into one"
        );
    }

    /// A configured callback address names the origin, path and all cut off,
    /// and it wins over whatever the request says.
    #[test]
    fn a_configured_callback_address_names_the_origin() {
        let rule = OriginRule::from_config(&config_with(Some(
            "https://knowledge.example/api/v1/auth/oidc/callback",
        )));
        assert_eq!(
            rule.origin(&headers_with("127.0.0.1:7411", None)).unwrap(),
            "https://knowledge.example",
            "the configured public address wins over the upstream's Host"
        );
        assert_eq!(
            rule.origin(&HeaderMap::new()).unwrap(),
            "https://knowledge.example",
            "and it needs no Host at all"
        );
        // A port is part of an origin; a default port is not spelled.
        assert_eq!(
            OriginRule::from_config(&config_with(Some(
                "https://knowledge.example:8443/api/v1/auth/oidc/callback"
            )))
            .origin(&HeaderMap::new())
            .unwrap(),
            "https://knowledge.example:8443"
        );
        // A value the settings layer would have refused, arriving through the
        // environment overlay: the rule falls back to deriving rather than
        // publishing something nobody can reach.
        let unusable = OriginRule::from_config(&config_with(Some("/api/v1/auth/oidc/callback")));
        assert_eq!(
            unusable
                .origin(&headers_with("127.0.0.1:7411", None))
                .unwrap(),
            "http://127.0.0.1:7411"
        );
    }

    /// **Only an http or https url names an origin here**, and every other
    /// scheme falls back to deriving from the request.
    ///
    /// The scheme is what decides, not the presence of a host. `Url::origin`
    /// answers a *tuple* origin for a handful of schemes and an opaque origin
    /// for everything else, and an opaque origin serializes to the literal
    /// "null" - which, published, would make `resource`, `issuer`, all three
    /// endpoint urls and the gate's challenge nonsense a client cannot use. A
    /// scheme with an authority (`foo://kb.example/...`, `ftp://`, `ws://`)
    /// gets past a host check and would produce exactly that, so the allowlist
    /// is the guard rather than the host.
    #[test]
    fn only_an_http_or_https_address_names_the_origin() {
        for unusable in [
            "foo://knowledge.example/api/v1/auth/oidc/callback",
            "ftp://knowledge.example/api/v1/auth/oidc/callback",
            "ws://knowledge.example/api/v1/auth/oidc/callback",
            "mailto:admin@knowledge.example",
            "data:text/plain,callback",
            "knowledge.example/api/v1/auth/oidc/callback",
        ] {
            let rule = OriginRule::from_config(&config_with(Some(unusable)));
            let origin = rule.origin(&headers_with("127.0.0.1:7411", None)).unwrap();
            assert_eq!(
                origin, "http://127.0.0.1:7411",
                "{unusable} must leave the rule deriving from the request"
            );
            assert!(
                !origin.contains("null"),
                "and an opaque origin must never be published: {unusable} gave {origin}"
            );
        }
        // The loopback development server the settings layer does allow.
        assert_eq!(
            OriginRule::from_config(&config_with(Some(
                "http://localhost:7411/api/v1/auth/oidc/callback"
            )))
            .origin(&HeaderMap::new())
            .unwrap(),
            "http://localhost:7411"
        );
    }

    /// One trailing slash is the same resource and nothing else is. A person
    /// typing the server address into a client adds or omits one without
    /// meaning anything by it; every other difference is a different origin.
    #[test]
    fn a_trailing_slash_names_the_same_resource() {
        assert!(OriginRule::same_resource(
            "https://knowledge.example",
            "https://knowledge.example/"
        ));
        assert!(OriginRule::same_resource(
            "https://knowledge.example/",
            " https://knowledge.example "
        ));
        assert!(
            !OriginRule::same_resource("https://knowledge.example", "https://knowledge.example//"),
            "one slash, not every slash"
        );
        assert!(
            !OriginRule::same_resource("https://knowledge.example", "https://KNOWLEDGE.example"),
            "nothing is lowercased: an audience check that folded spellings would be the hole \
             it exists to close"
        );
        assert!(!OriginRule::same_resource(
            "https://knowledge.example",
            "http://knowledge.example"
        ));
        assert!(!OriginRule::same_resource(
            "https://knowledge.example",
            "https://knowledge.example:8443"
        ));
    }

    /// The three endpoint urls carry the API mount and the metadata pointer
    /// does not. Getting that backwards sends a client either to the MCP
    /// transport or to a path the API guard answers 401 for, and in both cases
    /// the failure is far from here.
    #[test]
    fn the_documents_name_the_origin_and_only_the_endpoints_carry_the_api_mount() {
        let origin = "https://knowledge.example";
        assert_eq!(
            protected_resource_document(origin),
            json!({
                "resource": origin,
                "authorization_servers": [origin],
                "bearer_methods_supported": ["header"],
                "resource_name": "Crystalline",
            })
        );
        assert_eq!(
            authorization_server_document(origin),
            json!({
                "issuer": origin,
                "authorization_endpoint": "https://knowledge.example/api/v1/oauth/authorize",
                "token_endpoint": "https://knowledge.example/api/v1/oauth/token",
                "registration_endpoint": "https://knowledge.example/api/v1/oauth/register",
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "authorization_response_iss_parameter_supported": true,
            })
        );
        assert_eq!(
            resource_metadata_url(origin),
            "https://knowledge.example/.well-known/oauth-protected-resource",
            "the pointer is a root document, with no API mount in it"
        );
    }

    /// With the setting off there is no rule to answer with, and the refusal
    /// names the two keys that change that.
    #[test]
    fn without_the_setting_there_is_no_server_and_the_documents_are_gone() {
        let mut config = config_with(None);
        config.auth.as_mut().unwrap().oauth = None;
        assert!(OauthServer::new(&config).is_none());
        assert!(OauthServer::new(&config_with(None)).is_some());

        let error = enabled(&None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
        assert!(error.detail.contains("auth.oauth"), "{}", error.detail);
        assert!(error.detail.contains("auth.mcp"), "{}", error.detail);
    }
}
