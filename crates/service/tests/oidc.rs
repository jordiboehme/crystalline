//! The OpenID Connect relying party, driven end to end against a fake
//! provider this process runs itself.
//!
//! Nothing here reaches the internet. `FakeIdp` is a small axum server on an
//! ephemeral loopback port serving the four endpoints a relying party talks
//! to - the discovery document, the JSON Web Key Set, the authorization
//! endpoint and the token endpoint - signing its ID tokens with the committed
//! RSA fixtures in `tests/fixtures/oidc/`. That is what makes it possible to
//! assert on the things a real provider would never do on demand: rotate its
//! signing key between two sign-ins, sign a token with the wrong issuer, or
//! echo a nonce nobody asked for.
//!
//! The browser is a `reqwest` client with redirects switched off and its two
//! cookies carried by hand, so every hop is one assertion rather than a chain
//! the client walked invisibly.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Form, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, OidcConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};

mod support;

/// The client id and secret every test configures. The secret is also the
/// needle the leak assertions look for, so it is deliberately unmistakable in
/// any body or log line it might turn up in.
const CLIENT_ID: &str = "crystalline-test-client";
const CLIENT_SECRET: &str = "shhh-this-is-the-oidc-client-secret";

/// The password every local account in this file is created with. Local
/// accounts exist here to be the thing a sign-on must NOT land in.
const LOCAL_PASSWORD: &str = "correct horse battery staple";

/// The first key the fake provider signs with, and the one it rotates to.
const KEY_ONE: &str = include_str!("fixtures/oidc/test-idp-key.pem");
const KEY_TWO: &str = include_str!("fixtures/oidc/test-idp-key-2.pem");

// --- the fake provider ------------------------------------------------------

/// One signing key: the PEM `jsonwebtoken` signs with and the `kid` it names.
#[derive(Clone)]
struct TestKey {
    kid: &'static str,
    pem: &'static str,
}

impl TestKey {
    /// This key's public half as one JWKS entry, derived from the same PEM
    /// that signs, so the published set can never disagree with the signature.
    fn jwk(&self) -> serde_json::Value {
        use rsa::pkcs8::DecodePrivateKey as _;
        use rsa::traits::PublicKeyParts as _;

        let private = rsa::RsaPrivateKey::from_pkcs8_pem(self.pem).expect("the fixture is PKCS#8");
        let public = private.to_public_key();
        let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        serde_json::json!({
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": self.kid,
            "n": b64(public.n().to_bytes_be()),
            "e": b64(public.e().to_bytes_be()),
        })
    }
}

/// The person the provider signs in.
#[derive(Clone)]
struct IdpUser {
    subject: String,
    preferred_username: Option<String>,
    name: Option<String>,
    email: Option<String>,
}

impl Default for IdpUser {
    fn default() -> IdpUser {
        IdpUser {
            subject: "sub-ada".to_string(),
            preferred_username: Some("ada.lovelace".to_string()),
            name: Some("Ada Lovelace".to_string()),
            email: Some("ada@example.test".to_string()),
        }
    }
}

/// What one authorization request asked for, kept until its code is redeemed.
#[derive(Clone)]
struct AuthRecord {
    nonce: Option<String>,
    code_challenge: Option<String>,
    redirect_uri: String,
}

/// Everything the fake provider can be told to do differently.
#[derive(Default)]
struct IdpState {
    /// Its own base url, filled in once it is bound.
    issuer: Mutex<String>,
    /// What the ID token claims as `iss`. `None` is the truth.
    token_issuer: Mutex<Option<String>>,
    /// What the discovery document claims as `issuer`. `None` is the truth.
    discovery_issuer: Mutex<Option<String>>,
    /// What the ID token claims as `aud`. `None` is the client id.
    audience_override: Mutex<Option<String>>,
    /// Seconds from now to the ID token's `exp`. `None` is five minutes.
    expiry_offset: Mutex<Option<i64>>,
    /// While set, discovery answers 500: a provider that is up and broken.
    discovery_broken: std::sync::atomic::AtomicBool,
    /// While set, the ID token carries only the required claims and the
    /// presentation claims are served from userinfo alone, the way Authelia
    /// 4.38+ does without a claims policy.
    minimal_id_token: std::sync::atomic::AtomicBool,
    /// What userinfo answers as `sub`. `None` is the signed-in subject.
    userinfo_subject_override: Mutex<Option<String>>,
    /// Where discovery says userinfo lives. `None` is this provider's own.
    userinfo_endpoint_override: Mutex<Option<String>>,
    /// How many times userinfo was asked, so "never asked" is an assertion.
    userinfo_hits: AtomicUsize,
    /// The last ID token issued, so a log-hygiene test knows the one string
    /// that must never appear.
    last_id_token: Mutex<Option<String>>,
    /// The nonce to put in the ID token instead of the one that was asked for.
    nonce_override: Mutex<Option<String>>,
    /// The signing key in use. Index into [`IdpState::keys`].
    active_key: Mutex<usize>,
    /// Codes issued by `/authorize` and not yet redeemed.
    codes: Mutex<HashMap<String, AuthRecord>>,
    /// The person the next sign-in is for.
    user: Mutex<IdpUser>,
    /// How many times the key set has been fetched, which is what makes
    /// "refetched exactly once" an assertion rather than a hope.
    jwks_fetches: AtomicUsize,
    /// The authorization requests seen, whole query strings, so a test can
    /// assert what actually went out.
    authorize_queries: Mutex<Vec<String>>,
    /// How many times the discovery handler has been entered. Counted before
    /// it blocks, so a test can watch two requests arrive at once.
    discovery_entries: AtomicUsize,
    /// While this holds a receiver, the discovery handler waits for it to go
    /// true before answering: a provider that has gone dark, on demand.
    discovery_gate: Mutex<Option<tokio::sync::watch::Receiver<bool>>>,
}

impl IdpState {
    fn keys(&self) -> [TestKey; 2] {
        [
            TestKey {
                kid: "key-one",
                pem: KEY_ONE,
            },
            TestKey {
                kid: "key-two",
                pem: KEY_TWO,
            },
        ]
    }

    fn active(&self) -> TestKey {
        self.keys()[*self.active_key.lock().unwrap()].clone()
    }
}

/// A provider running on loopback for the life of one test.
struct FakeIdp {
    addr: SocketAddr,
    state: Arc<IdpState>,
}

impl FakeIdp {
    /// Bind on an ephemeral port and serve the four endpoints.
    async fn start() -> FakeIdp {
        let state = Arc::new(IdpState::default());
        let router = axum::Router::new()
            .route("/.well-known/openid-configuration", get(idp_discovery))
            .route("/jwks", get(idp_jwks))
            .route("/authorize", get(idp_authorize))
            .route("/token", post(idp_token))
            .route("/userinfo", get(idp_userinfo))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        *state.issuer.lock().unwrap() = format!("http://{addr}");
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        FakeIdp { addr, state }
    }

