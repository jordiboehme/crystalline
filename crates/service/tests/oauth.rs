//! The authorization-server half of OAuth for MCP clients, driven against the
//! production router over a live loopback listener.
//!
//! This file is the whole flow's home: registration is here now, and the
//! authorize, consent and token legs join it as they land, which is why the
//! harness below is a context object rather than the four free functions
//! `mcp_auth.rs` grew. A client that registers, consents in a browser and
//! exchanges a code is one story told across three endpoints, and a test that
//! can only reach one of them cannot tell it.
//!
//! Everything here talks HTTP the way a real client does - a `reqwest` client
//! with redirects turned off, so every hop is asserted rather than walked
//! invisibly, and with no cookie jar, so a browser's cookies are carried by
//! hand and a client's requests provably carry none.

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, REGISTRATION_BURST, Role, redirect_matches};
use serde_json::{Value, json};

/// The password every account in this suite is created with. Long enough for
/// the store's own rule and the same for everyone, because which account is
/// signing in is what the tests are about.
const PASSWORD: &str = "pw12345678";

/// The redirect uri Claude's hosted surfaces use, so the fixture looks like the
/// client this exists for.
const HOSTED_REDIRECT: &str = "https://claude.ai/api/mcp/auth_callback";

/// A native client's loopback redirect, registered on one port and presented
/// on whichever one it managed to bind. See RFC 8252 section 7.3.
const LOOPBACK_REDIRECT: &str = "http://127.0.0.1:33418/callback";

/// RFC 7636's own example challenge: 43 characters of base64url, which is what
/// the S256 of any verifier is. The verifier behind it belongs to the token
/// endpoint; nothing here ever needs it, which is the point of PKCE.
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// A running instance and everything a test needs to talk to it.
struct OauthCtx {
    addr: SocketAddr,
    /// Held so the temp directory outlives the server.
    _tmp: tempfile::TempDir,
    /// The very store the routes resolve through, so a test can seed
    /// registrations, grants and accounts and read back what a request did.
    auth: Arc<AuthStore>,
    /// The accounts database on disk, for the two fixtures that have to reach
    /// past the store's API (aging a row, filling the table).
    db: std::path::PathBuf,
    /// Redirects off and no cookie jar. See the module documentation.
    client: reqwest::Client,
}

impl OauthCtx {
    /// An instance with `auth.mcp` and `auth.oauth` both on, which is the only
    /// combination the setting allows.
    async fn start() -> OauthCtx {
        OauthCtx::start_with(true).await
    }

