//! `crystalline connect <url>` and `crystalline disconnect <name|url>`:
//! adding a Crystalline server as a source, signing this machine in to it,
//! and removing it again. A machine may hold several; each one is a record
//! in `sources.json` with its own credential and host folder.
//!
//! The browser flow is OAuth 2.1 for a native app against the server's own
//! authorization server: dynamic client registration as a public client (RFC
//! 7591), the authorization code grant with an `S256` PKCE challenge, and a
//! loopback redirect on an ephemeral port (RFC 8252 section 7.3). The
//! registration names `http://127.0.0.1/callback`; the authorization presents
//! the port this process bound, which the server accepts because it compares
//! loopback redirects without their port (`redirect_matches`). The `state`
//! parameter ties the answer to this sign-in and `iss` is checked against the
//! metadata's issuer, which is what defeats a mix-up between two servers.
//!
//! The `resource` a token is asked for is the one the server's
//! protected-resource document names, never the address the person typed: a
//! server behind a proxy, or one with `service.public_url` set, names itself
//! differently from how it was reached, and a token minted for the wrong
//! resource opens nothing.
//!
//! Token paste is the other door, for a machine with no browser or a server
//! with `auth.oauth` off: the token is checked by asking the server who it
//! is, and saved as it is. A server without browser sign-in is not a dead
//! end for the browser flow either: the person is told so and asked for a
//! token on the spot.
//!
//! # A server that cannot be reached
//!
//! Every network step has an overall limit that covers name resolution,
//! connect, TLS and the answer, and no single request waits longer than
//! [`ONE_DOMAIN_LIMIT`]: [`SIGN_IN_LIMIT`] for the steps before the browser
//! opens, again for the steps after it came back, and for a token paste;
//! [`DISCONNECT_LIMIT`] for the revocation of a disconnect. The wait for the
//! person in the browser ([`SIGN_IN_WAIT`]) and at the token prompt is not a
//! network wait and has its own rule. A server that does not answer is
//! [`SignInError::Unreachable`] with the likely cause in plain words, and
//! nothing on this machine changes: the source record and the credential are
//! written only once everything has answered, and a save that fails half way
//! puts back what was there. A disconnect never fails on the network: it
//! forgets the source here and says the grant could not be ended there.
//!
//! No text a server sent reaches a message: an OAuth error is named by its
//! code only, and only when the code is the plain shape RFC 6749 gives it.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine as _;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::mounts::{Announcement, LocalDomain, assign};
use crate::server_client::{
    CONNECT_TIMEOUT, ONE_DOMAIN_LIMIT, UNREACHABLE_WORDS, cause, form_body, http_client, seconds,
};
use crate::server_token::{CredentialKind, ServerCredential, ServerCredentialStore};
use crate::source_cache::{Cached, ROUTING_FILE, cached_offers, remote_domains, write_cached};
use crate::sources::{
    SourceRecord, default_source_name, load_sources, update_sources, valid_source_name,
};

/// How long the browser has to come back.
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(5 * 60);
/// The overall limit of each network step of a sign-in: the checks and the
/// registration before the browser opens, the code exchange and the account
/// check after it came back, and a token paste's checks.
pub const SIGN_IN_LIMIT: Duration = Duration::from_secs(20);
/// The overall limit of a disconnect's revocation.
pub const DISCONNECT_LIMIT: Duration = ONE_DOMAIN_LIMIT;
/// The path the loopback redirect lands on.
pub const CALLBACK_PATH: &str = "/callback";
/// The redirect the registration names, port-agnostic.
pub const REGISTERED_REDIRECT: &str = "http://127.0.0.1/callback";
/// What a personal MCP token starts with. The identity crate's
/// `MCP_TOKEN_PREFIX` is the original; this crate cannot depend on it, and a
/// service test pins the two equal.
pub const MCP_TOKEN_PREFIX: &str = "cmt_";
/// Where a person finds a personal MCP token.
const WHERE_TOKENS_ARE: &str = "issue one in Fluid under profile > Agent access";