    fn issuer(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Sign the next ID token with a different `iss` than the one discovery
    /// advertised, which is the shape of a provider that has been swapped
    /// under a configured issuer.
    fn lie_about_the_token_issuer(&self, issuer: &str) {
        *self.state.token_issuer.lock().unwrap() = Some(issuer.to_string());
    }

    /// Advertise a different `issuer` in the discovery document than the url
    /// it is served from. This is exactly what Entra's tenant-independent
    /// `common` endpoint does: it answers with the `{tenantid}` template.
    fn lie_about_the_discovery_issuer(&self, issuer: &str) {
        *self.state.discovery_issuer.lock().unwrap() = Some(issuer.to_string());
    }

    /// Sign the next ID token for a different client.
    fn sign_for_another_audience(&self) {
        *self.state.audience_override.lock().unwrap() = Some("somebody-else".to_string());
    }

    /// Sign the next ID token with an `exp` this many seconds from now.
    fn expire_tokens_after(&self, seconds: i64) {
        *self.state.expiry_offset.lock().unwrap() = Some(seconds);
    }

    /// Issue ID tokens carrying only the required claims from now on, and
    /// serve `preferred_username`, `name` and `email` from userinfo alone.
    fn keep_presentation_claims_out_of_the_id_token(&self) {
        self.state
            .minimal_id_token
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Answer userinfo for somebody else: the token substitution the `sub`
    /// check exists for.
    fn answer_userinfo_for_another_subject(&self) {
        *self.state.userinfo_subject_override.lock().unwrap() =
            Some("sub-somebody-else".to_string());
    }

    /// Advertise a userinfo endpoint nobody is listening on.
    async fn point_userinfo_at_a_dead_port(&self) {
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = dead.local_addr().unwrap();
        drop(dead);
        *self.state.userinfo_endpoint_override.lock().unwrap() =
            Some(format!("http://{address}/userinfo"));
    }

    fn userinfo_hits(&self) -> usize {
        self.state.userinfo_hits.load(Ordering::SeqCst)
    }

    fn last_id_token(&self) -> String {
        self.state
            .last_id_token
            .lock()
            .unwrap()
            .clone()
            .expect("an id token was issued")
    }

    /// Answer discovery with a 500 from now on.
    fn break_discovery(&self) {
        self.state
            .discovery_broken
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Put a nonce in the next ID token that nobody asked for.
    fn send_a_nonce_nobody_asked_for(&self) {
        *self.state.nonce_override.lock().unwrap() = Some("not-the-nonce".to_string());
    }

    /// Roll the signing key, exactly as a provider does on its own schedule
    /// and without telling anybody.
    fn rotate_keys(&self) {
        *self.state.active_key.lock().unwrap() = 1;
    }

    fn jwks_fetches(&self) -> usize {
        self.state.jwks_fetches.load(Ordering::SeqCst)
    }

    fn set_user(&self, user: IdpUser) {
        *self.state.user.lock().unwrap() = user;
    }

    /// Stop answering discovery until [`FakeIdp::release_discovery`] is
    /// called. The sender is kept here so the gate outlives the setup call.
    fn block_discovery(&self) -> tokio::sync::watch::Sender<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        *self.state.discovery_gate.lock().unwrap() = Some(rx);
        tx
    }

    fn discovery_entries(&self) -> usize {
        self.state.discovery_entries.load(Ordering::SeqCst)
    }

    fn last_authorize_query(&self) -> String {
        self.state
            .authorize_queries
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("an authorization request was made")
    }
}

async fn idp_discovery(State(state): State<Arc<IdpState>>) -> Response {
    state.discovery_entries.fetch_add(1, Ordering::SeqCst);
    let gate = state.discovery_gate.lock().unwrap().clone();
    if let Some(mut gate) = gate {
        while !*gate.borrow_and_update() {
            gate.changed()
                .await
                .expect("the gate's sender outlives the test");
        }
    }
    if state
        .discovery_broken
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "discovery is broken today",
        )
            .into_response();
    }
    let issuer = state.issuer.lock().unwrap().clone();
    let advertised = state
        .discovery_issuer
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| issuer.clone());
    let userinfo = state
        .userinfo_endpoint_override
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| format!("{issuer}/userinfo"));
    Json(serde_json::json!({
        "issuer": advertised,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "userinfo_endpoint": userinfo,
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid", "profile", "email"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "nonce", "name", "email", "preferred_username"],
    }))
    .into_response()
}

async fn idp_jwks(State(state): State<Arc<IdpState>>) -> Json<serde_json::Value> {
    state.jwks_fetches.fetch_add(1, Ordering::SeqCst);
    Json(serde_json::json!({ "keys": [state.active().jwk()] }))
}

async fn idp_authorize(
    State(state): State<Arc<IdpState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    state.authorize_queries.lock().unwrap().push(
        query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&"),
    );
    let redirect_uri = query
        .get("redirect_uri")
        .cloned()
        .expect("the relying party sends a redirect_uri");
    let returned_state = query.get("state").cloned().unwrap_or_default();
    let code = format!("code-{}", state.codes.lock().unwrap().len());
    state.codes.lock().unwrap().insert(
        code.clone(),
        AuthRecord {
            nonce: query.get("nonce").cloned(),
            code_challenge: query.get("code_challenge").cloned(),
            redirect_uri: redirect_uri.clone(),
        },
    );
    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    // 302, the way a real authorization endpoint answers.
    (
        axum::http::StatusCode::FOUND,
        [(
            axum::http::header::LOCATION,
            format!("{redirect_uri}{separator}code={code}&state={returned_state}"),
        )],
    )
        .into_response()
}

async fn idp_token(
    State(state): State<Arc<IdpState>>,
    headers: axum::http::HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    // The client authenticates with HTTP Basic by default, so both spellings
    // are accepted and one of them has to carry the right secret: a token
    // endpoint that answered an unauthenticated request would make the
    // "the secret is actually sent" assertions vacuous.
    let basic = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|pair| {
            // Both halves are form-url-encoded per RFC 6749 section 2.3.1.
            let (id, secret) = pair.split_once(':').unwrap_or((pair.as_str(), ""));
            (decode_form(id), decode_form(secret))
        });
    let authenticated = match basic {
        Some((id, secret)) => id == CLIENT_ID && secret == CLIENT_SECRET,
        None => {
            form.get("client_id").map(String::as_str) == Some(CLIENT_ID)
                && form.get("client_secret").map(String::as_str) == Some(CLIENT_SECRET)
        }
    };
    if !authenticated {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid_client" })),
        )
            .into_response();
    }
    let Some(code) = form.get("code") else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_request" })),
        )
            .into_response();
    };
    let Some(record) = state.codes.lock().unwrap().remove(code) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response();
    };
    // PKCE, required rather than accepted: a relying party that stopped sending
    // the challenge, or the verifier, must fail here rather than sign somebody
    // in. A real provider with PKCE enforced behaves the same way.
    let Some(challenge) = record.code_challenge.as_deref() else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_request" })),
        )
            .into_response();
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let digest = <sha2::Sha256 as sha2::Digest>::digest(verifier.as_bytes());
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    if computed != challenge {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    if form.get("redirect_uri").map(String::as_str) != Some(record.redirect_uri.as_str()) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    let now = chrono::Utc::now().timestamp();
    let user = state.user.lock().unwrap().clone();
    let issuer = state
        .token_issuer
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| state.issuer.lock().unwrap().clone());
    let nonce = state
        .nonce_override
        .lock()
        .unwrap()
        .clone()
        .or(record.nonce);
    let audience = state
        .audience_override
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| CLIENT_ID.to_string());
    let expires_in = state.expiry_offset.lock().unwrap().unwrap_or(300);
    let mut claims = serde_json::json!({
        "iss": issuer,
        "sub": user.subject,
        "aud": audience,
        "exp": now + expires_in,
        "iat": now,
    });
    let object = claims.as_object_mut().unwrap();
    if let Some(nonce) = nonce {
        object.insert("nonce".to_string(), nonce.into());
    }
    let minimal = state
        .minimal_id_token
        .load(std::sync::atomic::Ordering::SeqCst);
    if !minimal {
        if let Some(value) = user.preferred_username {
            object.insert("preferred_username".to_string(), value.into());
        }
        if let Some(value) = user.name {
            object.insert("name".to_string(), value.into());
        }
        if let Some(value) = user.email {
            object.insert("email".to_string(), value.into());
        }
    }
    let key = state.active();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(key.kid.to_string());
    let id_token = jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(key.pem.as_bytes()).expect("the fixture parses"),
    )
    .expect("the fixture signs");
    *state.last_id_token.lock().unwrap() = Some(id_token.clone());
    Json(serde_json::json!({
        "access_token": "access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
    }))
    .into_response()
}

/// The userinfo endpoint: the access token in, the presentation claims out.
async fn idp_userinfo(
    State(state): State<Arc<IdpState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    state.userinfo_hits.fetch_add(1, Ordering::SeqCst);
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if bearer != Some("Bearer access-token") {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "invalid_token" })),
        )
            .into_response();
    }
    let user = state.user.lock().unwrap().clone();
    let subject = state
        .userinfo_subject_override
        .lock()
        .unwrap()
        .clone()
        .unwrap_or(user.subject);
    let mut body = serde_json::json!({ "sub": subject });
    let object = body.as_object_mut().unwrap();
    if let Some(value) = user.preferred_username {
        object.insert("preferred_username".to_string(), value.into());
    }
    if let Some(value) = user.name {
        object.insert("name".to_string(), value.into());
    }
    if let Some(value) = user.email {
        object.insert("email".to_string(), value.into());
    }
    Json(body).into_response()
}