    /// The same instance with `auth.oauth` set to `oauth`, for the one test
    /// that asks what a client finds when the surface is switched off.
    async fn start_with(oauth: bool) -> OauthCtx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = root.join("eng");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n",
        )
        .unwrap();
        let mut cfg = GlobalConfig::default();
        cfg.domains
            .insert("eng".to_string(), DomainEntry::file(dir));
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
        cfg.auth = Some(AuthConfig {
            mcp: Some(true),
            oauth: oauth.then_some(true),
            ..AuthConfig::default()
        });
        let config_path = root.join("config.yaml");
        crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let engine = Arc::new(Engine::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            cfg,
            None,
            Some(config_path),
        ));
        engine.sync(None).await.unwrap();
        let db = root.join("web-auth.db");
        let auth = Arc::new(AuthStore::open(&db).await.unwrap());
        let router = http_router(
            engine,
            Arc::new(AtomicUsize::new(0)),
            &[],
            auth.clone(),
            None,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        OauthCtx {
            addr,
            _tmp: tmp,
            auth,
            db,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        }
    }

    /// An absolute url under the API mount.
    fn url(&self, path: &str) -> String {
        format!("http://{}/api/v1{path}", self.addr)
    }

    /// The origin an ordinary request to this instance arrives at, which is the
    /// resource identifier every token here is minted for.
    fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// An account created the local way, for the legs of the flow a person
    /// walks in a browser.
    async fn create_user(&self, name: &str, role: Role) {
        self.auth
            .add_user(name, name, None, role, PASSWORD)
            .await
            .unwrap();
    }

    /// Sign an account in and hand back the cookies and the CSRF token its
    /// session carries, so a test can drive a route that needs a browser.
    async fn sign_in(&self, name: &str) -> Session {
        let response = self
            .client
            .post(self.url("/auth/login"))
            .json(&json!({ "name": name, "password": PASSWORD }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "the local login should succeed");
        let cookies = cookies_from(&response);
        let body: Value = response.json().await.unwrap();
        Session {
            cookies,
            csrf: body["csrf"].as_str().unwrap().to_string(),
        }
    }

    /// `POST /oauth/register` with `body` as the JSON payload.
    async fn register(&self, body: Value) -> reqwest::Response {
        self.client
            .post(self.url("/oauth/register"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// A registration that is expected to succeed, answered with the parsed
    /// body: the one line most of the later flow starts from.
    async fn register_ok(&self, redirect_uri: &str) -> Value {
        let response = self
            .register(json!({
                "client_name": "a hosted client",
                "redirect_uris": [redirect_uri],
            }))
            .await;
        assert_eq!(
            response.status(),
            201,
            "a well formed registration is created"
        );
        response.json().await.unwrap()
    }

    /// The registrations this instance holds.
    async fn client_count(&self) -> usize {
        self.auth.count_oauth_clients().await.unwrap()
    }

    /// A second connection to the running daemon's accounts database, for the
    /// two fixtures that have to reach past the store's API. The shared-WAL
    /// builder `mcp_auth.rs` uses, for the same reason: the daemon holds the
    /// file open.
    async fn db_conn(&self) -> turso::Connection {
        let name = self.db.to_string_lossy().to_string();
        let db = match turso::Builder::new_local(&name)
            .experimental_multiprocess_wal(true)
            .build()
            .await
        {
            Ok(db) => db,
            Err(_) => turso::Builder::new_local(&name).build().await.unwrap(),
        };
        db.connect().unwrap()
    }

    /// Age a registration past the thirty-day window, both dates at once, so
    /// the prune sees it however it was last touched. There is no API for it
    /// and there should not be: thirty days is not a thing a caller sets.
    async fn age_client(&self, client_id: &str) {
        let old = "2020-01-01T00:00:00Z".to_string();
        self.db_conn()
            .await
            .execute(
                "UPDATE oauth_clients SET created_at = ?1, last_used = ?1 WHERE client_id = ?2",
                vec![
                    turso::Value::Text(old),
                    turso::Value::Text(client_id.to_string()),
                ],
            )
            .await
            .unwrap();
    }

    /// `GET /oauth/authorize` with `params` as the query, arrived at the way a
    /// browser arrives at it: no cookies, and redirects off so the hop is
    /// asserted rather than walked.
    async fn authorize(&self, params: &[(&str, &str)]) -> reqwest::Response {
        let query = params
            .iter()
            .map(|(name, value)| format!("{name}={}", encoded(value)))
            .collect::<Vec<_>>()
            .join("&");
        self.client
            .get(format!("{}?{query}", self.url("/oauth/authorize")))
            .send()
            .await
            .unwrap()
    }

    /// A registration and one good authorization request against it, answered
    /// with the client id and the pending id the consent page is opened on.
    /// The line most of the flow below starts from.
    async fn start_authorization(
        &self,
        redirect_uri: &str,
        state: Option<&str>,
    ) -> (String, String) {
        let registered = self.register_ok(redirect_uri).await;
        let client_id = registered["client_id"].as_str().unwrap().to_string();
        let mut params = vec![
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ];
        if let Some(state) = state {
            params.push(("state", state));
        }
        let started = self.authorize(&params).await;
        assert_eq!(
            started.status(),
            302,
            "a good authorization request lands on the consent page"
        );
        (client_id, request_id(&location(&started)))
    }

    /// `GET /oauth/authorizations/{id}`, as `session`'s browser or as nobody.
    async fn consent(&self, id: &str, session: Option<&Session>) -> reqwest::Response {
        let mut request = self
            .client
            .get(self.url(&format!("/oauth/authorizations/{id}")));
        if let Some(session) = session {
            request = request.header("cookie", session.cookie_header());
        }
        request.send().await.unwrap()
    }

    /// `POST /oauth/authorizations/{id}` with the session's CSRF token, which
    /// is what a browser sends when the person presses a button.
    async fn decide(&self, id: &str, session: &Session, decision: &str) -> reqwest::Response {
        self.client
            .post(self.url(&format!("/oauth/authorizations/{id}")))
            .header("cookie", session.cookie_header())
            .header("x-csrf-token", session.csrf.clone())
            .json(&json!({ "decision": decision }))
            .send()
            .await
            .unwrap()
    }
}

/// A signed-in browser: the cookies it holds and the token its unsafe requests
/// echo.
struct Session {
    cookies: Vec<(String, String)>,
    csrf: String,
}

impl Session {
    /// The `Cookie` header this browser sends.
    fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// The cookies a response sets, name and value only.
fn cookies_from(response: &reqwest::Response) -> Vec<(String, String)> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| {
            let first = value.split(';').next()?;
            let (name, value) = first.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// One refused authorization request: what is wrong with it, the query it
/// sends, and the RFC 6749 error the client should get back.
type AuthorizeCase = (&'static str, Vec<(String, String)>, &'static str);

/// One query value, percent-encoded the way a client library sends it.
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// Where a redirect sends the browser.
fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("a redirect carries a location")
        .to_str()
        .unwrap()
        .to_string()
}

/// The `request` parameter of a consent-page location.
fn request_id(location: &str) -> String {
    let (page, query) = location
        .split_once('?')
        .unwrap_or_else(|| panic!("the consent page is opened with a request id: {location}"));
    assert_eq!(page, "/authorize", "the consent page is a Fluid route");
    let id = query_of(query)
        .remove("request")
        .unwrap_or_else(|| panic!("no request id in {location}"));
    assert!(
        id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit()),
        "a pending authorization is named by 32 random bytes: {id}"
    );
    id
}

/// A query string as its decoded pairs. Percent-decoded, so an assertion reads
/// the value a client would, not the spelling it travelled in.
fn query_of(query: &str) -> std::collections::HashMap<String, String> {
    let query = query.split_once('?').map_or(query, |(_, tail)| tail);
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| {
            let decode = |raw: &str| {
                percent_encoding::percent_decode_str(raw)
                    .decode_utf8()
                    .unwrap()
                    .to_string()
            };
            (decode(name), decode(value))
        })
        .collect()
}

/// Assert an RFC 7591 / RFC 6749 error answer: the status, the `error` code and
/// a description that says something.
async fn assert_oauth_error(response: reqwest::Response, status: u16, error: &str) -> Value {
    assert_eq!(response.status(), status, "the status of the refusal");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"], error, "the error code, in {body}");
    assert!(
        body["error_description"]
            .as_str()
            .is_some_and(|text| text.len() > 10),
        "a refusal says what is wrong: {body}"
    );
    body
}

/// **A client registers as a public client and is handed no secret.**
///
/// The whole of what dynamic registration is for: a hosted client that has
/// never met this instance sends its metadata and gets an identifier back. It
/// is a PUBLIC client - it runs where a secret cannot be kept - so the answer
/// carries `token_endpoint_auth_method: "none"` and no `client_secret`. A
/// registration answer that carried one would be a credential handed to a
/// caller that proved nothing, which is exactly the shape this surface refuses
/// to have.
#[tokio::test]
async fn a_client_registers_as_a_public_client_and_gets_no_secret() {
    let ctx = OauthCtx::start().await;
    let response = ctx
        .register(json!({
            "client_name": "  Cla\u{202e}ude  ",
            "client_uri": "https://claude.ai",
            "redirect_uris": [HOSTED_REDIRECT, "https://claude.com/api/mcp/auth_callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "application_type": "web",
        }))
        .await;
    assert_eq!(response.status(), 201);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "a registration answer is never cached"
    );
    let body: Value = response.json().await.unwrap();

    let client_id = body["client_id"].as_str().unwrap();
    assert!(
        client_id.starts_with("coc_") && client_id.len() == 4 + 32,
        "a client id is the crystalline oauth client prefix plus 32 hex: {client_id}"
    );
    assert!(
        client_id[4..].chars().all(|c| c.is_ascii_hexdigit()),
        "{client_id}"
    );
    assert_eq!(
        body["redirect_uris"],
        json!([HOSTED_REDIRECT, "https://claude.com/api/mcp/auth_callback"]),
        "the uris come back in the order they were sent"
    );
    assert_eq!(
        body["client_name"], "Claude",
        "trimmed, and the direction override taken out: this name is shown to \
         the person deciding whether to trust the client"
    );
    assert_eq!(body["client_uri"], "https://claude.ai");
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert_eq!(
        body["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(body["response_types"], json!(["code"]));
    assert!(
        body["client_id_issued_at"]
            .as_i64()
            .is_some_and(|at| at > 0),
        "the issue instant is seconds since the epoch: {body}"
    );
    assert!(
        body.get("client_secret").is_none()
            && body.get("client_secret_expires_at").is_none()
            && body.get("registration_access_token").is_none()
            && body.get("registration_client_uri").is_none(),
        "a public client is handed no secret and no management credential: {body}"
    );

    // And the store holds exactly what came back, which is what the authorize
    // leg will match a presented redirect uri against.
    let stored = ctx.auth.oauth_client(client_id).await.unwrap().unwrap();
    assert_eq!(stored.client_name, "Claude");
    assert_eq!(stored.client_uri.as_deref(), Some("https://claude.ai"));
    assert_eq!(
        stored.redirect_uris,
        vec![
            HOSTED_REDIRECT.to_string(),
            "https://claude.com/api/mcp/auth_callback".to_string()
        ]
    );
    assert!(stored.last_used.is_none(), "nothing has authorized yet");

    // A client that names no metadata at all is still a client: the name is
    // what a consent screen shows, so it has a default rather than a refusal.
    let bare = ctx
        .register(json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .await;
    assert_eq!(bare.status(), 201);
    let bare: Value = bare.json().await.unwrap();
    assert_eq!(bare["client_name"], "an MCP client");
    assert!(
        bare.get("client_uri").is_none(),
        "a member with nothing in it is absent rather than null: {bare}"
    );
    assert_ne!(
        bare["client_id"], body["client_id"],
        "two registrations never share an id"
    );
}

/// **A redirect uri that is not https, or carries a fragment or userinfo, is
/// refused.**
///
/// This is the single most load-bearing check on the surface: whatever is
/// stored here is where a browser is sent with a code in the query, so a
/// registration is an attacker's one chance to name the address the code goes
/// to. Plain `http` off the loopback interface puts the code on the wire; a
/// fragment or userinfo is how a redirect target is made to read as one thing
/// and resolve as another. None of them are stored, and the answer is RFC
/// 7591's `invalid_redirect_uri` rather than a problem detail, because the
/// client reading it speaks OAuth and nothing else.
#[tokio::test]
async fn a_registration_naming_a_plain_http_or_fragment_redirect_is_refused() {
    let ctx = OauthCtx::start().await;
    let refused = [
        // Plain http on a host that is not this machine: the code would cross
        // the network in clear.
        "http://knowledge.example/cb",
        // The loopback rule is about the host, not about the word.
        "http://localhost.evil.test/cb",
        "https://knowledge.example/cb#fragment",
        "https://knowledge.example/cb#",
        "https://user:pw@knowledge.example/cb",
        "https://user@knowledge.example/cb",
        // Not absolute at all.
        "/api/mcp/auth_callback",
        "knowledge.example/cb",
        // An absolute url of a scheme that redirects nowhere useful, and two
        // that are dangerous to hand a browser.
        "app://callback",
        "javascript:alert(1)",
        "data:text/html,hello",
        // The WHATWG parser strips a tab or a newline from anywhere in the
        // input, so a uri carrying one parses clean and then goes into a
        // `Location` header as written. Refused before it is parsed.
        "https://knowledge.\nexample/cb",
        "https://knowledge.example/cb\r\nSet-Cookie: a=b",
        " https://knowledge.example/cb",
        "https://knowledge.example/cb ",
        "",
        // Spellings a url library would canonicalize between registering and
        // presenting, which would then match nothing.
        "HTTPS://claude.ai/api/mcp/auth_callback",
        "https://knowledge.example",
    ];
    // Every attempt below costs a slot in the burst, refused or not. If this
    // table ever grew past the window the tail would answer 429 and this test
    // would be asserting something else entirely.
    assert!(
        refused.len() + 2 <= REGISTRATION_BURST,
        "the refusal table has grown past the registration burst"
    );
    for uri in refused {
        let response = ctx.register(json!({ "redirect_uris": [uri] })).await;
        assert_oauth_error(response, 400, "invalid_redirect_uri").await;
    }
    // A registration has to have somewhere to redirect: an empty list and an
    // absent member are the same refusal, and neither reaches the store, whose
    // own refusal would surface as a 500.
    for body in [json!({ "redirect_uris": [] }), json!({})] {
        let response = ctx.register(body).await;
        assert_oauth_error(response, 400, "invalid_redirect_uri").await;
    }
    assert_eq!(ctx.client_count().await, 0, "nothing was stored");
}

/// **A loopback redirect is accepted and matches on any port.**
///
/// RFC 8252: a native client binds an ephemeral port at run time, so the port
/// it will present is not knowable when it registers. The rest of the uri is
/// matched exactly - a different host, path or query is a different address,
/// and `localhost` is not `127.0.0.1`.
#[tokio::test]
async fn a_loopback_redirect_matches_with_any_port() {
    let ctx = OauthCtx::start().await;
    for uri in [
        "http://127.0.0.1:33418/callback",
        "http://localhost:8765/oauth/cb",
        "http://[::1]:9000/cb",
        // A loopback registration with no port at all is fine too: the port is
        // the part that moves.
        "http://127.0.0.1/callback",
    ] {
        let body = ctx.register_ok(uri).await;
        let stored = ctx
            .auth
            .oauth_client(body["client_id"].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.redirect_uris, vec![uri.to_string()]);
    }

    let registered = "http://127.0.0.1:33418/callback";
    assert!(redirect_matches(registered, registered));
    assert!(
        redirect_matches(registered, "http://127.0.0.1:51902/callback"),
        "the port a native client happened to bind is not part of the match"
    );
    assert!(
        redirect_matches(registered, "http://127.0.0.1/callback"),
        "and neither is having one at all"
    );
    assert!(
        !redirect_matches(registered, "http://localhost:33418/callback"),
        "localhost and 127.0.0.1 are different registrations"
    );
    assert!(
        !redirect_matches(registered, "http://127.0.0.1:33418/other"),
        "only the port is forgiven"
    );
    assert!(
        !redirect_matches(registered, "http://127.0.0.1:33418/callback?next=/evil"),
        "a query is part of the address"
    );
    assert!(
        !redirect_matches(
            HOSTED_REDIRECT,
            "https://claude.ai:8443/api/mcp/auth_callback"
        ),
        "a hosted client's port is exact: the loopback rule is for loopback"
    );
    assert!(redirect_matches(HOSTED_REDIRECT, HOSTED_REDIRECT));
}

/// **A client that says it can keep a secret is refused, and so is any other
/// metadata this server does not implement.**
///
/// Only public clients exist here: there is no secret to issue, so a client
/// announcing `client_secret_basic` has understood the server wrongly and
/// would fail at the token endpoint instead, one leg later and much harder to
/// read. The same argument covers a grant type this server does not run and a
/// response type it does not answer.
#[tokio::test]
async fn a_confidential_auth_method_is_invalid_client_metadata() {
    let ctx = OauthCtx::start().await;
    let refused = [
        json!({ "redirect_uris": [HOSTED_REDIRECT], "token_endpoint_auth_method": "client_secret_basic" }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "token_endpoint_auth_method": "client_secret_post" }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "token_endpoint_auth_method": "" }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "grant_types": ["client_credentials"] }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "grant_types": ["authorization_code", "implicit"] }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "response_types": ["token"] }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "client_uri": "http://claude.ai" }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "client_uri": "not a url" }),
        json!({ "redirect_uris": [HOSTED_REDIRECT], "client_name": "x".repeat(101) }),
    ];
    // These attempts plus the accepted one below have to fit inside the burst,
    // or the tail of this test would be asserting 429s.
    assert!(
        refused.len() < REGISTRATION_BURST,
        "the refusal table has grown past the registration burst"
    );
    for body in refused {
        let response = ctx.register(body).await;
        assert_oauth_error(response, 400, "invalid_client_metadata").await;
    }
    // `none` spelled out is the same as leaving it off, and `application_type`
    // is accepted and ignored.
    let ok = ctx
        .register(json!({
            "redirect_uris": [HOSTED_REDIRECT],
            "token_endpoint_auth_method": "none",
            "application_type": "native",
        }))
        .await;
    assert_eq!(ok.status(), 201);
    assert_eq!(ctx.client_count().await, 1, "only the good one was stored");
}