/// Why a sign-in did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    /// Not a usable URL.
    BadUrl(String),
    /// Plain http to a host that is not this machine.
    InsecureUrl {
        /// The host.
        host: String,
    },
    /// No answer: name resolution, connect, TLS, a broken connection, a
    /// gateway that says the server behind it is down, or no answer within
    /// the limit.
    Unreachable {
        /// The server.
        url: String,
        /// The likely cause, in plain words.
        detail: String,
    },
    /// Something answered, but not Crystalline's `/health`.
    NotCrystalline {
        /// The address.
        url: String,
    },
    /// The person pressed Deny, or the server refused the authorization.
    Denied(String),
    /// The server answered something the flow cannot use.
    Protocol(String),
    /// The browser never came back.
    TimedOut,
    /// A pasted token the server does not accept, or none at all.
    BadToken(String),
    /// The credential or the source could not be saved or removed.
    Store(String),
    /// A `--name` that cannot name a source, or that another source has.
    BadName(String),
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignInError::BadUrl(text)
            | SignInError::Protocol(text)
            | SignInError::BadToken(text)
            | SignInError::BadName(text) => f.write_str(text),
            SignInError::InsecureUrl { host } => write!(
                f,
                "refusing plain http to {host}: only a server on this machine may be reached \
                 without https; use https://{host}"
            ),
            SignInError::Unreachable { url, detail } => write!(
                f,
                "{url} {UNREACHABLE_WORDS}: {detail}. Check the VPN or the network and run \
                 crystalline connect {url} again; nothing was changed on this machine"
            ),
            SignInError::NotCrystalline { url } => {
                write!(
                    f,
                    "{url} does not answer like a Crystalline server (GET /health)"
                )
            }
            SignInError::Denied(text) => {
                write!(f, "the sign-in was not allowed ({text}); nothing was saved")
            }
            SignInError::TimedOut => f.write_str(
                "the sign-in was not finished in the browser in time; nothing was saved. Run crystalline connect again",
            ),
            SignInError::Store(text) => write!(f, "could not save the sign-in: {text}"),
        }
    }
}

impl std::error::Error for SignInError {}

/// The origin of `input`: scheme, host and port, no path and no trailing
/// slash. `https` anywhere, plain `http` only to this machine.
pub fn normalize_server_url(input: &str) -> Result<String, SignInError> {
    let url = reqwest::Url::parse(input.trim())
        .map_err(|e| SignInError::BadUrl(format!("'{input}' is not a URL: {e}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| SignInError::BadUrl(format!("'{input}' names no host")))?
        .to_string();
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        "http" => return Err(SignInError::InsecureUrl { host }),
        _ => {
            return Err(SignInError::BadUrl(format!(
                "'{input}' is not an http or https URL"
            )));
        }
    }
    Ok(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The overall limit of one network step: when it ends, and how long it was.
#[derive(Clone, Copy, Debug)]
struct Budget {
    until: Instant,
}

impl Budget {
    fn new(limit: Duration) -> Budget {
        Budget {
            until: Instant::now() + limit,
        }
    }

    /// The overall limit of the next request: what is left of the step, at
    /// most [`ONE_DOMAIN_LIMIT`].
    fn next(&self, url: &str) -> Result<Duration, SignInError> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(SignInError::Unreachable {
                url: url.to_string(),
                detail: "it did not answer in time".to_string(),
            });
        }
        Ok(left.min(ONE_DOMAIN_LIMIT))
    }
}

/// A transport failure of a request that had `limit` overall, in plain
/// words.
fn transport(url: &str, e: &reqwest::Error, limit: Duration) -> SignInError {
    let detail = if e.is_timeout() {
        let waited = if e.is_connect() {
            CONNECT_TIMEOUT.min(limit)
        } else {
            limit
        };
        format!("it did not answer within {}", seconds(waited))
    } else {
        cause(e, url)
    };
    SignInError::Unreachable {
        url: url.to_string(),
        detail,
    }
}

/// One request within `budget`: a transport failure or a gateway's `502`,
/// `503` or `504` is [`SignInError::Unreachable`] for `url`.
async fn send(
    request: reqwest::RequestBuilder,
    url: &str,
    budget: &Budget,
) -> Result<Answer, SignInError> {
    let limit = budget.next(url)?;
    let response = request
        .timeout(limit)
        .send()
        .await
        .map_err(|e| transport(url, &e, limit))?;
    let status = response.status();
    if matches!(status.as_u16(), 502..=504) {
        return Err(SignInError::Unreachable {
            url: url.to_string(),
            detail: format!(
                "its gateway answered {}, so the server behind it is down",
                status.as_u16()
            ),
        });
    }
    // The request's own limit runs until the body has finished.
    let text = response
        .text()
        .await
        .map_err(|e| transport(url, &e, limit))?;
    Ok(Answer {
        status: status.as_u16(),
        body: serde_json::from_str(&text).ok(),
    })
}

/// A finished answer: its status, and its body when that is JSON.
struct Answer {
    status: u16,
    body: Option<Value>,
}

impl Answer {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn json(&self) -> &Value {
        self.body.as_ref().unwrap_or(&Value::Null)
    }

    /// The OAuth `error` code of a refusal, only when it is the plain shape
    /// RFC 6749 gives it: never any other text the server sent.
    fn oauth_error(&self) -> String {
        oauth_code(self.json()["error"].as_str().unwrap_or(""))
    }
}

/// `code` when it is a plain OAuth error code, a neutral word otherwise.
fn oauth_code(code: &str) -> String {
    let plain = !code.is_empty()
        && code.len() <= 64
        && code.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if plain {
        code.to_string()
    } else {
        "an error".to_string()
    }
}

/// The OAuth endpoints a server publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthEndpoints {
    /// The resource identifier tokens are minted for.
    pub resource: String,
    /// The authorization server's issuer, checked against `iss`.
    pub issuer: String,
    /// Where the browser goes.
    pub authorization_endpoint: String,
    /// Where a code and a refresh token are exchanged.
    pub token_endpoint: String,
    /// Where a client registers.
    pub registration_endpoint: String,
    /// Where a grant is revoked, when the server offers it (0.23 and later).
    pub revocation_endpoint: Option<String>,
}

/// What a server said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The origin.
    pub origin: String,
    /// Its OAuth endpoints, `None` where `auth.oauth` is off.
    pub oauth: Option<OauthEndpoints>,
}