/// `application/x-www-form-urlencoded` decoding for the Basic credentials.
fn decode_form(value: &str) -> String {
    percent_encoding::percent_decode_str(&value.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

// --- the instance under test ------------------------------------------------

/// A served Crystalline instance plus the browser that drives it.
struct RestCtx {
    addr: SocketAddr,
    client: reqwest::Client,
    _tmp: tempfile::TempDir,
    /// The accounts database the instance serves from, so a test can ask what
    /// a sign-in actually wrote rather than infer it from a response.
    auth: Arc<AuthStore>,
}

impl RestCtx {
    /// The production router over an engine whose `auth.oidc` block names
    /// `issuer`, with the shared client id and secret.
    async fn with_oidc(issuer: &str) -> RestCtx {
        RestCtx::with_oidc_role(issuer, None).await
    }

    /// The same instance with `auth.oidc.default_role` set to `role`.
    async fn with_oidc_role(issuer: &str, role: Option<&str>) -> RestCtx {
        RestCtx::build(Some(OidcConfig {
            issuer: Some(issuer.to_string()),
            client_id: Some(CLIENT_ID.to_string()),
            client_secret: Some(CLIENT_SECRET.to_string()),
            name: Some("Contoso".to_string()),
            scopes: None,
            default_role: role.map(str::to_string),
            redirect_uri: None,
        }))
        .await
    }

    /// The same instance with `auth.oidc.redirect_uri` set, the override a
    /// deployment behind a proxy that rewrites the Host needs.
    async fn with_redirect_uri(issuer: &str, redirect_uri: &str) -> RestCtx {
        RestCtx::build(Some(OidcConfig {
            issuer: Some(issuer.to_string()),
            client_id: Some(CLIENT_ID.to_string()),
            client_secret: Some(CLIENT_SECRET.to_string()),
            name: Some("Contoso".to_string()),
            scopes: None,
            default_role: None,
            redirect_uri: Some(redirect_uri.to_string()),
        }))
        .await
    }

    /// The same instance with no provider configured at all.
    async fn without_oidc() -> RestCtx {
        RestCtx::build(None).await
    }

    /// A provider plus `auth.trusted_header`: the deployment whose identities
    /// arrive in a header a proxy sets rather than in a session cookie.
    async fn with_oidc_and_trusted_header(issuer: &str, header: &str) -> RestCtx {
        RestCtx::build_with(
            Some(OidcConfig {
                issuer: Some(issuer.to_string()),
                client_id: Some(CLIENT_ID.to_string()),
                client_secret: Some(CLIENT_SECRET.to_string()),
                name: Some("Contoso".to_string()),
                scopes: None,
                default_role: None,
                redirect_uri: None,
            }),
            None,
            Some(header.to_string()),
        )
        .await
    }

    async fn build(oidc: Option<OidcConfig>) -> RestCtx {
        RestCtx::build_capped(oidc, None).await
    }

    /// The same instance with `auth.max_users` set, so the cap a provisioning
    /// has to respect can actually be reached in a test.
    async fn build_capped(oidc: Option<OidcConfig>, max_users: Option<u32>) -> RestCtx {
        RestCtx::build_with(oidc, max_users, None).await
    }

    async fn build_with(
        oidc: Option<OidcConfig>,
        max_users: Option<u32>,
        trusted_header: Option<String>,
    ) -> RestCtx {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("eng");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: Eng\npermalink: MANIFEST\ntags: []\nstatus: stable\n\
             recorded_at: 2026-01-01T00:00:00Z\n---\n\n# Eng\n",
        )
        .unwrap();
        let mut config = GlobalConfig {
            auth: Some(AuthConfig {
                trusted_header,
                proxy_headers: None,
                anonymous: None,
                mcp: None,
                max_users,
                oidc,
            }),
            service: Some(ServiceConfig {
                response_format: Some(ResponseFormat::Json),
                ..ServiceConfig::default()
            }),
            ..GlobalConfig::default()
        };
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        let config_path = tmp.path().join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &config).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            config,
            None,
            Some(config_path),
        ));
        engine.sync(None).await.unwrap();
        let auth = Arc::new(
            AuthStore::open(&tmp.path().join("web-auth.db"))
                .await
                .unwrap(),
        );
        let router = http_router(
            engine,
            Arc::new(AtomicUsize::new(0)),
            &[],
            auth.clone(),
            None,
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        RestCtx {
            addr,
            // Redirects off: every hop of the flow is asserted here rather
            // than walked invisibly by the client.
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            _tmp: tmp,
            auth,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}/api/v1{path}", self.addr)
    }

    /// One account as the store holds it, or `None` when the name is nobody.
    async fn user(&self, name: &str) -> Option<crystalline_service::rest::User> {
        self.auth.user(name).await.unwrap()
    }

    /// An account created the local way: a name, a password and a role that a
    /// sign-on must never inherit.
    async fn create_local_user(&self, name: &str, email: &str, role: Role) {
        self.auth
            .add_user(name, name, Some(email), role, LOCAL_PASSWORD)
            .await
            .unwrap();
    }

    /// Sign a local account in and hand back the cookies it holds, so a test
    /// can drive a route that needs a session.
    async fn local_login(&self, name: &str) -> Vec<(String, String)> {
        let response = self
            .client
            .post(self.url("/auth/login"))
            .json(&serde_json::json!({ "name": name, "password": LOCAL_PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "the local login should succeed");
        cookies_from(&response)
    }

    /// A GET carrying `cookies`, which the caller collects by hand because the
    /// browser here has no cookie jar.
    async fn get(&self, url: &str, cookies: &[(String, String)]) -> reqwest::Response {
        self.get_with(url, cookies, &[]).await
    }

    /// The same, plus request headers of the caller's own - the trusted header
    /// a proxy sets, for the tests that drive an instance whose identities come
    /// from somewhere other than the session cookie.
    async fn get_with(
        &self,
        url: &str,
        cookies: &[(String, String)],
        extra: &[(&str, String)],
    ) -> reqwest::Response {
        let mut request = self.client.get(url);
        if !cookies.is_empty() {
            request = request.header(reqwest::header::COOKIE, cookie_header(cookies));
        }
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        request.send().await.unwrap()
    }

    /// Walk the whole flow: start a sign-in, follow the provider's redirect
    /// back and answer the callback. Returns the callback's response and the
    /// cookies the browser was holding when it made that request.
    async fn sign_in(&self) -> reqwest::Response {
        self.sign_in_from("/auth/oidc/login", &[]).await
    }

    /// The same walk from a chosen start, carrying `held` (a session cookie,
    /// say) on every hop this instance sees.
    async fn sign_in_from(&self, path: &str, held: &[(String, String)]) -> reqwest::Response {
        let (callback, cookies) = self.walk_to_callback(path, held).await;
        self.get(&callback, &cookies).await
    }

    /// The link flow, walked the way the app walks it: POST the start with the
    /// session's CSRF token, take the authorize url out of the answer, bounce
    /// off the provider and answer the callback.
    async fn link_from(&self, held: &[(String, String)]) -> reqwest::Response {
        let (callback, cookies) = self.walk_link_to_callback(held, &[]).await;
        self.get(&callback, &cookies).await
    }

    /// That walk stopped one hop short, so a test can answer the callback with
    /// somebody else's cookies - which is what "started by one session,
    /// finished by another" looks like on the wire - or with a trusted header
    /// and no cookie at all.
    async fn walk_link_to_callback(
        &self,
        held: &[(String, String)],
        extra: &[(&str, String)],
    ) -> (String, Vec<(String, String)>) {
        let start = self.start_link(held, extra).await;
        assert_eq!(start.status(), 200, "the link start should be accepted");
        let mut cookies = held.to_vec();
        cookies.extend(cookies_from(&start));
        let body: serde_json::Value = start.json().await.unwrap();
        let authorize = body["location"]
            .as_str()
            .expect("the start hands back where to send the browser")
            .to_string();
        let bounced = self.client.get(&authorize).send().await.unwrap();
        assert_eq!(bounced.status(), 302, "the provider should redirect back");
        (location(&bounced), cookies)
    }

    /// `POST /auth/oidc/login`, carrying the caller's CSRF token: starting a
    /// link is an unsafe act by a signed-in account, and the guard treats it
    /// as one.
    async fn start_link(
        &self,
        held: &[(String, String)],
        extra: &[(&str, String)],
    ) -> reqwest::Response {
        let csrf = self.csrf_with(held, extra).await;
        let mut request = self.client.post(self.url("/auth/oidc/login"));
        if !held.is_empty() {
            request = request.header(reqwest::header::COOKIE, cookie_header(held));
        }
        for (name, value) in extra {
            request = request.header(*name, value);
        }
        if let Some(token) = csrf {
            request = request.header("x-csrf-token", token);
        }
        request.send().await.unwrap()
    }

    /// The same walk stopped one hop short: the callback url the provider sent
    /// the browser to, and the cookies that browser is holding. Split out for
    /// the tests that answer the callback with somebody else's cookies, which
    /// is what "started by one session, finished by another" looks like on the
    /// wire.
    async fn walk_to_callback(
        &self,
        path: &str,
        held: &[(String, String)],
    ) -> (String, Vec<(String, String)>) {
        let start = self.get(&self.url(path), held).await;
        assert_eq!(start.status(), 302, "the sign-in should redirect out");
        let mut cookies = held.to_vec();
        cookies.extend(cookies_from(&start));
        let authorize = location(&start);
        let bounced = self.client.get(&authorize).send().await.unwrap();
        assert_eq!(bounced.status(), 302, "the provider should redirect back");
        (location(&bounced), cookies)
    }

    /// The CSRF token of the session `cookies` carries, from the probe every
    /// client opens on. Needed by the unsafe requests below, which the guard
    /// refuses without it.
    async fn csrf(&self, cookies: &[(String, String)]) -> String {
        self.csrf_with(cookies, &[])
            .await
            .expect("a session carries a csrf token")
    }

    /// The same probe with headers of the caller's own, and tolerant of there
    /// being no token at all: an identity with no account has none, and the
    /// route it is about to drive is the one that says so.
    async fn csrf_with(
        &self,
        cookies: &[(String, String)],
        extra: &[(&str, String)],
    ) -> Option<String> {
        let body: serde_json::Value = self
            .get_with(&self.url("/auth/me"), cookies, extra)
            .await
            .json()
            .await
            .unwrap();
        body["csrf"].as_str().map(str::to_string)
    }

    /// A DELETE carrying `cookies` and the session's CSRF token.
    async fn delete(&self, url: &str, cookies: &[(String, String)]) -> reqwest::Response {
        let csrf = self.csrf(cookies).await;
        let header = cookie_header(cookies);
        self.client
            .delete(url)
            .header(reqwest::header::COOKIE, header)
            .header("x-csrf-token", csrf)
            .send()
            .await
            .unwrap()
    }

    /// The identity links one session's account holds, as the profile card
    /// reads them.
    async fn my_links(&self, cookies: &[(String, String)]) -> serde_json::Value {
        self.get(&self.url("/me/identity-links"), cookies)
            .await
            .json()
            .await
            .unwrap()
    }
}

/// The `Cookie` header for a browser holding `cookies`.
fn cookie_header(cookies: &[(String, String)]) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Every `set-cookie` on a response, as name and value pairs.
fn cookies_from(response: &reqwest::Response) -> Vec<(String, String)> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|raw| raw.split(';').next())
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect()
}