/// **Registrations are rate limited per window, and the refusal says when to
/// come back.**
///
/// Registration is the one unauthenticated write on this surface: anybody who
/// can reach the port can make a row. The bound is a burst per process rather
/// than a per-caller quota, because there is no caller identity to key one on
/// before the flow has started, and 429 with `Retry-After` is what a client
/// library already knows how to wait on.
#[tokio::test]
async fn registrations_are_rate_limited_per_window() {
    let ctx = OauthCtx::start().await;
    for i in 0..30 {
        let response = ctx
            .register(json!({ "redirect_uris": [HOSTED_REDIRECT] }))
            .await;
        assert_eq!(
            response.status(),
            201,
            "registration {i} is inside the burst"
        );
    }
    let refused = ctx
        .register(json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .await;
    let retry: u64 = refused
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .expect("a 429 says when to come back")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (1..=600).contains(&retry),
        "the wait is the rest of the window: {retry}"
    );
    assert_oauth_error(refused, 429, "temporarily_unavailable").await;
    assert_eq!(
        ctx.client_count().await,
        30,
        "the refused registration stored nothing"
    );

    // A request the limiter turned away costs the caller nothing else: the
    // body is not even read, so a malformed one gets the same answer.
    let refused = ctx
        .register(json!({ "redirect_uris": ["http://evil.test/cb"] }))
        .await;
    assert_eq!(refused.status(), 429);
}