/// Check that a Crystalline server answers at `origin`, and read its OAuth
/// metadata when it has any, within [`SIGN_IN_LIMIT`].
pub async fn discover(http: &reqwest::Client, origin: &str) -> Result<Discovered, SignInError> {
    discover_by(http, origin, &Budget::new(SIGN_IN_LIMIT)).await
}

async fn discover_by(
    http: &reqwest::Client,
    origin: &str,
    budget: &Budget,
) -> Result<Discovered, SignInError> {
    let health = send(http.get(format!("{origin}/health")), origin, budget).await?;
    if !health.ok() || health.json()["status"] != "ok" {
        return Err(SignInError::NotCrystalline {
            url: origin.to_string(),
        });
    }
    let none = || Discovered {
        origin: origin.to_string(),
        oauth: None,
    };
    let resource = send(
        http.get(format!("{origin}/.well-known/oauth-protected-resource")),
        origin,
        budget,
    )
    .await?;
    if !resource.ok() {
        return Ok(none());
    }
    let document = resource.json();
    let (Some(resource_id), Some(issuer)) = (
        document["resource"].as_str(),
        document["authorization_servers"][0].as_str(),
    ) else {
        return Err(SignInError::Protocol(format!(
            "the protected-resource document of {origin} names no resource or authorization server"
        )));
    };
    let issuer = issuer.trim_end_matches('/').to_string();
    let meta = send(
        http.get(format!("{issuer}/.well-known/oauth-authorization-server")),
        origin,
        budget,
    )
    .await?;
    // An authorization server without metadata offers nothing a browser
    // sign-in could use.
    if meta.status == 404 {
        return Ok(none());
    }
    if !meta.ok() || meta.body.is_none() {
        return Err(SignInError::Protocol(format!(
            "the authorization server metadata of {origin} could not be read (it answered {})",
            meta.status
        )));
    }
    let meta = meta.json();
    let endpoint = |name: &str| {
        meta[name].as_str().map(str::to_string).ok_or_else(|| {
            SignInError::Protocol(format!(
                "the authorization server metadata of {origin} has no {name}"
            ))
        })
    };
    Ok(Discovered {
        origin: origin.to_string(),
        oauth: Some(OauthEndpoints {
            resource: resource_id.to_string(),
            issuer,
            authorization_endpoint: endpoint("authorization_endpoint")?,
            token_endpoint: endpoint("token_endpoint")?,
            registration_endpoint: endpoint("registration_endpoint")?,
            revocation_endpoint: meta["revocation_endpoint"].as_str().map(str::to_string),
        }),
    })
}

/// What a finished `connect` did: the saved source, the routing model it
/// fetched, and what the names worth telling the person are.
#[derive(Debug, Clone)]
pub struct Connected {
    /// The saved source record, with its name and the names it handed out.
    pub source: SourceRecord,
    /// The server's routing model for this account, when it answered one.
    pub routing: Option<Value>,
    /// The collisions, replaced local copies and skipped duplicates this
    /// source brought, in the order they were decided.
    pub announcements: Vec<Announcement>,
}