/// One value as a single path segment: everything that is not alphanumeric is
/// percent-encoded, which is what an issuer url needs before it can ride in a
/// path the way `DELETE /me/identity-links/{issuer}` asks it to.
fn path_segment(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("a redirect carries a location")
        .to_str()
        .unwrap()
        .to_string()
}

// --- the tests --------------------------------------------------------------

/// The whole flow, hop by hop: the authorization request carries PKCE S256, a
/// state and a nonce; the provider sends a code back; the callback exchanges
/// it, validates the token, provisions the account and mints a session.
#[tokio::test]
async fn a_full_code_flow_signs_in_and_mints_a_session() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    let start = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    assert_eq!(start.status(), 302);
    let authorize = location(&start);
    assert!(
        authorize.contains("code_challenge_method=S256"),
        "PKCE S256 must be on the authorization request: {authorize}"
    );
    assert!(authorize.contains("code_challenge="), "{authorize}");
    assert!(authorize.contains("state="), "{authorize}");
    assert!(authorize.contains("nonce="), "{authorize}");
    let cookies = cookies_from(&start);
    assert!(
        cookies.iter().any(|(name, _)| name == "fluid_oidc_state"),
        "the state cookie binds the sign-in to this browser: {cookies:?}"
    );

    let bounced = ctx.client.get(&authorize).send().await.unwrap();
    assert_eq!(bounced.status(), 302);
    let done = ctx.get(&location(&bounced), &cookies).await;
    assert_eq!(done.status(), 302, "the sign-in completes");
    assert_eq!(location(&done), "/", "the browser lands on the application");
    let minted = cookies_from(&done);
    assert!(
        minted.iter().any(|(name, _)| name == "fluid_session"),
        "the callback mints a session: {minted:?}"
    );
    // And it is a session the instance actually honours.
    let me = ctx.get(&ctx.url("/auth/me"), &minted).await;
    assert_eq!(me.status(), 200, "the minted session is live");

    // The scopes the settings layer defaults to actually went out, beside the
    // `openid` the library adds itself.
    let query = idp.last_authorize_query();
    assert!(query.contains("openid"), "{query}");
    assert!(query.contains("profile"), "{query}");
    assert!(query.contains("email"), "{query}");
}

/// A token whose `iss` is not the configured issuer, and a token carrying a
/// nonce nobody asked for, are both refused - and refused with the same
/// sentence, so nothing is learned about which check failed.
#[tokio::test]
async fn an_issuer_mismatch_and_a_bad_nonce_are_refused() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    idp.lie_about_the_token_issuer("https://evil.example");
    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 401);
    let issuer_message = refused.text().await.unwrap();

    *idp.state.token_issuer.lock().unwrap() = None;
    idp.send_a_nonce_nobody_asked_for();
    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 401);
    let nonce_message = refused.text().await.unwrap();

    assert_eq!(
        issuer_message, nonce_message,
        "one message for every way validation can fail"
    );
}

/// A provider that rolls its signing key between two sign-ins is survived by
/// one refetch, and by exactly one: a relying party that refetched on every
/// token would pass the first half of this and fail the count.
#[tokio::test]
async fn jwks_rotation_is_survived_by_exactly_one_refetch() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    assert_eq!(ctx.sign_in().await.status(), 302);
    let after_first = idp.jwks_fetches();
    assert_eq!(after_first, 1, "discovery fetched the key set once");

    idp.rotate_keys();
    assert_eq!(
        ctx.sign_in().await.status(),
        302,
        "the second sign-in validates against the rotated key"
    );
    assert_eq!(
        idp.jwks_fetches(),
        after_first + 1,
        "the rotation costs one refetch, not one per token"
    );

    // A third sign-in on the now-cached rotated key costs nothing more.
    assert_eq!(ctx.sign_in().await.status(), 302);
    assert_eq!(idp.jwks_fetches(), after_first + 1);
}

/// The state is single use and is bound to the browser that started the
/// sign-in: replaying a callback url, and presenting one without the cookie,
/// both fail.
#[tokio::test]
async fn a_callback_is_single_use_and_bound_to_its_browser() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    let start = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    let cookies = cookies_from(&start);
    let bounced = ctx.client.get(location(&start)).send().await.unwrap();
    let callback = location(&bounced);

    // The same url, with no state cookie: another person's browser.
    let stolen = ctx.get(&callback, &[]).await;
    assert_eq!(stolen.status(), 401);

    // With the cookie it works once...
    assert_eq!(ctx.get(&callback, &cookies).await.status(), 302);
    // ...and the record is spent, so a replay finds nothing.
    assert_eq!(ctx.get(&callback, &cookies).await.status(), 401);
}

/// A callback whose state names no sign-in this process started is refused
/// before anything is exchanged.
#[tokio::test]
async fn a_forged_state_is_refused() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    let forged = ctx.url("/auth/oidc/callback?code=whatever&state=forged");
    let refused = ctx
        .get(
            &forged,
            &[("fluid_oidc_state".to_string(), "forged".to_string())],
        )
        .await;
    assert_eq!(refused.status(), 401);
}

/// A provider that refuses the sign-in is reported as a refusal, and its own
/// words are not echoed into the page.
#[tokio::test]
async fn a_provider_refusal_is_reported_without_echoing_its_words() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    let url = ctx.url(
        "/auth/oidc/callback?error=access_denied&error_description=%3Cscript%3Ealert(1)%3C/script%3E",
    );
    let refused = ctx.get(&url, &[]).await;
    assert_eq!(refused.status(), 401);
    let body = refused.text().await.unwrap();
    assert!(!body.contains("script"), "{body}");
    assert!(!body.contains("access_denied"), "{body}");
}

/// The client secret is sent to the token endpoint and appears nowhere else:
/// not in a response body, not in a redirect, and not in a log line.
#[tokio::test]
async fn the_client_secret_never_reaches_a_response_or_a_log() {
    let (logs, _guard) = support::capture_logs();
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    // The happy path, which is the one that actually sends the secret: the
    // fake token endpoint answers `invalid_client` if it does not arrive.
    let start = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    let cookies = cookies_from(&start);
    let authorize = location(&start);
    assert!(!authorize.contains(CLIENT_SECRET), "{authorize}");
    let bounced = ctx.client.get(&authorize).send().await.unwrap();
    let done = ctx.get(&location(&bounced), &cookies).await;
    assert_eq!(
        done.status(),
        302,
        "the exchange succeeded, so the secret was accepted"
    );
    assert!(!done.text().await.unwrap().contains(CLIENT_SECRET));

    // And the failure path, where a provider's raw error is most tempting to
    // pass along.
    let refused = ctx
        .get(&ctx.url("/auth/oidc/callback?code=stale&state=stale"), &[])
        .await;
    assert!(!refused.text().await.unwrap().contains(CLIENT_SECRET));

    assert!(
        !logs.any_contains(CLIENT_SECRET),
        "the client secret reached a log line: {:?}",
        logs.lines()
    );
}