/// **The stored registrations are capped, and the cap is a refusal rather than
/// unbounded growth.**
///
/// Pruning runs first, so an instance whose table filled with abandoned
/// registrations makes room for itself; what is left is an instance whose
/// registrations are all in use, and the honest answer there is that the
/// server cannot take another one now.
#[tokio::test]
async fn the_registration_table_is_capped_rather_than_grown_forever() {
    let ctx = OauthCtx::start().await;
    // Filled through one transaction on a second connection rather than a
    // thousand round trips: this is a fixture, not the behaviour under test,
    // and a thousand registrations over HTTP would run into the burst limit
    // thirty rows in anyway.
    let conn = ctx.db_conn().await;
    conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
    for i in 0..1000 {
        conn.execute(
            "INSERT INTO oauth_clients (client_id, client_name, client_uri, redirect_uris, \
             created_at, last_used) VALUES (?1, 'a client', NULL, ?2, ?3, ?3)",
            vec![
                turso::Value::Text(format!("coc_{i:032x}")),
                turso::Value::Text(json!([HOSTED_REDIRECT]).to_string()),
                turso::Value::Text(chrono::Utc::now().to_rfc3339()),
            ],
        )
        .await
        .unwrap();
    }
    conn.execute("COMMIT", ()).await.unwrap();
    assert_eq!(ctx.client_count().await, 1000);

    let refused = ctx
        .register(json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .await;
    let body = assert_oauth_error(refused, 503, "temporarily_unavailable").await;
    let says = body["error_description"].as_str().unwrap().to_string();
    assert!(
        says.contains("collected") && !says.contains("revoke"),
        "the refusal names what actually frees a slot - the registrations \
         expiring on their own - rather than a control no operator has: {says}"
    );
    assert_eq!(ctx.client_count().await, 1000, "and nothing was added");
}

/// **A registration nobody used is pruned when the next client registers, and
/// one somebody is connected through never is.**
///
/// The cost of dynamic registration is a row per fresh connection, so the row
/// has to go away on its own. What decides is whether anything came of it: a
/// registration that produced a grant is somebody's live connection however
/// old it is, and one that produced nothing is thirty days of an abandoned
/// attempt.
#[tokio::test]
async fn an_unused_registration_is_pruned_when_the_next_client_registers() {
    let ctx = OauthCtx::start().await;
    ctx.create_user("ada", Role::Editor).await;

    let abandoned = ctx.register_ok(HOSTED_REDIRECT).await["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    let connected = ctx.register_ok(HOSTED_REDIRECT).await["client_id"]
        .as_str()
        .unwrap()
        .to_string();
    ctx.auth
        .issue_oauth_grant("ada", &connected, &ctx.origin())
        .await
        .unwrap();
    // Both are aged past the window, so the survivor survives on its grant
    // rather than on its freshness.
    ctx.age_client(&abandoned).await;
    ctx.age_client(&connected).await;

    let fresh = ctx.register_ok(HOSTED_REDIRECT).await["client_id"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(
        ctx.auth.oauth_client(&abandoned).await.unwrap().is_none(),
        "thirty days with nothing to show for it"
    );
    assert!(
        ctx.auth.oauth_client(&connected).await.unwrap().is_some(),
        "somebody is connected through this one"
    );
    assert!(ctx.auth.oauth_client(&fresh).await.unwrap().is_some());
}

/// **With `auth.oauth` off there is no registration endpoint**, and the answer
/// is this surface's own 404 rather than an OAuth error: a client that reached
/// this path on an instance serving no OAuth was not sent here by the metadata,
/// which is a 404 of its own.
#[tokio::test]
async fn registering_is_gone_while_oauth_is_off() {
    let ctx = OauthCtx::start_with(false).await;
    let response = ctx
        .register(json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .await;
    assert_eq!(response.status(), 404);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json"),
        "the mount's own error shape"
    );
    let body: Value = response.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("auth.oauth"),
        "the refusal names the setting that changes it: {body}"
    );
}

/// **A body that is not the JSON metadata is `invalid_request` in RFC 7591's
/// shape**, not this surface's problem detail.
///
/// A deliberate departure from the mount's convention, and the only one here:
/// the caller is an OAuth client library that branches on `error`, and it
/// reaches this endpoint before it holds any identity at all, so a problem
/// detail would be the one answer in the flow it cannot read. The wrong
/// content type is folded into the same 400 for that reason - RFC 7591 has no
/// 415.
#[tokio::test]
async fn a_body_that_is_not_the_client_metadata_is_an_invalid_request() {
    let ctx = OauthCtx::start().await;
    let not_json = ctx
        .client
        .post(ctx.url("/oauth/register"))
        .header(reqwest::header::CONTENT_TYPE, "text/plain")
        .body("redirect_uris=https://claude.ai/cb")
        .send()
        .await
        .unwrap();
    assert_oauth_error(not_json, 400, "invalid_request").await;

    let malformed = ctx
        .client
        .post(ctx.url("/oauth/register"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{\"redirect_uris\": ")
        .send()
        .await
        .unwrap();
    assert_oauth_error(malformed, 400, "invalid_request").await;

    // A member of the right name and the wrong type is the same failure: it
    // never reaches the metadata checks, so it cannot be told apart from a
    // body that is not JSON at all.
    let wrong_type = ctx
        .register(json!({ "redirect_uris": HOSTED_REDIRECT }))
        .await;
    assert_oauth_error(wrong_type, 400, "invalid_request").await;
    assert_eq!(ctx.client_count().await, 0);
}

/// **Registration is reachable with no session and refuses to be ridden from
/// another origin's browser.**
///
/// It is a pre-session route by necessity - a client registers before anybody
/// has signed in anywhere - so it is public by path. What it is not is exempt
/// from the CSRF rule: a signed-in browser posting a registration still has to
/// echo its session's token, so a page on another origin cannot make one on a
/// visitor's behalf.
#[tokio::test]
async fn registration_needs_no_session_and_is_not_csrf_exempt() {
    let ctx = OauthCtx::start().await;
    ctx.create_user("ada", Role::Editor).await;
    let session = ctx.sign_in("ada").await;

    let ridden = ctx
        .client
        .post(ctx.url("/oauth/register"))
        .header(reqwest::header::COOKIE, session.cookie_header())
        .json(&json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        ridden.status(),
        403,
        "a browser's registration carries the session's CSRF token or none is made"
    );

    let with_token = ctx
        .client
        .post(ctx.url("/oauth/register"))
        .header(reqwest::header::COOKIE, session.cookie_header())
        .header("x-csrf-token", &session.csrf)
        .json(&json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .send()
        .await
        .unwrap();
    assert_eq!(with_token.status(), 201);

    // And the ordinary case: no cookie, no token, no account anywhere.
    let anonymous = ctx
        .register(json!({ "redirect_uris": [HOSTED_REDIRECT] }))
        .await;
    assert_eq!(anonymous.status(), 201);
}

/// **A good authorization request lands on the consent page, and nowhere
/// else.**
///
/// The hinge of the whole flow: the client sends a browser here, this server
/// checks everything it can check without a person, remembers the request under
/// an unguessable id and hands the browser to Fluid. Nothing is granted yet -
/// no code exists, no account has been consulted - which is why the answer is a
/// redirect to a page and not to the client.
///
/// The loopback leg is RFC 8252 section 7.3 at the authorize endpoint: a native
/// client registers one port and binds another, so the presented uri matches
/// port-agnostically and the record keeps the PRESENTED one, because that is
/// where the browser will actually be sent.
#[tokio::test]
async fn a_good_authorize_request_lands_on_the_consent_page() {
    let ctx = OauthCtx::start().await;
    let registered = ctx.register_ok(HOSTED_REDIRECT).await;
    let client_id = registered["client_id"].as_str().unwrap().to_string();
    assert!(
        ctx.auth
            .oauth_client(&client_id)
            .await
            .unwrap()
            .unwrap()
            .last_used
            .is_none(),
        "a registration that has authorized nothing has never been used"
    );

    let started = ctx
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", HOSTED_REDIRECT),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
            ("state", "st-1"),
            ("scope", "openid profile"),
            ("resource", &ctx.origin()),
        ])
        .await;
    assert_eq!(started.status(), 302);
    assert_eq!(
        started
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "the hop that names a pending authorization is never cached"
    );
    let id = request_id(&location(&started));

    // The prune clock does NOT move here. This route is an unauthenticated
    // GET, so a caller sending one per row would keep the whole registration
    // table alive forever; the stamp waits for somebody to allow something.
    assert!(
        ctx.auth
            .oauth_client(&client_id)
            .await
            .unwrap()
            .unwrap()
            .last_used
            .is_none(),
        "an authorization request nobody has answered is not a use"
    );

    // A trailing slash on the resource is the same resource, and no `resource`
    // at all is this one by default: both start a request of their own.
    for resource in [Some(format!("{}/", ctx.origin())), None] {
        let mut params = vec![
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", HOSTED_REDIRECT),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ];
        if let Some(resource) = &resource {
            params.push(("resource", resource.as_str()));
        }
        let response = ctx.authorize(&params).await;
        assert_eq!(
            response.status(),
            302,
            "resource {resource:?} names this instance"
        );
        let other = request_id(&location(&response));
        assert_ne!(
            other, id,
            "every authorization request is remembered on its own"
        );
    }

    // RFC 8252 section 7.3: the port moved between registration and use.
    let moved = "http://127.0.0.1:51902/callback";
    let native = ctx.register_ok(LOOPBACK_REDIRECT).await;
    let native_id = native["client_id"].as_str().unwrap().to_string();
    let started = ctx
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &native_id),
            ("redirect_uri", moved),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ])
        .await;
    assert_eq!(
        started.status(),
        302,
        "a loopback client is matched without its port"
    );
    let native_request = request_id(&location(&started));

    // And the redirect the decision produces goes to the port the client is
    // listening on, not the one it registered months ago.
    ctx.create_user("ada", Role::Viewer).await;
    let ada = ctx.sign_in("ada").await;
    let allowed = ctx.decide(&native_request, &ada, "allow").await;
    assert_eq!(allowed.status(), 200);
    let body: Value = allowed.json().await.unwrap();
    let target = body["location"].as_str().unwrap().to_string();
    assert!(
        target.starts_with(&format!("{moved}?")),
        "the browser goes to the uri the client presented: {target}"
    );
    assert!(
        ctx.auth
            .oauth_client(&native_id)
            .await
            .unwrap()
            .unwrap()
            .last_used
            .is_some(),
        "and an allowed consent is what stamps the registration's last use"
    );
}

/// **A client this server does not know, or a redirect uri it did not
/// register, is answered here rather than redirected.**
///
/// The one rule that cannot be relaxed: an authorization error is only ever
/// sent to a redirect uri that a registration named, because sending it
/// anywhere else is an open redirect with an OAuth error attached, and the
/// browser arriving here is a person's. So these four refusals are rendered as
/// this surface's problem detail, with no `Location` at all.
#[tokio::test]
async fn a_bad_client_or_redirect_uri_is_answered_without_redirecting() {
    let ctx = OauthCtx::start().await;
    let registered = ctx.register_ok(HOSTED_REDIRECT).await;
    let client_id = registered["client_id"].as_str().unwrap().to_string();

    let refused: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "no client id at all",
            vec![
                ("response_type", "code"),
                ("redirect_uri", HOSTED_REDIRECT),
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "S256"),
            ],
        ),
        (
            "a client id nothing registered",
            vec![
                ("response_type", "code"),
                ("client_id", "coc_0000000000000000000000000000dead"),
                ("redirect_uri", HOSTED_REDIRECT),
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "S256"),
            ],
        ),
        (
            "a redirect uri this client never registered",
            vec![
                ("response_type", "code"),
                ("client_id", &client_id),
                ("redirect_uri", "https://evil.example/collect"),
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "S256"),
            ],
        ),
        (
            "a redirect uri with the right host and the wrong path",
            vec![
                ("response_type", "code"),
                ("client_id", &client_id),
                ("redirect_uri", "https://claude.ai/api/mcp/elsewhere"),
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "S256"),
            ],
        ),
        (
            "no redirect uri at all",
            vec![
                ("response_type", "code"),
                ("client_id", &client_id),
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "S256"),
            ],
        ),
    ];

    for (what, params) in refused {
        let response = ctx.authorize(&params).await;
        assert_eq!(response.status(), 400, "{what}");
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/problem+json"),
            "{what} is rendered for the person at the browser"
        );
        assert!(
            response.headers().get(reqwest::header::LOCATION).is_none(),
            "{what} must not redirect anywhere"
        );
    }

    // A loopback uri the registration did not name is refused too: only the
    // port is forgiven, never the host or the path.
    ctx.register_ok(LOOPBACK_REDIRECT).await;
    let response = ctx
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", "http://127.0.0.1:51902/callback"),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ])
        .await;
    assert_eq!(
        response.status(),
        400,
        "another client's loopback uri is not this client's"
    );
}