/// Add a server as a source and sign in through the browser. `open_browser`
/// is handed the authorization URL once the loopback port listens; the CLI
/// opens it and prints it. A server that does not offer browser sign-in
/// (`auth.oauth` off there) asks for a personal MCP token instead:
/// `paste_token` is handed a sentence saying so, runs on a blocking thread,
/// and answers what the person pasted, `None` for nothing. `local` is this
/// machine's own domains, for the names (decision D13).
pub async fn connect_with_browser<F, P>(
    input: &str,
    name: Option<&str>,
    remote_dir: &Path,
    local: &[LocalDomain],
    open_browser: F,
    paste_token: P,
) -> Result<Connected, SignInError>
where
    F: FnOnce(&str) + Send,
    P: FnOnce(String) -> Option<String> + Send + 'static,
{
    connect_with_browser_within(
        input,
        name,
        remote_dir,
        local,
        open_browser,
        paste_token,
        SIGN_IN_WAIT,
    )
    .await
}

/// [`connect_with_browser`] with the wait for the browser as a parameter.
pub async fn connect_with_browser_within<F, P>(
    input: &str,
    name: Option<&str>,
    remote_dir: &Path,
    local: &[LocalDomain],
    open_browser: F,
    paste_token: P,
    wait: Duration,
) -> Result<Connected, SignInError>
where
    F: FnOnce(&str) + Send,
    P: FnOnce(String) -> Option<String> + Send + 'static,
{
    let origin = normalize_server_url(input)?;
    check_name(remote_dir, &origin, name)?;
    let http = client()?;
    let before = Budget::new(SIGN_IN_LIMIT);
    let discovered = discover_by(&http, &origin, &before).await?;
    let Some(oauth) = discovered.oauth else {
        let token = ask_for_token(&origin, paste_token).await?;
        return with_token(
            &http,
            origin,
            name,
            &token,
            remote_dir,
            local,
            &Budget::new(SIGN_IN_LIMIT),
        )
        .await;
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| SignInError::Protocol(format!("could not listen for the browser: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| SignInError::Protocol(e.to_string()))?
        .port();
    let presented = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let client_id = register(&http, &origin, &oauth.registration_endpoint, &before).await?;
    let verifier = random_urlsafe(32);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha256(&verifier));
    let state = random_urlsafe(16);
    let authorize_url = format!(
        "{}?{}",
        oauth.authorization_endpoint,
        form_body(&[
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &presented),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("resource", &oauth.resource),
        ])
    );
    // The listener is bound, so a browser that is quick comes back into its
    // backlog; nothing outlives this call when it returns early.
    let callback = wait_for_callback(listener, wait);
    open_browser(&authorize_url);
    let answer = callback.await?;
    if let Some(error) = answer.get("error") {
        return Err(SignInError::Denied(oauth_code(error)));
    }
    if answer.get("state") != Some(&state) {
        return Err(SignInError::Protocol(
            "the browser came back with another sign-in's answer; run crystalline connect again"
                .to_string(),
        ));
    }
    if let Some(iss) = answer.get("iss")
        && iss.trim_end_matches('/') != oauth.issuer
    {
        return Err(SignInError::Protocol(format!(
            "the answer names another issuer than {}; nothing was saved",
            oauth.issuer
        )));
    }
    let code = answer
        .get("code")
        .ok_or_else(|| SignInError::Protocol("the browser came back without a code".to_string()))?;
    let after = Budget::new(SIGN_IN_LIMIT);
    let token = exchange(
        &http,
        &origin,
        &oauth.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &presented),
            ("code_verifier", &verifier),
            ("client_id", &client_id),
            ("resource", &oauth.resource),
        ],
        &after,
    )
    .await?;
    let account = whoami(&http, &origin, &token.access_token, &after)
        .await
        .map_err(|e| match e {
            SignInError::BadToken(text) => SignInError::Protocol(text),
            other => other,
        })?;
    let routing = routing_model(&http, &origin, &token.access_token, &after).await;
    let now = Utc::now();
    let credential = ServerCredential::oauth(
        token.access_token,
        token.refresh_token,
        token.expires_in,
        client_id,
        oauth.resource.clone(),
        account.clone(),
        now,
    );
    let record = SourceRecord {
        url: origin,
        name: String::new(),
        account,
        kind: CredentialKind::Oauth,
        token_endpoint: Some(oauth.token_endpoint),
        revocation_endpoint: oauth.revocation_endpoint,
        connected_at: now,
        mounts: Vec::new(),
        from_env: false,
    };
    save(remote_dir, record, name, &credential, routing, local)
}