/// The claims the identity layer consumes are read off the token and land in
/// the account: the username is derived from `preferred_username`, and the
/// display name and address come through as they were sent.
#[tokio::test]
async fn the_claims_the_identity_layer_needs_are_carried_through() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.set_user(IdpUser {
        subject: "sub-9".to_string(),
        preferred_username: Some("Ada.Lovelace".to_string()),
        name: Some("Ada L".to_string()),
        email: Some("new@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);
    let user = ctx
        .user("ada.lovelace")
        .await
        .expect("the derived name is the folded preferred_username");
    assert_eq!(user.display, "Ada L");
    assert_eq!(user.email.as_deref(), Some("new@example.test"));
    assert!(!user.disabled);
}

/// An instance with no provider configured says so on all three routes, and
/// says it in a way that names the fix.
#[tokio::test]
async fn an_unconfigured_instance_offers_only_local_accounts() {
    let ctx = RestCtx::without_oidc().await;

    let providers = ctx.get(&ctx.url("/auth/providers"), &[]).await;
    assert_eq!(providers.status(), 200);
    let body: serde_json::Value = providers.json().await.unwrap();
    assert_eq!(body["local"], serde_json::json!(true));
    assert_eq!(body["oidc"]["enabled"], serde_json::json!(false));
    assert_eq!(body["oidc"]["name"], serde_json::Value::Null);

    let login = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    assert_eq!(login.status(), 404);
    let detail = login.text().await.unwrap();
    assert!(detail.contains("auth.oidc.issuer"), "{detail}");

    assert_eq!(
        ctx.get(&ctx.url("/auth/oidc/callback?code=a&state=b"), &[])
            .await
            .status(),
        404
    );
}

/// A configured instance advertises the button's label and nothing else about
/// the provider: no issuer, no client id, no secret.
#[tokio::test]
async fn providers_carries_the_label_and_no_configuration() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    let response = ctx.get(&ctx.url("/auth/providers"), &[]).await;
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("\"enabled\":true"), "{body}");
    assert!(body.contains("Contoso"), "{body}");
    assert!(!body.contains(CLIENT_ID), "{body}");
    assert!(!body.contains(CLIENT_SECRET), "{body}");
    assert!(!body.contains(&idp.issuer()), "{body}");
}

/// The three routes are public: they answer without a session, where every
/// other `/api/v1` path answers 401. That is what makes a sign-in reachable by
/// somebody who is not signed in yet.
#[tokio::test]
async fn the_three_routes_are_reachable_without_a_session() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    // The control: a data route with no identity is closed.
    assert_eq!(ctx.get(&ctx.url("/domains"), &[]).await.status(), 401);
    assert_eq!(
        ctx.get(&ctx.url("/auth/providers"), &[]).await.status(),
        200
    );
    assert_eq!(
        ctx.get(&ctx.url("/auth/oidc/login"), &[]).await.status(),
        302
    );
    // The callback is public too, and refuses on its own terms rather than on
    // the guard's.
    let callback = ctx
        .get(&ctx.url("/auth/oidc/callback?code=a&state=b"), &[])
        .await;
    assert_eq!(callback.status(), 401);
    assert!(
        callback.text().await.unwrap().contains("sign-in"),
        "the refusal is the callback's, not the guard's"
    );
}

/// Link intent needs a signed-in account. Without one the request is refused
/// in words that say what to do, rather than started as an ordinary sign-in.
#[tokio::test]
async fn link_intent_without_a_session_is_refused() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    let refused = ctx.start_link(&[], &[]).await;
    assert_eq!(refused.status(), 401);
    let detail = refused.text().await.unwrap();
    assert!(detail.contains("sign in first"), "{detail}");
}

/// A GET never starts a link, whoever sends it.
///
/// `GET /auth/oidc/login` is public, the session cookie is `SameSite=Lax`, and
/// a Lax cookie rides a top-level cross-site navigation - so another origin
/// could send a signed-in browser to `?link=true` and start a link bound to
/// that account without its owner doing anything. Starting one is an unsafe
/// act by a signed-in account and is a POST, which the CSRF gate covers like
/// every other write; the flag on a GET is refused in words that say so, and
/// an ordinary sign-in through the same GET is untouched.
#[tokio::test]
async fn a_get_never_starts_a_link() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;

    let refused = ctx
        .get(&ctx.url("/auth/oidc/login?link=true"), &session)
        .await;
    assert_eq!(refused.status(), 400);
    let detail = refused.text().await.unwrap();
    assert!(detail.contains("POST"), "{detail}");
    assert!(
        ctx.auth.identity_links("ada").await.unwrap().is_empty(),
        "nothing was started, so nothing can be finished"
    );

    // The ordinary sign-in through the same route is exactly as it was.
    assert_eq!(
        ctx.get(&ctx.url("/auth/oidc/login"), &session)
            .await
            .status(),
        302
    );
}

/// The POST is an unsafe request from a signed-in account, so the guard's one
/// CSRF rule covers it: no token and a wrong token are both refused before the
/// handler runs, which is the whole point of moving the start off the GET.
#[tokio::test]
async fn starting_a_link_needs_the_session_csrf_token() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;

    let bare = ctx
        .client
        .post(ctx.url("/auth/oidc/login"))
        .header(reqwest::header::COOKIE, cookie_header(&session))
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), 403, "no token, no link");
    let wrong = ctx
        .client
        .post(ctx.url("/auth/oidc/login"))
        .header(reqwest::header::COOKIE, cookie_header(&session))
        .header("x-csrf-token", "not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 403, "a guessed token is no token");
}

/// The finishing session is resolved the way the starting one is - through the
/// request's identity - so an instance whose identities arrive in a trusted
/// header rather than in a cookie can link at all.
///
/// The browser here holds no session cookie on any hop: the proxy's header is
/// the whole of who it is, exactly as it is for every other route on such an
/// instance.
#[tokio::test]
async fn a_trusted_header_instance_can_link_without_a_session_cookie() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc_and_trusted_header(&idp.issuer(), "x-forwarded-user").await;
    let header = [("x-forwarded-user", "ada".to_string())];

    let (callback, cookies) = ctx.walk_link_to_callback(&[], &header).await;
    let state_only: Vec<(String, String)> = cookies
        .into_iter()
        .filter(|(name, _)| name == "fluid_oidc_state")
        .collect();
    let linked = ctx.get_with(&callback, &state_only, &header).await;
    assert_eq!(linked.status(), 302, "the header names who finished it");

    let links = ctx.auth.identity_links("ada").await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].subject, "sub-ada");
    assert_eq!(links[0].linked_by, "ada");
    assert!(
        ctx.user("ada.lovelace").await.is_none(),
        "a link provisions nobody"
    );
}

/// Two concurrent first sign-ins share ONE discovery fetch, and neither waits
/// behind a second one.
///
/// `/auth/oidc/login` is public and unauthenticated and the outbound timeout is
/// ten seconds, so both halves matter: a lock held across the fetch would let
/// anyone turn concurrent sign-ins into a queue, and a fetch per call would
/// let anyone turn them into an amplifier aimed at the provider. The fake
/// provider's discovery endpoint is held on a gate, two logins are started at
/// once, and the assertion is that exactly one request reaches the gate while
/// it is held and that both logins are answered promptly once it opens.
#[tokio::test]
async fn concurrent_first_sign_ins_share_one_discovery_and_neither_waits_twice() {
    let idp = FakeIdp::start().await;
    let gate = idp.block_discovery();
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    let login_url = ctx.url("/auth/oidc/login");
    let logins = async { tokio::join!(ctx.get(&login_url, &[]), ctx.get(&login_url, &[])) };
    let watcher = async {
        let first_arrived = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while idp.discovery_entries() < 1 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        // Long enough for a second, uncoalesced request to have reached the
        // gate too; it must not have.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let entries_while_held = idp.discovery_entries();
        let released = std::time::Instant::now();
        gate.send(true).unwrap();
        (first_arrived, entries_while_held, released)
    };
    let ((first, second), (first_arrived, entries_while_held, released)) =
        tokio::join!(logins, watcher);
    let answered_after = released.elapsed();

    assert!(first_arrived.is_ok(), "no sign-in ever reached discovery");
    assert_eq!(
        entries_while_held, 1,
        "two concurrent first sign-ins must share one discovery fetch"
    );
    assert_eq!(first.status(), 302);
    assert_eq!(second.status(), 302);
    assert_eq!(
        idp.discovery_entries(),
        1,
        "the second sign-in must not fetch again after the first one landed"
    );
    assert!(
        answered_after < std::time::Duration::from_secs(2),
        "both sign-ins should be answered right after the one release, not {answered_after:?} later"
    );
}

/// A provider that is up and broken is probed once, not once per sign-in.
///
/// Every failed discovery is a ten-second outbound request on a public route;
/// without a negative cache, N strangers' GETs are N outbound requests at the
/// provider. With one, the second sign-in inside the window is refused from
/// memory.
#[tokio::test]
async fn a_broken_provider_is_not_re_probed_on_every_sign_in() {
    let idp = FakeIdp::start().await;
    idp.break_discovery();
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    let first = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    assert_eq!(first.status(), 502);
    let second = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    assert_eq!(second.status(), 502);
    assert_eq!(
        idp.discovery_entries(),
        1,
        "the second sign-in must be refused from the negative cache"
    );
    // The refusal is the same sentence either way: nothing about the second
    // answer says it came from memory.
    assert_eq!(first.text().await.unwrap(), second.text().await.unwrap());
}