/// **Everything else is the client's problem, so it goes back to the client -
/// with `state` and with `iss`.**
///
/// Once the redirect uri is one the client registered, this server can talk to
/// the client rather than to the person, and RFC 6749 section 4.1.2.1 says it
/// must. `iss` is the 2026-07-28 era's own requirement (RFC 9207): a client
/// talking to two authorization servers tells their answers apart by it, and
/// without it a mix-up attack is undetectable.
///
/// PKCE is the load-bearing case. This server registers public clients only, so
/// the code is the whole credential; `plain` would make the challenge worth as
/// much as the code it protects, and no challenge at all would make an
/// intercepted code enough on its own.
#[tokio::test]
async fn a_missing_pkce_or_foreign_resource_redirects_with_an_error_and_iss() {
    let ctx = OauthCtx::start().await;
    let registered = ctx.register_ok(HOSTED_REDIRECT).await;
    let client_id = registered["client_id"].as_str().unwrap().to_string();
    let base = |extra: &[(&str, &str)]| {
        let mut params = vec![
            ("response_type".to_string(), "code".to_string()),
            ("client_id".to_string(), client_id.clone()),
            ("redirect_uri".to_string(), HOSTED_REDIRECT.to_string()),
            ("state".to_string(), "st ate&=?".to_string()),
        ];
        for (name, value) in extra {
            params.push((name.to_string(), value.to_string()));
        }
        params
    };

    let cases: Vec<AuthorizeCase> = vec![
        ("no code challenge at all", base(&[]), "invalid_request"),
        (
            "a plain challenge, which protects nothing",
            base(&[
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "plain"),
            ]),
            "invalid_request",
        ),
        (
            "a challenge with no method named",
            base(&[("code_challenge", CHALLENGE)]),
            "invalid_request",
        ),
        (
            "a challenge that is not 43 to 128 unreserved characters",
            base(&[
                ("code_challenge", "short"),
                ("code_challenge_method", "S256"),
            ]),
            "invalid_request",
        ),
        (
            "a resource that is some other server",
            base(&[
                ("code_challenge", CHALLENGE),
                ("code_challenge_method", "S256"),
                ("resource", "https://knowledge.example"),
            ]),
            "invalid_target",
        ),
        (
            "a response type this server does not answer",
            {
                let mut params = base(&[
                    ("code_challenge", CHALLENGE),
                    ("code_challenge_method", "S256"),
                ]);
                params[0].1 = "token".to_string();
                params
            },
            "unsupported_response_type",
        ),
    ];

    for (what, params, expected) in cases {
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (&name[..], &value[..]))
            .collect();
        let response = ctx.authorize(&borrowed).await;
        assert_eq!(response.status(), 302, "{what}");
        let target = location(&response);
        assert!(
            target.starts_with(&format!("{HOSTED_REDIRECT}?")),
            "{what} goes back to the client: {target}"
        );
        let query = query_of(&target);
        assert_eq!(
            query.get("error").map(String::as_str),
            Some(expected),
            "{what}"
        );
        assert_eq!(
            query.get("state").map(String::as_str),
            Some("st ate&=?"),
            "{what} carries the state back verbatim"
        );
        assert_eq!(
            query.get("iss").map(String::as_str),
            Some(ctx.origin().as_str()),
            "{what} says which server answered"
        );
        assert!(!query.contains_key("code"), "{what} grants nothing");
    }

    // A state longer than this server will carry is refused - and the refusal
    // carries no state, because there is no bounded value to carry. The map it
    // would have ridden in is one a stranger can add to.
    let huge = "s".repeat(513);
    let response = ctx
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", HOSTED_REDIRECT),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
            ("state", &huge),
        ])
        .await;
    assert_eq!(response.status(), 302);
    let query = query_of(&location(&response));
    assert_eq!(
        query.get("error").map(String::as_str),
        Some("invalid_request")
    );
    assert!(
        !query.contains_key("state"),
        "an unbounded state is not echoed"
    );

    // A request with no state gets an answer with no state, rather than an
    // empty one: a client that sent none has nothing to compare.
    let response = ctx
        .authorize(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", HOSTED_REDIRECT),
        ])
        .await;
    assert_eq!(response.status(), 302);
    let query = query_of(&location(&response));
    assert_eq!(
        query.get("error").map(String::as_str),
        Some("invalid_request")
    );
    assert!(!query.contains_key("state"), "no state in, no state out");
}