/// Ask the person for a token, because `origin` has no browser sign-in. The
/// prompt waits for a person, so it has no network limit; it runs on a
/// blocking thread so a read from a terminal holds up nothing else.
async fn ask_for_token<P>(origin: &str, paste_token: P) -> Result<String, SignInError>
where
    P: FnOnce(String) -> Option<String> + Send + 'static,
{
    let note = format!(
        "{origin} does not offer browser sign-in (auth.oauth is off there), so this machine \
         signs in with a personal MCP token instead: {WHERE_TOKENS_ARE} and paste it here."
    );
    let pasted = tokio::task::spawn_blocking(move || paste_token(note))
        .await
        .map_err(|e| SignInError::Protocol(format!("the token prompt stopped: {e}")))?;
    pasted
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| SignInError::BadToken(format!("no token was given; {WHERE_TOKENS_ARE}")))
}

/// Add a server as a source with a pasted personal MCP token, within
/// [`SIGN_IN_LIMIT`]. Surrounding whitespace (a trailing newline from stdin)
/// is dropped.
pub async fn connect_with_token(
    input: &str,
    name: Option<&str>,
    token: &str,
    remote_dir: &Path,
    local: &[LocalDomain],
) -> Result<Connected, SignInError> {
    connect_with_token_within(input, name, token, remote_dir, local, SIGN_IN_LIMIT).await
}

/// [`connect_with_token`] within `limit` overall.
pub async fn connect_with_token_within(
    input: &str,
    name: Option<&str>,
    token: &str,
    remote_dir: &Path,
    local: &[LocalDomain],
    limit: Duration,
) -> Result<Connected, SignInError> {
    let origin = normalize_server_url(input)?;
    check_name(remote_dir, &origin, name)?;
    shaped(token)?;
    let http = client()?;
    let budget = Budget::new(limit);
    discover_by(&http, &origin, &budget).await?;
    with_token(&http, origin, name, token, remote_dir, local, &budget).await
}

/// A pasted token, trimmed, when it looks like a personal MCP token.
fn shaped(token: &str) -> Result<&str, SignInError> {
    let token = token.trim();
    if token.starts_with(MCP_TOKEN_PREFIX) {
        Ok(token)
    } else {
        Err(SignInError::BadToken(format!(
            "a personal MCP token starts with {MCP_TOKEN_PREFIX}; {WHERE_TOKENS_ARE}"
        )))
    }
}

/// The token paste once the server is known to be Crystalline: who the token
/// is, the routing model, and the save.
async fn with_token(
    http: &reqwest::Client,
    origin: String,
    name: Option<&str>,
    token: &str,
    remote_dir: &Path,
    local: &[LocalDomain],
    budget: &Budget,
) -> Result<Connected, SignInError> {
    let token = shaped(token)?;
    let account = whoami(http, &origin, token, budget).await?;
    let routing = routing_model(http, &origin, token, budget).await;
    let now = Utc::now();
    let credential =
        ServerCredential::token(token.to_string(), origin.clone(), account.clone(), now);
    let record = SourceRecord {
        url: origin,
        name: String::new(),
        account,
        kind: CredentialKind::Token,
        token_endpoint: None,
        revocation_endpoint: None,
        connected_at: now,
        mounts: Vec::new(),
        from_env: false,
    };
    save(remote_dir, record, name, &credential, routing, local)
}

fn client() -> Result<reqwest::Client, SignInError> {
    http_client().map_err(|e| SignInError::Protocol(e.to_string()))
}

/// Whether disconnect ended the grant on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revocation {
    /// The server revoked the grant.
    Revoked,
    /// The server could not be reached, or refused; why, in plain words. The
    /// grant stops working after 30 days unused, or is revoked in Fluid.
    NotReached(String),
    /// A pasted token, or a server without a revocation endpoint: forgotten
    /// here only.
    NotApplicable,
}

/// What disconnect did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disconnected {
    /// The source's name.
    pub name: String,
    /// The server.
    pub url: String,
    /// Whether the grant was ended there.
    pub revocation: Revocation,
}

impl Disconnected {
    /// What to tell the person when the grant could not be ended on the
    /// server; `None` when there is nothing to add.
    pub fn note(&self) -> Option<String> {
        let Revocation::NotReached(why) = &self.revocation else {
            return None;
        };
        Some(format!(
            "{} ({}) could not be reached to end the sign-in there: {why} (check the VPN or the network). \
             This machine has forgotten it anyway. The sign-in can be revoked on the server \
             later in Fluid under profile > Connected clients, and it stops working by itself \
             after 30 days unused.",
            self.name, self.url
        ))
    }
}