/// A discovery document whose `issuer` is the Entra `{tenantid}` template is
/// what the tenant-independent `common` endpoint answers. The operator who
/// configured it gets the correction written for that mistake, not "could not
/// be reached": the provider was reached, and it answered.
#[tokio::test]
async fn a_discovery_document_naming_the_entra_template_gets_the_correction() {
    let idp = FakeIdp::start().await;
    idp.lie_about_the_discovery_issuer("https://login.microsoftonline.com/{tenantid}/v2.0");
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    let refused = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    assert_eq!(refused.status(), 502);
    let body = refused.text().await.unwrap();
    assert!(body.contains("tenant-specific"), "{body}");
    assert!(!body.contains("could not be reached"), "{body}");
}

/// A token for another client and a token that has already expired are both
/// refused, with the same sentence as every other refused token.
#[tokio::test]
async fn a_wrong_audience_and_an_expired_token_are_refused() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    idp.lie_about_the_token_issuer("https://evil.example");
    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 401);
    let reference = refused.text().await.unwrap();
    *idp.state.token_issuer.lock().unwrap() = None;

    idp.sign_for_another_audience();
    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 401, "a token for another client");
    assert_eq!(refused.text().await.unwrap(), reference);
    *idp.state.audience_override.lock().unwrap() = None;

    idp.expire_tokens_after(-60);
    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 401, "a token that expired a minute ago");
    assert_eq!(refused.text().await.unwrap(), reference);

    // And the control: with both knobs back, the same browser signs in.
    idp.expire_tokens_after(300);
    assert_eq!(ctx.sign_in().await.status(), 302);
}

/// Every answer the callback gives is marked uncacheable, the refusals
/// included: a 401 or a 409 is heuristically cacheable and carries the state
/// cookie's deletion, and nothing on this route should be served from a cache.
/// Both legs drive an error tail, since the success path always carried
/// `no-store` and would pin nothing: the protocol refusal before the seam,
/// and the identity refusal after it (an identity another account holds).
#[tokio::test]
async fn every_callback_answer_is_uncacheable() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    // The identity's own first sign-on provisions `ada.lovelace` and links it,
    // so a second account's attempt to link the same identity is the refusal
    // past the seam.
    assert_eq!(ctx.sign_in().await.status(), 302);
    ctx.create_local_user("grace", "grace@example.test", Role::Editor)
        .await;
    let session = ctx.local_login("grace").await;

    let cache_control = |response: &reqwest::Response| {
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let refused = ctx
        .get(&ctx.url("/auth/oidc/callback?code=a&state=b"), &[])
        .await;
    assert_eq!(refused.status(), 401);
    assert!(cache_control(&refused).contains("no-store"), "{refused:?}");

    let past_the_seam = ctx.link_from(&session).await;
    assert_eq!(past_the_seam.status(), 409);
    assert!(
        cache_control(&past_the_seam).contains("no-store"),
        "{past_the_seam:?}"
    );
}

/// A provider that keeps the presentation claims out of the ID token - Authelia
/// 4.38+ without a claims policy - has them fetched from userinfo, and a
/// provider that puts them in the token is never asked.
#[tokio::test]
async fn presentation_claims_missing_from_the_id_token_are_filled_from_userinfo() {
    let (logs, _guard) = support::capture_logs();
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    assert_eq!(ctx.sign_in().await.status(), 302);
    assert_eq!(
        idp.userinfo_hits(),
        0,
        "a complete id token needs no userinfo call"
    );

    idp.keep_presentation_claims_out_of_the_id_token();
    assert_eq!(ctx.sign_in().await.status(), 302);
    assert_eq!(
        idp.userinfo_hits(),
        1,
        "a minimal id token is completed from userinfo"
    );
    assert!(
        logs.any_contains("filled preferred_username, name, email from userinfo"),
        "the fill is recorded by field name: {:?}",
        logs.lines()
    );
}

/// Userinfo answering for another subject is the token substitution the `sub`
/// check exists for: the sign-in is refused rather than completed with
/// somebody else's name on it.
#[tokio::test]
async fn a_userinfo_answer_for_another_subject_refuses_the_sign_in() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.keep_presentation_claims_out_of_the_id_token();
    idp.answer_userinfo_for_another_subject();

    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 401);
    assert_eq!(idp.userinfo_hits(), 1);
}

/// A userinfo endpoint that cannot be reached costs the sign-in its
/// presentation claims, not the sign-in: the identity is the validated ID
/// token's `(issuer, subject)`, and that is already in hand.
#[tokio::test]
async fn an_unreachable_userinfo_endpoint_does_not_block_the_sign_in() {
    let (logs, _guard) = support::capture_logs();
    let idp = FakeIdp::start().await;
    idp.point_userinfo_at_a_dead_port().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.keep_presentation_claims_out_of_the_id_token();

    assert_eq!(
        ctx.sign_in().await.status(),
        302,
        "the flow reached the seam and signed in"
    );
    assert_eq!(idp.userinfo_hits(), 0);
    // With no presentation claim from either source, provisioning falls back
    // to its generic name rather than refusing: the identity is the subject.
    assert!(
        ctx.user("sso-user").await.is_some(),
        "an account was provisioned under the fallback name"
    );
    assert!(
        logs.lines()
            .iter()
            .any(|line| line.starts_with("WARN") && line.contains("userinfo")),
        "an operator can see that userinfo was unreachable: {:?}",
        logs.lines()
    );
}

/// A refused sign-in leaves a line an operator can read at the daemon's
/// default level, naming the reason and the issuer, and nothing else: no
/// state, no code, no nonce, no token, no secret, no claim value.
#[tokio::test]
async fn a_refused_callback_warns_the_operator_and_leaks_nothing() {
    let (logs, _guard) = support::capture_logs();
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.lie_about_the_token_issuer("https://evil.example");

    // Walked by hand so every secret of the flow is in hand to assert on.
    let start = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    let cookies = cookies_from(&start);
    let state_value = cookies
        .iter()
        .find(|(name, _)| name == "fluid_oidc_state")
        .map(|(_, value)| value.clone())
        .expect("the state cookie");
    let authorize = location(&start);
    let nonce = authorize
        .split('&')
        .find_map(|pair| pair.strip_prefix("nonce="))
        .expect("the nonce")
        .to_string();
    let bounced = ctx.client.get(&authorize).send().await.unwrap();
    let callback = location(&bounced);
    let code = callback
        .split(&['?', '&'][..])
        .find_map(|pair| pair.strip_prefix("code="))
        .expect("the code")
        .to_string();
    let refused = ctx.get(&callback, &cookies).await;
    assert_eq!(refused.status(), 401);

    let lines = logs.lines();
    let warning = lines
        .iter()
        .find(|line| line.starts_with("WARN") && line.contains("refused"))
        .unwrap_or_else(|| panic!("a refusal warns at a level the daemon shows: {lines:?}"));
    assert!(warning.contains("issuer"), "{warning}");
    assert!(
        warning.contains(&idp.issuer()),
        "the warning names the issuer: {warning}"
    );

    let id_token = idp.last_id_token();
    for (what, needle) in [
        ("the state", state_value.as_str()),
        ("the code", code.as_str()),
        ("the nonce", nonce.as_str()),
        ("the id token", id_token.as_str()),
        ("the client secret", CLIENT_SECRET),
        ("a claim value", "ada@example.test"),
    ] {
        assert!(
            !lines.iter().any(|line| line.contains(needle)),
            "{what} reached the log: {lines:?}"
        );
    }
}

// --- provisioning and the identity key --------------------------------------