/// **The consent page is a signed-in person's, and the decision on it happens
/// once.**
///
/// The whole point of the page: the account that presses Allow is the account
/// the client will act as, so there has to be one. An anonymous browser is told
/// to sign in the way every other account-bearing route tells it, and Fluid
/// carries it to the login page and back (`return_to`, pinned in `oidc.rs`).
///
/// The record is taken by the decision rather than read, so a second press
/// finds nothing: a person who leaves the tab open and presses Allow again is
/// not a second grant.
#[tokio::test]
async fn consent_needs_a_signed_in_account_and_is_single_use() {
    let ctx = OauthCtx::start().await;
    ctx.create_user("ada", Role::Viewer).await;
    let (client_id, id) = ctx.start_authorization(HOSTED_REDIRECT, Some("st-1")).await;

    let anonymous = ctx.consent(&id, None).await;
    assert_eq!(anonymous.status(), 401, "there is nobody to grant anything");
    assert!(
        ctx.decide(&id, &ctx.sign_in("ada").await, "allow")
            .await
            .status()
            != 401,
        "and a signed-in browser is not refused for the same reason"
    );

    // A second request, this time walked all the way.
    let (_, id) = ctx.start_authorization(HOSTED_REDIRECT, Some("st-2")).await;
    let ada = ctx.sign_in("ada").await;
    let view = ctx.consent(&id, Some(&ada)).await;
    assert_eq!(view.status(), 200);
    assert_eq!(
        view.headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "a page naming an account and a client is not stored anywhere"
    );
    let body: Value = view.json().await.unwrap();
    assert_eq!(body["client_name"], "a hosted client");
    assert_eq!(body["redirect_host"], "claude.ai");
    assert_eq!(body["loopback"], false);
    assert_eq!(body["account"], "ada");
    let left = body["expires_in"].as_u64().unwrap();
    assert!(
        left > 0 && left <= 600,
        "ten minutes at the outside: {left}"
    );
    let rendered = body.to_string();
    for secret in [CHALLENGE, "st-2"] {
        assert!(
            !rendered.contains(secret),
            "the consent view shows what to decide about, never the protocol: {rendered}"
        );
    }
    assert!(
        !rendered.contains(&client_id),
        "nor an identifier only the client has any use for: {rendered}"
    );

    assert_eq!(
        ctx.consent("not-a-request-anybody-made", Some(&ada))
            .await
            .status(),
        404,
        "an id naming no pending request is gone, not refused"
    );

    // No CSRF token: refused before the record is touched, so the decision is
    // still there to be made afterwards.
    let no_token = ctx
        .client
        .post(ctx.url(&format!("/oauth/authorizations/{id}")))
        .header("cookie", ada.cookie_header())
        .json(&json!({ "decision": "allow" }))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 403, "a decision is an unsafe request");

    let allowed = ctx.decide(&id, &ada, "allow").await;
    assert_eq!(allowed.status(), 200);
    assert_eq!(
        ctx.decide(&id, &ada, "allow").await.status(),
        404,
        "the record was taken by the decision, not read"
    );
    assert_eq!(
        ctx.consent(&id, Some(&ada)).await.status(),
        404,
        "and the page it was on is gone with it"
    );

    // A body that is not a decision decides nothing.
    let (_, id) = ctx.start_authorization(HOSTED_REDIRECT, None).await;
    let nonsense = ctx.decide(&id, &ada, "maybe").await;
    assert_eq!(
        nonsense.status(),
        422,
        "there are two answers and no others - refused by the body extractor,          the way this mount refuses every other unreadable member"
    );
    assert_eq!(
        ctx.consent(&id, Some(&ada)).await.status(),
        200,
        "and the request survives an unreadable answer to it"
    );
}