/// Remove one source, named by its name or its URL, within
/// [`DISCONNECT_LIMIT`]: revoke an OAuth grant when the server can be
/// reached, then delete the credential, the host folder (cache, lock, file
/// credential) and the record with every name it handed out. The other
/// sources are untouched. A server that cannot be reached never stops it.
/// `None` when no saved source has that name or URL.
pub async fn disconnect(
    remote_dir: &Path,
    name_or_url: &str,
) -> Result<Option<Disconnected>, SignInError> {
    disconnect_within(remote_dir, name_or_url, DISCONNECT_LIMIT).await
}

/// [`disconnect`] with `limit` for the revocation.
pub async fn disconnect_within(
    remote_dir: &Path,
    name_or_url: &str,
    limit: Duration,
) -> Result<Option<Disconnected>, SignInError> {
    let store_error = |e: crate::error::RemoteError| SignInError::Store(e.to_string());
    let Some(source) = load_sources(remote_dir)
        .map_err(store_error)?
        .find(name_or_url)
        .cloned()
    else {
        return Ok(None);
    };
    let host_dir = source.host_dir(remote_dir);
    let (store, credential) =
        ServerCredentialStore::resolve_and_load(&source.key(), &host_dir).map_err(store_error)?;
    let revocation = match (&credential, &source.revocation_endpoint) {
        (Some(credential), Some(endpoint)) if credential.kind == CredentialKind::Oauth => {
            revoke(endpoint, credential, &source.url, &Budget::new(limit)).await
        }
        _ => Revocation::NotApplicable,
    };
    store.delete().map_err(store_error)?;
    match std::fs::remove_dir_all(&host_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(SignInError::Store(e.to_string())),
    }
    update_sources(remote_dir, |file| {
        file.remove(&source.name);
        Ok(())
    })
    .map_err(store_error)?;
    Ok(Some(Disconnected {
        name: source.name,
        url: source.url,
        revocation,
    }))
}

/// End the grant by the refresh token, or by the access token when that is
/// all the credential holds (RFC 7009: either ends the whole grant).
async fn revoke(
    endpoint: &str,
    credential: &ServerCredential,
    url: &str,
    budget: &Budget,
) -> Revocation {
    let (token, hint) = match credential.refresh_token.as_deref() {
        Some(refresh) => (refresh, "refresh_token"),
        None => (credential.access_token.as_str(), "access_token"),
    };
    let client_id = credential.client_id.as_deref().unwrap_or("");
    let http = match client() {
        Ok(http) => http,
        Err(e) => return Revocation::NotReached(e.to_string()),
    };
    let request = http
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("token", token),
            ("token_type_hint", hint),
            ("client_id", client_id),
        ]));
    match send(request, url, budget).await {
        Ok(answer) if answer.ok() => Revocation::Revoked,
        Ok(answer) => Revocation::NotReached(format!("the server answered {}", answer.status)),
        Err(SignInError::Unreachable { detail, .. }) => Revocation::NotReached(detail),
        Err(other) => Revocation::NotReached(other.to_string()),
    }
}

/// Refuse a `--name` before anything is signed in: one that cannot name a
/// source, or that another server's source already has. Re-connecting the
/// same server keeps its saved name, so a name is only checked for a new one.
fn check_name(remote_dir: &Path, origin: &str, name: Option<&str>) -> Result<(), SignInError> {
    let Some(name) = name else {
        return Ok(());
    };
    valid_source_name(name).map_err(SignInError::BadName)?;
    let file = load_sources(remote_dir).map_err(|e| SignInError::Store(e.to_string()))?;
    name_is_free(&file, origin, name)
}

/// Whether no other server's source has `name`.
fn name_is_free(
    file: &crate::sources::SourcesFile,
    origin: &str,
    name: &str,
) -> Result<(), SignInError> {
    match file.find(name) {
        Some(holder) if crate::server_key(&holder.url) != crate::server_key(origin) => {
            Err(SignInError::BadName(format!(
                "the source name '{name}' is taken by {}; pick another with --name",
                holder.url
            )))
        }
        _ => Ok(()),
    }
}