/// The first sign-in provisions an account; the second lands in the same one
/// even though the provider has renamed the person since. `(issuer, sub)` is
/// the key, and a username is presentation data that follows the rename
/// rather than moving the account.
#[tokio::test]
async fn first_sign_in_provisions_and_second_reuses_despite_renames() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.set_user(IdpUser {
        subject: "sub-1".to_string(),
        preferred_username: Some("Ada.Lovelace".to_string()),
        name: Some("Ada".to_string()),
        email: Some("ada@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);
    let provisioned = ctx.user("ada.lovelace").await.expect("provisioned");
    assert_eq!(provisioned.display, "Ada");
    assert_eq!(provisioned.role, Role::Viewer);

    idp.set_user(IdpUser {
        subject: "sub-1".to_string(),
        preferred_username: Some("Countess".to_string()),
        name: Some("Ada L".to_string()),
        email: Some("new@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);
    assert!(
        ctx.user("countess").await.is_none(),
        "the subject is the key, not the username"
    );
    let same = ctx.user("ada.lovelace").await.expect("the same account");
    assert_eq!(same.display, "Ada L", "the display name follows the rename");
    assert_eq!(
        same.email.as_deref(),
        Some("new@example.test"),
        "the address is presentation data and follows too"
    );
    assert_eq!(
        same.role,
        Role::Viewer,
        "a returning sign-in changes no role"
    );
}

/// An address that matches an existing account links nothing: the sign-on
/// provisions its own fresh account at the default role, and the account it
/// shares an address with is untouched.
#[tokio::test]
async fn matching_email_never_links_silently() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    idp.set_user(IdpUser {
        subject: "sub-9".to_string(),
        preferred_username: Some("ada2".to_string()),
        name: Some("Ada".to_string()),
        email: Some("ada@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);

    let jit = ctx.user("ada2").await.expect("a fresh account");
    assert_eq!(
        jit.role,
        Role::Viewer,
        "a fresh viewer account, never the admin's"
    );
    let local = ctx.user("ada").await.expect("the local account survives");
    assert_eq!(local.role, Role::Admin, "the admin is untouched");
    assert!(
        ctx.auth.identity_links("ada").await.unwrap().is_empty(),
        "the account sharing the address was linked to nothing"
    );
    let links = ctx.auth.identity_links("ada2").await.unwrap();
    assert_eq!(links.len(), 1, "the fresh account holds the link");
    assert_eq!(links[0].subject, "sub-9");
    assert_eq!(links[0].linked_by, "jit");
}

/// A derived username that is already taken is uniquified rather than
/// colliding with, or silently joining, the account holding it.
#[tokio::test]
async fn username_collisions_uniquify() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "x@example.test", Role::Editor)
        .await;
    idp.set_user(IdpUser {
        subject: "sub-2".to_string(),
        preferred_username: Some("Ada".to_string()),
        name: Some("Ada Two".to_string()),
        email: None,
    });
    assert_eq!(ctx.sign_in().await.status(), 302);
    let fresh = ctx.user("ada-2").await.expect("the uniquified name");
    assert_eq!(fresh.display, "Ada Two");
    assert_eq!(
        ctx.user("ada").await.expect("the local account").role,
        Role::Editor,
        "the account that held the name is untouched"
    );
}

/// A provider that sends no `preferred_username` still gets a readable
/// account name: the address's local part, and never the raw address.
#[tokio::test]
async fn a_missing_preferred_username_falls_back_to_the_address() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.set_user(IdpUser {
        subject: "sub-3".to_string(),
        preferred_username: None,
        name: Some("Grace Hopper".to_string()),
        email: Some("Grace.Hopper@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);
    assert!(ctx.user("grace.hopper").await.is_some());
    assert!(
        ctx.user("grace.hopper@example.test").await.is_none(),
        "an address is not a login name"
    );
}

/// `auth.oidc.default_role` is what a provisioned account is created at.
#[tokio::test]
async fn the_configured_default_role_is_what_is_provisioned() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc_role(&idp.issuer(), Some("editor")).await;
    assert_eq!(ctx.sign_in().await.status(), 302);
    assert_eq!(
        ctx.user("ada.lovelace").await.expect("provisioned").role,
        Role::Editor
    );
}

/// A disabled account is refused at the callback, in the words the rest of
/// the surface uses for a disabled account, and its sign-in mints nothing.
#[tokio::test]
async fn a_disabled_account_cannot_sign_on() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    assert_eq!(ctx.sign_in().await.status(), 302);
    ctx.auth.set_disabled("ada.lovelace", true).await.unwrap();

    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 403);
    assert!(
        !cookies_from(&refused)
            .iter()
            .any(|(name, _)| name == "fluid_session"),
        "a refused sign-in mints nothing"
    );
    let body = refused.text().await.unwrap();
    assert!(body.contains("this account is disabled"), "{body}");
}

/// The whole point of the task: a signed-in local account starts a sign-in
/// with `?link=true` and comes back holding the provider identity, with no
/// second account created and nothing about the account changed.
#[tokio::test]
async fn a_link_intent_sign_in_links_the_account_that_started_it() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;

    let linked = ctx.link_from(&session).await;
    assert_eq!(
        linked.status(),
        302,
        "a completed link signs the account in"
    );
    assert_eq!(location(&linked), "/");

    assert!(
        ctx.user("ada.lovelace").await.is_none(),
        "linking provisions no second account"
    );
    let ada = ctx.user("ada").await.expect("the account survives");
    assert_eq!(ada.role, Role::Admin, "linking moves no role");
    let links = ctx.auth.identity_links("ada").await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].issuer, idp.issuer());
    assert_eq!(links[0].subject, "sub-ada");
    assert_eq!(links[0].linked_by, "ada", "the account linked itself");

    // And the link is what the next ordinary sign-on resolves: no `?link`,
    // no session, and it lands in ada rather than provisioning anybody.
    let again = ctx.sign_in().await;
    assert_eq!(again.status(), 302);
    assert!(ctx.user("ada.lovelace").await.is_none());
    assert_eq!(ctx.auth.list_users().await.unwrap().len(), 1);
}

/// A link one session started cannot be finished by another: the callback is
/// answered while a different account holds the session, and the link is
/// refused rather than made for whoever happens to be signed in now.
#[tokio::test]
async fn a_link_started_by_one_session_is_not_finished_by_another() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    ctx.create_local_user("grace", "grace@example.test", Role::Editor)
        .await;
    let ada = ctx.local_login("ada").await;
    let grace = ctx.local_login("grace").await;

    // ada starts the link; the state cookie ada picked up is presented with
    // grace's session cookie, which is what a link finished from somebody
    // else's session looks like on the wire.
    let (callback, cookies) = ctx.walk_link_to_callback(&ada, &[]).await;
    let mut as_grace: Vec<(String, String)> = cookies
        .iter()
        .filter(|(name, _)| name == "fluid_oidc_state")
        .cloned()
        .collect();
    as_grace.extend(grace.clone());
    let refused = ctx.get(&callback, &as_grace).await;
    assert_eq!(refused.status(), 409);
    let body = refused.text().await.unwrap();
    assert!(body.contains("started"), "{body}");
    assert!(ctx.auth.identity_links("ada").await.unwrap().is_empty());
    assert!(ctx.auth.identity_links("grace").await.unwrap().is_empty());
    assert!(
        ctx.user("ada.lovelace").await.is_none(),
        "a refused link provisions nobody"
    );
}

/// The same rule with nobody at all on the callback: a session that was signed
/// out (or expired) while the browser was at the provider is not a session
/// that can link anything.
#[tokio::test]
async fn a_link_whose_session_is_gone_is_refused() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;

    let (callback, cookies) = ctx.walk_link_to_callback(&session, &[]).await;
    // The session is revoked while the browser is away at the provider.
    ctx.auth
        .set_password("ada", "correct horse battery staple")
        .await
        .unwrap();
    let refused = ctx.get(&callback, &cookies).await;
    assert_eq!(refused.status(), 401);
    let body = refused.text().await.unwrap();
    assert!(body.contains("signed in"), "{body}");
    assert!(ctx.auth.identity_links("ada").await.unwrap().is_empty());
}

/// An identity another account already holds is refused, and the refusal does
/// not say whose it is: a person who can start a sign-on must not be able to
/// use it to learn which local account somebody else has.
#[tokio::test]
async fn an_identity_linked_elsewhere_is_refused_without_naming_the_holder() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    // The identity's own first sign-on provisions `ada.lovelace` and links it.
    assert_eq!(ctx.sign_in().await.status(), 302);
    ctx.create_local_user("grace", "grace@example.test", Role::Editor)
        .await;
    let grace = ctx.local_login("grace").await;

    let refused = ctx.link_from(&grace).await;
    assert_eq!(refused.status(), 409);
    let body = refused.text().await.unwrap();
    assert!(body.contains("already linked to another account"), "{body}");
    assert!(
        !body.contains("ada.lovelace"),
        "the refusal names no other account: {body}"
    );
    assert!(ctx.auth.identity_links("grace").await.unwrap().is_empty());
}

/// Linking an identity the account already holds is not an error: the person
/// pressed the button twice, and the honest answer is the sign-in they asked
/// for rather than a conflict with themselves.
#[tokio::test]
async fn linking_an_identity_the_account_already_holds_signs_it_in() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;
    assert_eq!(ctx.link_from(&session).await.status(), 302);

    let session = ctx.local_login("ada").await;
    let again = ctx.link_from(&session).await;
    assert_eq!(again.status(), 302);
    assert_eq!(
        ctx.auth.identity_links("ada").await.unwrap().len(),
        1,
        "the second link is the same link"
    );
}