/// **Allow hands the client a code; Deny hands it `access_denied`.**
///
/// Both answers are a location the browser navigates to, because the client is
/// waiting at its redirect uri either way and a person who says no is owed the
/// same round trip as one who says yes. `state` comes back verbatim in both -
/// it is the client's own value and the thing it matches its pending request
/// on - and `iss` says which server answered.
#[tokio::test]
async fn allow_returns_a_code_and_deny_returns_access_denied() {
    let ctx = OauthCtx::start().await;
    ctx.create_user("ada", Role::Viewer).await;
    ctx.create_user("bob", Role::Editor).await;
    let ada = ctx.sign_in("ada").await;
    let bob = ctx.sign_in("bob").await;
    let state = "a state&with=punctuation";

    let (_, id) = ctx.start_authorization(HOSTED_REDIRECT, Some(state)).await;
    let allowed = ctx.decide(&id, &ada, "allow").await;
    assert_eq!(allowed.status(), 200);
    assert_eq!(
        allowed
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "an answer carrying an authorization code is never cached"
    );
    let body: Value = allowed.json().await.unwrap();
    let target = body["location"].as_str().unwrap().to_string();
    assert!(
        target.starts_with(&format!("{HOSTED_REDIRECT}?")),
        "{target}"
    );
    let query = query_of(&target);
    let code = query.get("code").expect("allow issues a code");
    assert!(
        code.len() == 64 && code.chars().all(|c| c.is_ascii_hexdigit()),
        "a code is 32 random bytes: {code}"
    );
    assert_eq!(query.get("state").map(String::as_str), Some(state));
    assert_eq!(
        query.get("iss").map(String::as_str),
        Some(ctx.origin().as_str())
    );
    assert!(
        !query.contains_key("error"),
        "an allowed request is not an error"
    );

    // Deny: the same round trip, and nothing granted.
    let (denied_client, id) = ctx.start_authorization(HOSTED_REDIRECT, Some(state)).await;
    let denied = ctx.decide(&id, &bob, "deny").await;
    assert_eq!(denied.status(), 200);
    let body: Value = denied.json().await.unwrap();
    let query = query_of(body["location"].as_str().unwrap());
    assert_eq!(
        query.get("error").map(String::as_str),
        Some("access_denied")
    );
    assert_eq!(query.get("state").map(String::as_str), Some(state));
    assert_eq!(
        query.get("iss").map(String::as_str),
        Some(ctx.origin().as_str())
    );
    assert!(!query.contains_key("code"), "a refusal grants nothing");

    // And a refusal is not a use: the registration's prune clock is untouched
    // by a deny and by an authorization nobody ever answers, so a client that
    // was never actually let in is still collected after thirty days.
    let (abandoned_client, _) = ctx.start_authorization(HOSTED_REDIRECT, None).await;
    for client_id in [&denied_client, &abandoned_client] {
        assert!(
            ctx.auth
                .oauth_client(client_id)
                .await
                .unwrap()
                .unwrap()
                .last_used
                .is_none(),
            "a registration nobody allowed has never been used"
        );
    }

    // The decision is any signed-in account's, and the code binds to whoever
    // made it: there is no account at the moment a client starts a request, so
    // there is nothing for a request to belong to before somebody decides.
    let (_, id) = ctx.start_authorization(HOSTED_REDIRECT, None).await;
    let view: Value = ctx.consent(&id, Some(&bob)).await.json().await.unwrap();
    assert_eq!(
        view["account"], "bob",
        "the page names who is about to grant"
    );
    assert_eq!(ctx.decide(&id, &bob, "allow").await.status(), 200);

    // A redirect uri that already carries a query keeps it, and the answer is
    // appended rather than substituted for it.
    let with_query = "https://claude.ai/api/mcp/auth_callback?tenant=eu";
    let (_, id) = ctx.start_authorization(with_query, Some("st")).await;
    let allowed: Value = ctx.decide(&id, &ada, "allow").await.json().await.unwrap();
    let target = allowed["location"].as_str().unwrap().to_string();
    assert!(target.starts_with(&format!("{with_query}&")), "{target}");
    let query = query_of(&target);
    assert_eq!(query.get("tenant").map(String::as_str), Some("eu"));
    assert!(query.contains_key("code"));
}

/// **With `auth.oauth` off there is no front door either.**
///
/// The same `404` the two well-known documents and the registration endpoint
/// answer, so a client probing an instance that serves no OAuth gets one
/// consistent answer wherever it knocks.
#[tokio::test]
async fn authorizing_is_gone_while_oauth_is_off() {
    let ctx = OauthCtx::start_with(false).await;
    ctx.create_user("ada", Role::Viewer).await;
    let ada = ctx.sign_in("ada").await;

    let response = ctx
        .authorize(&[
            ("response_type", "code"),
            ("client_id", "coc_0000000000000000000000000000dead"),
            ("redirect_uri", HOSTED_REDIRECT),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ])
        .await;
    assert_eq!(response.status(), 404);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json"),
    );
    let body: Value = response.json().await.unwrap();
    assert!(
        body["detail"].as_str().unwrap().contains("auth.oauth"),
        "the operator reading it is the one who can change the answer: {body}"
    );

    assert_eq!(ctx.consent("whatever", Some(&ada)).await.status(), 404);
    assert_eq!(ctx.decide("whatever", &ada, "allow").await.status(), 404);
}