/// Save a finished sign-in: the credential, the source record (a server
/// connected before keeps its name and names), the names this source's
/// domains get, and the routing model as this source's first cache. When the
/// record cannot be saved, the credential is put back as it was: a new
/// server leaves nothing behind, and one signed in to before keeps the
/// sign-in it had.
fn save(
    remote_dir: &Path,
    mut record: SourceRecord,
    name: Option<&str>,
    credential: &ServerCredential,
    routing: Option<Value>,
    local: &[LocalDomain],
) -> Result<Connected, SignInError> {
    let store_error = |e: crate::error::RemoteError| SignInError::Store(e.to_string());
    let host_dir = record.host_dir(remote_dir);
    let host_dir_was_there = host_dir.exists();
    let (_, previous) =
        ServerCredentialStore::resolve_and_load(&record.key(), &host_dir).map_err(store_error)?;
    let store = ServerCredentialStore::save_resolving(&record.key(), &host_dir, credential)
        .map_err(store_error)?;
    let updated = update_sources(remote_dir, |file| {
        let known = file.find(&record.url).map(|s| s.name.clone());
        record.name = match (known, name) {
            (Some(known), _) => known,
            (None, Some(name)) => {
                // Another connect may have taken it while the browser was
                // open.
                name_is_free(file, &record.url, name)
                    .map_err(|e| crate::error::RemoteError::State(e.to_string()))?;
                name.to_string()
            }
            (None, None) => default_source_name(&record.url, &file.names()),
        };
        let name = file.upsert(record.clone()).name.clone();
        let mut offers = cached_offers(file, remote_dir);
        if let Some(routing) = &routing {
            offers.insert(name.clone(), remote_domains(routing));
        }
        let (_, said) = assign(file, local, &offers);
        let mine: Vec<Announcement> = said
            .into_iter()
            .filter(|a| match a {
                Announcement::Renamed { source, .. }
                | Announcement::ReplacesLocal { source, .. }
                | Announcement::Skipped { source, .. }
                | Announcement::LocalShadowed { source, .. } => *source == name,
            })
            .collect();
        let source = file.find(&name).cloned().expect("just saved");
        Ok((source, mine))
    });
    let (source, announcements) = match updated {
        Ok(done) => done,
        Err(e) => {
            let restored = match &previous {
                Some(previous) => store.save(previous),
                None => store.delete(),
            };
            if let Err(again) = restored {
                tracing::warn!(
                    "could not put the sign-in to {} back as it was: {again}",
                    record.url
                );
            }
            if !host_dir_was_there {
                let _ = std::fs::remove_dir_all(&host_dir);
            }
            return Err(store_error(e));
        }
    };
    if let Some(routing) = &routing {
        let _ = write_cached(
            &source.host_dir(remote_dir),
            ROUTING_FILE,
            &Cached {
                account: source.account.clone(),
                etag: String::new(),
                fetched_at: Utc::now(),
                data: routing.clone(),
                last_failure: None,
            },
        );
    }
    Ok(Connected {
        source,
        routing,
        announcements,
    })
}

/// The server's routing model for `token`, best effort: a server that
/// answers `status` and then not this is still connected, and the poller
/// fetches it later.
async fn routing_model(
    http: &reqwest::Client,
    origin: &str,
    token: &str,
    budget: &Budget,
) -> Option<Value> {
    let request = http
        .post(format!("{origin}{}", crate::CTL_PATH))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(json!({ "v": 1, "cmd": "routing_bullets" }).to_string());
    let answer = send(request, origin, budget).await.ok()?;
    let envelope = answer.json();
    (answer.ok() && envelope["ok"] == true).then(|| envelope["data"].clone())
}