/// The profile surface: an account reads back the identities it holds and
/// unlinks one, and the listing says whether it has a password to fall back
/// on.
#[tokio::test]
async fn an_account_lists_and_unlinks_its_own_identities() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;
    assert_eq!(ctx.link_from(&session).await.status(), 302);
    let session = ctx.local_login("ada").await;

    let listed = ctx.my_links(&session).await;
    assert_eq!(listed["links"].as_array().unwrap().len(), 1);
    assert_eq!(listed["links"][0]["issuer"], idp.issuer());
    assert_eq!(listed["links"][0]["linked_by"], "ada");
    assert_eq!(listed["has_password"], true);

    let issuer = path_segment(&idp.issuer());
    let removed = ctx
        .delete(&ctx.url(&format!("/me/identity-links/{issuer}")), &session)
        .await;
    assert_eq!(removed.status(), 204);
    assert!(
        ctx.my_links(&session).await["links"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(ctx.auth.identity_links("ada").await.unwrap().is_empty());

    let again = ctx
        .delete(&ctx.url(&format!("/me/identity-links/{issuer}")), &session)
        .await;
    assert_eq!(again.status(), 404, "there is no such link to remove");
}

/// The rule that keeps an account reachable: an account provisioned by a
/// sign-on has no password, so its one identity is its only way in and the
/// unlink is refused in words that name the command which fixes that.
#[tokio::test]
async fn unlinking_the_last_way_in_is_refused_over_http_too() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    let signed_in = ctx.sign_in().await;
    assert_eq!(signed_in.status(), 302);
    let session = cookies_from(&signed_in);

    let listed = ctx.my_links(&session).await;
    assert_eq!(listed["links"][0]["linked_by"], "jit");
    assert_eq!(
        listed["has_password"], false,
        "a provisioned account has no password to fall back on"
    );

    let issuer = path_segment(&idp.issuer());
    let refused = ctx
        .delete(&ctx.url(&format!("/me/identity-links/{issuer}")), &session)
        .await;
    assert_eq!(refused.status(), 409);
    let body = refused.text().await.unwrap();
    assert!(body.contains("crystalline users passwd"), "{body}");
    assert_eq!(
        ctx.auth.identity_links("ada.lovelace").await.unwrap().len(),
        1,
        "a refused unlink removes nothing"
    );

    // A password is the second way in, and with one the unlink goes through.
    ctx.auth
        .set_password("ada.lovelace", "correct horse battery staple")
        .await
        .unwrap();
    let session = ctx.local_login("ada.lovelace").await;
    let removed = ctx
        .delete(&ctx.url(&format!("/me/identity-links/{issuer}")), &session)
        .await;
    assert_eq!(removed.status(), 204);
}

/// Both spellings of the flag on a GET are refused, and for two different
/// reasons: `link=true` is understood and answered with the route that starts
/// a link, `link=1` is not a bool as far as the query layer is concerned and
/// never reaches the handler at all. Pinned because both are 400s a client
/// would otherwise have to tell apart by prose.
#[tokio::test]
async fn neither_spelling_of_the_flag_starts_a_link_on_a_get() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;

    let understood = ctx
        .get(&ctx.url("/auth/oidc/login?link=true"), &session)
        .await;
    assert_eq!(understood.status(), 400);
    let detail = understood.text().await.unwrap();
    assert!(detail.contains("POST"), "{detail}");

    let numeric = ctx.get(&ctx.url("/auth/oidc/login?link=1"), &session).await;
    assert_eq!(
        numeric.status(),
        400,
        "`link=1` is refused rather than read as an ordinary sign-in"
    );
}

/// An issuer that is not one - blank, or nothing but spaces - is the same 404
/// as any other issuer this account holds no link at, rather than a 500. It
/// names no link either way, and a client that mis-encodes a segment should
/// read that as "not found", not as "this server broke".
#[tokio::test]
async fn a_blank_issuer_is_not_found_rather_than_a_server_error() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;
    let session = ctx.local_login("ada").await;

    for segment in ["%20", "%20%20"] {
        let answer = ctx
            .delete(&ctx.url(&format!("/me/identity-links/{segment}")), &session)
            .await;
        assert_eq!(answer.status(), 404, "segment {segment}");
    }
}

/// The identity-link surface is the caller's own: no session, no answer.
#[tokio::test]
async fn the_identity_link_routes_need_a_session() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    assert_eq!(
        ctx.get(&ctx.url("/me/identity-links"), &[]).await.status(),
        401
    );
}

/// The account cap is respected, and the refusal is the operator-facing 403
/// that names the setting rather than a 500.
///
/// The status is what this pins: the mapping reads the store's own sentence,
/// so a reworded cap message would silently turn an operator's 403 into an
/// unexplained server error, and nothing else in the suite would notice.
#[tokio::test]
async fn a_sign_in_past_the_account_cap_is_refused_in_the_operators_words() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::build_capped(
        Some(OidcConfig {
            issuer: Some(idp.issuer()),
            client_id: Some(CLIENT_ID.to_string()),
            client_secret: Some(CLIENT_SECRET.to_string()),
            name: Some("Contoso".to_string()),
            scopes: None,
            default_role: None,
            redirect_uri: None,
        }),
        Some(1),
    )
    .await;
    ctx.create_local_user("ada", "ada@example.test", Role::Admin)
        .await;

    let refused = ctx.sign_in().await;
    assert_eq!(refused.status(), 403);
    let body = refused.text().await.unwrap();
    assert!(body.contains("auth.max_users"), "{body}");
    assert!(
        ctx.user("ada.lovelace").await.is_none(),
        "a refused provisioning leaves no account behind"
    );
}

/// A provider cannot write invisible direction-flipping characters into the
/// account list an operator makes privilege decisions in.
///
/// The display name is the provider's to restate, so it is stored as sent -
/// except for the characters that are not text at all. `Ada` with a
/// right-to-left override in front of a suffix renders as something else
/// entirely in a table, and the same trick in a login name is what turns a
/// user list into an unreliable place to decide who gets admin.
#[tokio::test]
async fn control_characters_never_reach_a_stored_name() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.set_user(IdpUser {
        subject: "sub-4".to_string(),
        preferred_username: Some("ada\u{202e}nimda".to_string()),
        name: Some("Ada\u{202e}ecalevoL\u{7}".to_string()),
        email: Some("ada\u{200f}@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);

    let user = ctx
        .user("adanimda")
        .await
        .expect("the derived name drops the override");
    assert_eq!(user.display, "AdaecalevoL");
    assert_eq!(user.email.as_deref(), Some("ada@example.test"));
    for stored in [user.name.as_str(), user.display.as_str()] {
        assert!(
            !stored.chars().any(|ch| ch.is_control() || ch == '\u{202e}'),
            "{stored:?} still carries a character that is not text"
        );
    }
}

/// The same, on the other boundary claims enter through: a provider that
/// keeps the presentation claims out of the ID token and serves them from
/// userinfo instead cannot write invisible characters into a stored name
/// either.
///
/// Worth its own test rather than a case in the sibling above, because the
/// two boundaries are two pieces of code: the ID token's claims and
/// userinfo's fill each map the provider's strings into `OidcClaims`, and a
/// fix applied to one of them leaves the other exactly as it was.
#[tokio::test]
async fn control_characters_never_reach_a_stored_name_through_userinfo() {
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.keep_presentation_claims_out_of_the_id_token();
    idp.set_user(IdpUser {
        subject: "sub-5".to_string(),
        preferred_username: Some("ada\u{202e}nimda".to_string()),
        name: Some("Ada\u{202e}ecalevoL\u{7}".to_string()),
        email: Some("ada\u{200f}@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 302);
    assert_eq!(
        idp.userinfo_hits(),
        1,
        "the claims this asserts on came from userinfo, not from the token"
    );

    let user = ctx
        .user("adanimda")
        .await
        .expect("the derived name drops the override");
    assert_eq!(user.display, "AdaecalevoL");
    assert_eq!(user.email.as_deref(), Some("ada@example.test"));
    for stored in [user.name.as_str(), user.display.as_str()] {
        assert!(
            !stored.chars().any(|ch| ch.is_control() || ch == '\u{202e}'),
            "{stored:?} still carries a character that is not text"
        );
    }
}

/// The configured callback address goes out on the authorization request
/// exactly as written, instead of the one this instance's own `Host` would
/// derive. That is what a deployment behind a proxy needs: the address
/// registered with the provider is a fact about the public entrance, and the
/// token exchange has to repeat it byte for byte.
#[tokio::test]
async fn a_configured_redirect_uri_goes_out_instead_of_the_derived_one() {
    let idp = FakeIdp::start().await;
    let configured = "https://kb.example.test/api/v1/auth/oidc/callback";
    let ctx = RestCtx::with_redirect_uri(&idp.issuer(), configured).await;

    let start = ctx.get(&ctx.url("/auth/oidc/login"), &[]).await;
    assert_eq!(start.status(), 302);
    let authorize = location(&start);
    let sent = openidconnect::url::Url::parse(&authorize)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned())
        .expect("the relying party sends a redirect_uri");
    assert_eq!(sent, configured);
    assert!(
        !sent.contains(&ctx.addr.to_string()),
        "the request's own address is not what was registered: {sent}"
    );
}
