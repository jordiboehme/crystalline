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
use crystalline_service::rest::AuthStore;

mod support;

/// The client id and secret every test configures. The secret is also the
/// needle the leak assertions look for, so it is deliberately unmistakable in
/// any body or log line it might turn up in.
const CLIENT_ID: &str = "crystalline-test-client";
const CLIENT_SECRET: &str = "shhh-this-is-the-oidc-client-secret";

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

async fn idp_discovery(State(state): State<Arc<IdpState>>) -> Json<serde_json::Value> {
    state.discovery_entries.fetch_add(1, Ordering::SeqCst);
    let gate = state.discovery_gate.lock().unwrap().clone();
    if let Some(mut gate) = gate {
        while !*gate.borrow_and_update() {
            gate.changed()
                .await
                .expect("the gate's sender outlives the test");
        }
    }
    let issuer = state.issuer.lock().unwrap().clone();
    Json(serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid", "profile", "email"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "nonce", "name", "email", "preferred_username"],
    }))
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
    // PKCE, checked rather than accepted: a relying party that stopped sending
    // the verifier must fail here rather than sign somebody in.
    if let Some(challenge) = record.code_challenge.as_deref() {
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
    let mut claims = serde_json::json!({
        "iss": issuer,
        "sub": user.subject,
        "aud": CLIENT_ID,
        "exp": now + 300,
        "iat": now,
    });
    let object = claims.as_object_mut().unwrap();
    if let Some(nonce) = nonce {
        object.insert("nonce".to_string(), nonce.into());
    }
    if let Some(value) = user.preferred_username {
        object.insert("preferred_username".to_string(), value.into());
    }
    if let Some(value) = user.name {
        object.insert("name".to_string(), value.into());
    }
    if let Some(value) = user.email {
        object.insert("email".to_string(), value.into());
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
    Json(serde_json::json!({
        "access_token": "access-token",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
    }))
    .into_response()
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
    _auth: Arc<AuthStore>,
}

impl RestCtx {
    /// The production router over an engine whose `auth.oidc` block names
    /// `issuer`, with the shared client id and secret.
    async fn with_oidc(issuer: &str) -> RestCtx {
        RestCtx::build(Some(OidcConfig {
            issuer: Some(issuer.to_string()),
            client_id: Some(CLIENT_ID.to_string()),
            client_secret: Some(CLIENT_SECRET.to_string()),
            name: Some("Contoso".to_string()),
            scopes: None,
            default_role: None,
        }))
        .await
    }

    /// The same instance with no provider configured at all.
    async fn without_oidc() -> RestCtx {
        RestCtx::build(None).await
    }

    async fn build(oidc: Option<OidcConfig>) -> RestCtx {
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
                trusted_header: None,
                anonymous: None,
                mcp: None,
                max_users: None,
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
            _auth: auth,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}/api/v1{path}", self.addr)
    }

    /// A GET carrying `cookies`, which the caller collects by hand because the
    /// browser here has no cookie jar.
    async fn get(&self, url: &str, cookies: &[(String, String)]) -> reqwest::Response {
        let mut request = self.client.get(url);
        if !cookies.is_empty() {
            let header = cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            request = request.header(reqwest::header::COOKIE, header);
        }
        request.send().await.unwrap()
    }

    /// Walk the whole flow: start a sign-in, follow the provider's redirect
    /// back and answer the callback. Returns the callback's response and the
    /// cookies the browser was holding when it made that request.
    async fn sign_in(&self) -> reqwest::Response {
        let start = self.get(&self.url("/auth/oidc/login"), &[]).await;
        assert_eq!(start.status(), 302, "the sign-in should redirect out");
        let cookies = cookies_from(&start);
        let authorize = location(&start);
        let bounced = self.client.get(&authorize).send().await.unwrap();
        assert_eq!(bounced.status(), 302, "the provider should redirect back");
        let callback = location(&bounced);
        self.get(&callback, &cookies).await
    }
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
/// it and validates the token.
///
/// It stops at 501 rather than at a session, and that is the assertion: the
/// protocol is finished and the identity resolver is not. When the
/// provisioning task fills `resolve_oidc_identity`, this test's expected
/// status moves to 302 and everything above it stays where it is.
#[tokio::test]
async fn a_full_code_flow_validates_and_reaches_the_identity_seam() {
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
    assert_eq!(
        done.status(),
        501,
        "the token validated and the flow reached the identity seam"
    );

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

    assert_eq!(ctx.sign_in().await.status(), 501);
    let after_first = idp.jwks_fetches();
    assert_eq!(after_first, 1, "discovery fetched the key set once");

    idp.rotate_keys();
    assert_eq!(
        ctx.sign_in().await.status(),
        501,
        "the second sign-in validates against the rotated key"
    );
    assert_eq!(
        idp.jwks_fetches(),
        after_first + 1,
        "the rotation costs one refetch, not one per token"
    );

    // A third sign-in on the now-cached rotated key costs nothing more.
    assert_eq!(ctx.sign_in().await.status(), 501);
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
    assert_eq!(ctx.get(&callback, &cookies).await.status(), 501);
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
        501,
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

/// The claims the identity layer will consume are read off the token: the
/// subject is the key, and the three presentation claims come through.
#[tokio::test]
async fn the_claims_the_identity_layer_needs_are_carried_through() {
    let (logs, _guard) = support::capture_logs();
    let idp = FakeIdp::start().await;
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;
    idp.set_user(IdpUser {
        subject: "sub-9".to_string(),
        preferred_username: Some("Ada.Lovelace".to_string()),
        name: Some("Ada L".to_string()),
        email: Some("new@example.test".to_string()),
    });
    assert_eq!(ctx.sign_in().await.status(), 501);
    // The seam logs what it was handed, which is the only observation point
    // there is until the resolver behind it exists.
    assert!(
        logs.any_contains("sub-9") && logs.any_contains(&idp.issuer()),
        "the seam was handed the issuer and the subject: {:?}",
        logs.lines()
    );
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
    let refused = ctx.get(&ctx.url("/auth/oidc/login?link=true"), &[]).await;
    assert_eq!(refused.status(), 401);
    let detail = refused.text().await.unwrap();
    assert!(detail.contains("sign in first"), "{detail}");
}

/// Discovery is fetched with no lock held, so a provider that has gone quiet
/// does not turn concurrent sign-ins into a queue.
///
/// `/auth/oidc/login` is public and unauthenticated and the outbound timeout is
/// ten seconds, so serializing the first fetch would let anyone make every
/// waiting sign-in wait for every earlier one. The assertion is direct: with
/// the provider's discovery endpoint blocked, two logins both reach it before
/// either is answered.
#[tokio::test]
async fn a_slow_provider_does_not_serialize_concurrent_sign_ins() {
    let idp = FakeIdp::start().await;
    let gate = idp.block_discovery();
    let ctx = RestCtx::with_oidc(&idp.issuer()).await;

    let login_url = ctx.url("/auth/oidc/login");
    let logins = async { tokio::join!(ctx.get(&login_url, &[]), ctx.get(&login_url, &[])) };
    let watcher = async {
        let both_arrived = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while idp.discovery_entries() < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        gate.send(true).unwrap();
        both_arrived
    };
    let ((first, second), both_arrived) = tokio::join!(logins, watcher);

    assert!(
        both_arrived.is_ok(),
        "the second sign-in never reached discovery: the first one was holding a lock across it"
    );
    assert_eq!(first.status(), 302);
    assert_eq!(second.status(), 302);
}