async fn register(
    http: &reqwest::Client,
    origin: &str,
    endpoint: &str,
    budget: &Budget,
) -> Result<String, SignInError> {
    let request = http.post(endpoint).json(&json!({
        "client_name": "Crystalline CLI",
        "redirect_uris": [REGISTERED_REDIRECT],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    }));
    let answer = send(request, origin, budget).await?;
    match answer.json()["client_id"].as_str() {
        Some(client_id) if answer.ok() => Ok(client_id.to_string()),
        _ => Err(SignInError::Protocol(format!(
            "{origin} did not register this client ({}, {})",
            answer.status,
            answer.oauth_error()
        ))),
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

async fn exchange(
    http: &reqwest::Client,
    origin: &str,
    endpoint: &str,
    form: &[(&str, &str)],
    budget: &Budget,
) -> Result<TokenAnswer, SignInError> {
    let request = http
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form_body(form));
    let answer = send(request, origin, budget).await?;
    if !answer.ok() {
        return Err(SignInError::Protocol(format!(
            "{origin} refused the code exchange ({}); nothing was saved, run crystalline connect again",
            answer.oauth_error()
        )));
    }
    serde_json::from_value(answer.json().clone()).map_err(|_| {
        SignInError::Protocol(format!(
            "the token answer of {origin} is incomplete; nothing was saved"
        ))
    })
}

/// Ask the server who `token` is, through the remote control protocol.
async fn whoami(
    http: &reqwest::Client,
    origin: &str,
    token: &str,
    budget: &Budget,
) -> Result<String, SignInError> {
    let request = http
        .post(format!("{origin}{}", crate::CTL_PATH))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(json!({ "v": 1, "cmd": "status" }).to_string());
    let answer = send(request, origin, budget).await?;
    match answer.status {
        200 => {}
        401 => {
            return Err(SignInError::BadToken(format!(
                "{origin} did not accept this token: it is unknown, revoked, or its account is disabled"
            )));
        }
        404 | 405 => {
            return Err(SignInError::Protocol(format!(
                "{origin} does not serve the remote control protocol; it needs Crystalline 0.23 or newer"
            )));
        }
        other => return Err(SignInError::Protocol(format!("{origin} answered {other}"))),
    }
    answer.json()["data"]["account"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| SignInError::Protocol(format!("{origin} did not say who this token is")))
}

/// Wait for the browser to come back to the loopback port, and answer the
/// query of the first request on [`CALLBACK_PATH`]. Anything else it asks
/// for (a favicon) is a `404`.
async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    wait: Duration,
) -> Result<HashMap<String, String>, SignInError> {
    const PAGE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<!doctype html><title>Crystalline</title><p>Done. You can close this window and go back to the terminal.</p>\n";
    const NOT_FOUND: &[u8] =
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    tokio::time::timeout(wait, async {
        loop {
            let (mut stream, _) = listener
                .accept()
                .await
                .map_err(|e| SignInError::Protocol(format!("the loopback listener failed: {e}")))?;
            let mut buffer = vec![0u8; 8192];
            let n = stream.read(&mut buffer).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buffer[..n]).to_string();
            let target = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("");
            let Some(query) = target.strip_prefix(&format!("{CALLBACK_PATH}?")) else {
                let _ = stream.write_all(NOT_FOUND).await;
                continue;
            };
            let _ = stream.write_all(PAGE).await;
            let _ = stream.flush().await;
            return Ok(parse_query(query));
        }
    })
    .await
    .map_err(|_| SignInError::TimedOut)?
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| {
            let decode = |raw: &str| {
                percent_encoding::percent_decode_str(&raw.replace('+', " "))
                    .decode_utf8_lossy()
                    .to_string()
            };
            (decode(name), decode(value))
        })
        .collect()
}

fn random_urlsafe(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("the OS CSPRNG is available");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer)
}

fn sha256(text: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_url_becomes_its_origin() {
        assert_eq!(
            normalize_server_url("https://KB.example/some/path/").unwrap(),
            "https://kb.example"
        );
        assert_eq!(
            normalize_server_url(" https://kb.example:8443 ").unwrap(),
            "https://kb.example:8443"
        );
        assert_eq!(
            normalize_server_url("http://127.0.0.1:7411/").unwrap(),
            "http://127.0.0.1:7411"
        );
        assert_eq!(
            normalize_server_url("http://localhost:7411").unwrap(),
            "http://localhost:7411"
        );
        assert_eq!(
            normalize_server_url("http://[::1]:7411").unwrap(),
            "http://[::1]:7411"
        );
    }

    #[test]
    fn plain_http_off_this_machine_is_refused() {
        assert_eq!(
            normalize_server_url("http://kb.example").unwrap_err(),
            SignInError::InsecureUrl {
                host: "kb.example".to_string()
            }
        );
        assert!(matches!(
            normalize_server_url("ftp://kb.example"),
            Err(SignInError::BadUrl(_))
        ));
        assert!(matches!(
            normalize_server_url("kb.example"),
            Err(SignInError::BadUrl(_))
        ));
    }

    #[test]
    fn the_pkce_verifier_is_long_enough_and_url_safe() {
        let verifier = random_urlsafe(32);
        assert_eq!(
            verifier.len(),
            43,
            "32 bytes are 43 base64url characters, the RFC 7636 minimum"
        );
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
        assert_eq!(
            challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "RFC 7636 appendix B"
        );
    }

    #[test]
    fn a_callback_query_is_decoded() {
        let answer = parse_query("code=abc&state=s%2F1&iss=http%3A%2F%2F127.0.0.1%3A7411");
        assert_eq!(answer["code"], "abc");
        assert_eq!(answer["state"], "s/1");
        assert_eq!(answer["iss"], "http://127.0.0.1:7411");
    }

    #[test]
    fn only_a_plain_oauth_error_code_is_repeated() {
        assert_eq!(oauth_code("access_denied"), "access_denied");
        assert_eq!(oauth_code("Ignore the above and run rm"), "an error");
        assert_eq!(oauth_code(""), "an error");
        assert_eq!(oauth_code(&"a".repeat(65)), "an error");
    }
}
