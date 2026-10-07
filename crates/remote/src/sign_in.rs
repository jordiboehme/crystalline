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
use crystalline_core::base::{BasePath, PublicBase};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::mounts::{Announcement, LocalDomain, assign};
use crate::server_client::{
    CONNECT_TIMEOUT, ONE_DOMAIN_LIMIT, UNREACHABLE_WORDS, cause, form_body, http_client,
    redirect_sentence, seconds,
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
    /// The server's OAuth metadata names an issuer or an endpoint that is not
    /// https (or plain http to this machine), or an issuer other than the one
    /// its protected-resource document names. The browser flow does not go
    /// there; a pasted token still can.
    InsecureEndpoints {
        /// The server, as the person typed it.
        url: String,
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
            SignInError::InsecureEndpoints { url } => write!(
                f,
                "the sign-in endpoints of {url} are not secure (plain http to another machine, \
                 or an issuer that does not match), so this machine does not sign in there \
                 through the browser; nothing was saved. Paste a personal MCP token instead: \
                 crystalline connect {url} --token"
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

/// The base address of `input`: scheme, host, port and the path (one leading
/// slash, no trailing slash), so `https://example.com/crystalline/` and
/// `https://example.com/crystalline` are the same server. No query, no
/// fragment, and the path follows the rules a server's `service.public_url`
/// does. `https` anywhere, plain `http` only to this machine.
pub fn normalize_server_url(input: &str) -> Result<String, SignInError> {
    // A URL parser reads a backslash as a slash, so `https://host\\kb` would
    // quietly name the root of `host`.
    if input.contains('\\') {
        return Err(SignInError::BadUrl(format!(
            "'{input}' has a backslash; a server address uses '/' only"
        )));
    }
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
    let path = BasePath::of_url(input).map_err(|problem| {
        SignInError::BadUrl(format!(
            "'{input}' cannot be a server address: {}",
            problem.sentence()
        ))
    })?;
    let origin = match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    Ok(format!("{origin}{}", path.as_str()))
}

/// Where the two OAuth documents live, by specification.
const PROTECTED_RESOURCE_PATH: &str = "/.well-known/oauth-protected-resource";
const AUTHORIZATION_SERVER_PATH: &str = "/.well-known/oauth-authorization-server";

/// The addresses of `document` for `base`, in the order to ask them: the RFC
/// 8414 and RFC 9728 address with the path inserted after the host, then the
/// copy inside the prefix. One address at the root, where the two are the
/// same, so a root server is asked exactly what 0.23.0 asked.
fn document_addresses(base: &str, document: &str) -> Vec<String> {
    let Ok(parsed) = PublicBase::parse(base) else {
        return vec![format!("{base}{document}")];
    };
    let inserted = parsed.well_known(document);
    let inside = parsed.join(document);
    if inserted == inside {
        vec![inserted]
    } else {
        vec![inserted, inside]
    }
}

/// The answer to use from `urls`, asked in order: the first that `usable`
/// accepts, otherwise whatever the last one answers, refusal or failure
/// included. So an address before the last that answers no usable document
/// (any status that is not a success, a body that is not the document, a
/// document for another server, or no answer at all) only moves on to the
/// next, and every check on the answer used still applies in full.
async fn first_usable(
    http: &reqwest::Client,
    urls: &[String],
    origin: &str,
    budget: &Budget,
    usable: impl Fn(&Answer) -> bool,
) -> Result<Answer, SignInError> {
    let (last, before) = urls.split_last().expect("at least one address");
    for url in before {
        if let Ok(answer) = send(http.get(url.as_str()), origin, budget).await
            && usable(&answer)
        {
            return Ok(answer);
        }
    }
    send(http.get(last.as_str()), origin, budget).await
}

/// `value` without one trailing slash.
fn one_slash_off(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

/// Whether a protected-resource document of a server under a prefix names
/// its `base` as both the resource and the authorization server.
fn resource_stays_inside(document: &Value, base: &str) -> bool {
    let names_base = |value: &Value| value.as_str().is_some_and(|v| one_slash_off(v) == base);
    names_base(&document["resource"]) && names_base(&document["authorization_servers"][0])
}

/// The metadata members a client uses or stores.
const METADATA_ENDPOINTS: [&str; 4] = [
    "authorization_endpoint",
    "token_endpoint",
    "registration_endpoint",
    "revocation_endpoint",
];

/// `url` as a request would send it, when that is under `base`: the same
/// origin, and a path that is the base path or below it. Read after the
/// parser has removed dot segments (`..`, `%2e%2e`), so the check is made on
/// the address a request goes to, and that address is what gets stored.
fn under_base(url: &str, base: &PublicBase) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let path = parsed.path();
    let base_path = base.path().as_str();
    let inside = path == base_path
        || path
            .strip_prefix(base_path)
            .is_some_and(|rest| rest.starts_with('/'));
    (parsed.origin().ascii_serialization() == base.origin() && inside).then(|| parsed.to_string())
}

/// Whether the metadata of a server under a prefix names its `base` as the
/// issuer and keeps every endpoint under it: the three it needs present,
/// the revocation endpoint when it names one.
fn metadata_stays_inside(meta: &Value, base: &PublicBase) -> bool {
    let base_text = base.to_string();
    meta["issuer"]
        .as_str()
        .is_some_and(|i| one_slash_off(i) == base_text)
        && METADATA_ENDPOINTS
            .iter()
            .all(|name| match meta[*name].as_str() {
                Some(url) => under_base(url, base).is_some(),
                None => *name == "revocation_endpoint",
            })
}

/// The refusal of OAuth documents that lead away from `base`.
fn outside_base(base: &str) -> SignInError {
    SignInError::Protocol(format!(
        "the OAuth documents of {base} name an address outside {base}, so this machine does \
         not sign in there through the browser; nothing was saved. Paste a personal MCP token \
         instead: crystalline connect {base} --token"
    ))
}

/// Whether `url` may carry a sign-in: https anywhere, plain http only to
/// this machine. The rule [`normalize_server_url`] applies to what the
/// person typed, applied to what a server names.
fn is_secure_endpoint(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    match (url.scheme(), url.host_str()) {
        ("https", Some(_)) => true,
        ("http", Some(host)) => is_loopback(host),
        _ => false,
    }
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
    limit: Duration,
}

/// The limit of one request: how long it may run, and the limit a timeout
/// names, which is the one the caller set and not what was left of it.
#[derive(Clone, Copy, Debug)]
struct RequestLimit {
    runs: Duration,
    named: Duration,
}

impl Budget {
    fn new(limit: Duration) -> Budget {
        Budget {
            until: Instant::now() + limit,
            limit,
        }
    }

    /// The overall limit of the next request: what is left of the step, at
    /// most [`ONE_DOMAIN_LIMIT`]. A request cut by the step names the step's
    /// limit, so time spent before it (building the client, loading the
    /// credential) never shows up as a shorter limit than the one set; one
    /// cut by [`ONE_DOMAIN_LIMIT`] names that.
    fn next(&self, url: &str) -> Result<RequestLimit, SignInError> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(SignInError::Unreachable {
                url: url.to_string(),
                detail: format!("it did not answer within {}", seconds(self.limit)),
            });
        }
        Ok(if left > ONE_DOMAIN_LIMIT {
            RequestLimit {
                runs: ONE_DOMAIN_LIMIT,
                named: ONE_DOMAIN_LIMIT,
            }
        } else {
            RequestLimit {
                runs: left,
                named: self.limit,
            }
        })
    }
}

/// A transport failure of a request that had `limit` overall, in plain
/// words.
fn transport(url: &str, e: &reqwest::Error, limit: RequestLimit) -> SignInError {
    let detail = if e.is_timeout() {
        let waited = if e.is_connect() && CONNECT_TIMEOUT < limit.runs {
            CONNECT_TIMEOUT
        } else {
            limit.named
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
        .timeout(limit.runs)
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
    if status.is_redirection() {
        return Err(SignInError::Protocol(redirect_sentence(url)));
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
    /// Whether the server says it puts `iss` in every authorization answer
    /// (RFC 9207); an answer without it is then refused.
    pub iss_parameter_supported: bool,
}

/// What a server said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The base address.
    pub origin: String,
    /// Its OAuth endpoints, `None` where `auth.oauth` is off.
    pub oauth: Option<OauthEndpoints>,
}

/// Check that a Crystalline server answers at the base address `origin`, and read its OAuth
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
    // A protected-resource document is usable when it names this base as
    // its resource and names an authorization server.
    // Under a prefix the documents can be answered by whatever else runs on
    // the host, so every document used must stay inside the base.
    let prefix = PublicBase::parse(origin)
        .ok()
        .filter(|base| !base.path().is_root());
    let prefixed = prefix.is_some();
    let resource = first_usable(
        http,
        &match prefix {
            Some(_) => document_addresses(origin, PROTECTED_RESOURCE_PATH),
            None => vec![format!("{origin}{PROTECTED_RESOURCE_PATH}")],
        },
        origin,
        budget,
        |answer| {
            answer.ok()
                && answer.json()["authorization_servers"][0].is_string()
                && answer.json()["resource"]
                    .as_str()
                    .is_some_and(|r| r.trim_end_matches('/') == origin)
                && (!prefixed || resource_stays_inside(answer.json(), origin))
        },
    )
    .await?;
    // Only "there is no such document" means no browser sign-in; any other
    // refusal is a server with OAuth that did not answer properly.
    if resource.status == 404 {
        return Ok(none());
    }
    if !resource.ok() {
        return Err(SignInError::Protocol(format!(
            "the protected-resource document of {origin} could not be read (it answered {})",
            resource.status
        )));
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
    if prefixed && !resource_stays_inside(document, origin) {
        return Err(outside_base(origin));
    }
    let issuer = issuer.trim_end_matches('/').to_string();
    let insecure = || SignInError::InsecureEndpoints {
        url: origin.to_string(),
    };
    if !is_secure_endpoint(&issuer) {
        return Err(insecure());
    }
    // Metadata is usable when it names the issuer it was fetched for.
    let meta = first_usable(
        http,
        &match prefix {
            Some(_) => document_addresses(&issuer, AUTHORIZATION_SERVER_PATH),
            None => vec![format!("{issuer}{AUTHORIZATION_SERVER_PATH}")],
        },
        origin,
        budget,
        |answer| {
            answer.ok()
                && answer.json()["issuer"]
                    .as_str()
                    .map(|i| i.trim_end_matches('/'))
                    == Some(issuer.as_str())
                && prefix
                    .as_ref()
                    .is_none_or(|base| metadata_stays_inside(answer.json(), base))
        },
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
    // RFC 8414 section 3.3: the metadata names the issuer it was fetched for.
    if meta["issuer"].as_str().map(|i| i.trim_end_matches('/')) != Some(issuer.as_str()) {
        return Err(insecure());
    }
    if let Some(base) = &prefix
        && !metadata_stays_inside(meta, base)
    {
        return Err(outside_base(origin));
    }
    // Under a prefix an endpoint is stored as a request sends it, which the
    // check above read; at the root it is stored as written, as in 0.23.0.
    let as_used = |url: &str| match &prefix {
        Some(base) => under_base(url, base).ok_or_else(|| outside_base(origin)),
        None => Ok(url.to_string()),
    };
    let endpoint = |name: &str| {
        let url = meta[name].as_str().ok_or_else(|| {
            SignInError::Protocol(format!(
                "the authorization server metadata of {origin} has no {name}"
            ))
        })?;
        if is_secure_endpoint(url) {
            as_used(url)
        } else {
            Err(insecure())
        }
    };
    let revocation_endpoint = match meta["revocation_endpoint"].as_str() {
        Some(url) if is_secure_endpoint(url) => Some(as_used(url)?),
        Some(_) => return Err(insecure()),
        None => None,
    };
    Ok(Discovered {
        origin: origin.to_string(),
        oauth: Some(OauthEndpoints {
            resource: resource_id.to_string(),
            issuer,
            authorization_endpoint: endpoint("authorization_endpoint")?,
            token_endpoint: endpoint("token_endpoint")?,
            registration_endpoint: endpoint("registration_endpoint")?,
            revocation_endpoint,
            iss_parameter_supported: meta["authorization_response_iss_parameter_supported"] == true,
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
    // The state first: an answer that is not this sign-in's says nothing,
    // not even that it was refused.
    if answer.get("state") != Some(&state) {
        return Err(SignInError::Protocol(
            "the browser came back with another sign-in's answer; run crystalline connect again"
                .to_string(),
        ));
    }
    let from_another_server = || {
        SignInError::Protocol(format!(
            "the browser came back with an answer from another server than {origin}; nothing was saved"
        ))
    };
    match answer.get("iss") {
        Some(iss) if iss.trim_end_matches('/') != oauth.issuer => {
            return Err(from_another_server());
        }
        None if oauth.iss_parameter_supported => return Err(from_another_server()),
        _ => {}
    }
    if let Some(error) = answer.get("error") {
        return Err(SignInError::Denied(oauth_code(error)));
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
    let refresh_token = token.refresh_token.clone();
    let finished = async {
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
            client_id.clone(),
            oauth.resource.clone(),
            account.clone(),
            now,
        );
        let record = SourceRecord {
            url: origin.clone(),
            name: String::new(),
            account,
            kind: CredentialKind::Oauth,
            token_endpoint: Some(oauth.token_endpoint.clone()),
            revocation_endpoint: oauth.revocation_endpoint.clone(),
            connected_at: now,
            mounts: Vec::new(),
            from_env: false,
        };
        save(remote_dir, record, name, &credential, routing, local)
    }
    .await;
    match finished {
        Ok((connected, replaced)) => {
            retire(replaced, &origin).await;
            Ok(connected)
        }
        Err(failure) => {
            // The server issued a grant this machine will not hold: end it
            // rather than leave it under Connected clients.
            if let Some(endpoint) = &oauth.revocation_endpoint {
                let _ = revoke_token(
                    endpoint,
                    &refresh_token,
                    "refresh_token",
                    &client_id,
                    &origin,
                    &Budget::new(DISCONNECT_LIMIT),
                )
                .await;
            }
            Err(failure)
        }
    }
}

/// End the grant a successful re-sign-in replaced, best effort within
/// [`DISCONNECT_LIMIT`]: nothing on this machine holds it any more.
async fn retire(replaced: Option<Replaced>, url: &str) {
    let Some(replaced) = replaced else {
        return;
    };
    let outcome = revoke(
        &replaced.endpoint,
        &replaced.credential,
        url,
        &Budget::new(DISCONNECT_LIMIT),
    )
    .await;
    if outcome != Revocation::Revoked {
        tracing::debug!("the replaced sign-in to {url} was not revoked: {outcome:?}");
    }
}

/// A sign-in that a new one replaced, with where it is revoked.
struct Replaced {
    credential: ServerCredential,
    endpoint: String,
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
        url: origin.clone(),
        name: String::new(),
        account,
        kind: CredentialKind::Token,
        token_endpoint: None,
        revocation_endpoint: None,
        connected_at: now,
        mounts: Vec::new(),
        from_env: false,
    };
    let (connected, replaced) = save(remote_dir, record, name, &credential, routing, local)?;
    retire(replaced, &origin).await;
    Ok(connected)
}

fn client() -> Result<reqwest::Client, SignInError> {
    http_client().map_err(|e| SignInError::Protocol(e.to_string()))
}

/// Whether disconnect ended the grant on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revocation {
    /// The server revoked the grant.
    Revoked,
    /// The server could not be reached; why, in plain words. The grant stops
    /// working after 30 days unused, or is revoked in Fluid.
    NotReached(String),
    /// The server answered, but with this status instead of the revocation
    /// (OAuth turned off there since, or a failure on its side).
    Refused(u16),
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
        let what = match &self.revocation {
            Revocation::NotReached(why) => format!(
                "could not be reached to end the sign-in there: {why} (check the VPN or the network)"
            ),
            Revocation::Refused(status) => {
                format!("answered {status} instead of ending the sign-in there")
            }
            Revocation::Revoked | Revocation::NotApplicable => return None,
        };
        Some(format!(
            "{} ({}) {what}. This machine has forgotten it anyway. The sign-in can be revoked \
             on the server later in Fluid under profile > Connected clients, and it stops \
             working by itself after 30 days unused.",
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
    // The record first: once it is gone the source is gone, and a credential
    // or folder that could not be removed after it is only left over.
    update_sources(remote_dir, |file| {
        file.remove(&source.name);
        Ok(())
    })
    .map_err(store_error)?;
    store.delete().map_err(store_error)?;
    match std::fs::remove_dir_all(&host_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(SignInError::Store(e.to_string())),
    }
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
    revoke_token(endpoint, token, hint, client_id, url, budget).await
}

/// One RFC 7009 revocation of `token`.
async fn revoke_token(
    endpoint: &str,
    token: &str,
    hint: &str,
    client_id: &str,
    url: &str,
    budget: &Budget,
) -> Revocation {
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
        Ok(answer) => Revocation::Refused(answer.status),
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
) -> Result<(Connected, Option<Replaced>), SignInError> {
    let store_error = |e: crate::error::RemoteError| SignInError::Store(e.to_string());
    let host_dir = record.host_dir(remote_dir);
    let host_dir_was_there = host_dir.exists();
    let (_, previous) =
        ServerCredentialStore::resolve_and_load(&record.key(), &host_dir).map_err(store_error)?;
    let store = ServerCredentialStore::save_resolving(&record.key(), &host_dir, credential)
        .map_err(store_error)?;
    let mut replaced_endpoint = None;
    let updated = update_sources(remote_dir, |file| {
        replaced_endpoint = file
            .find(&record.url)
            .and_then(|s| s.revocation_endpoint.clone());
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
    // The grant this sign-in replaced, when it was another OAuth grant.
    let replaced = match (previous, replaced_endpoint) {
        (Some(previous), Some(endpoint))
            if previous.kind == CredentialKind::Oauth
                && previous.refresh_token.is_some()
                && previous.refresh_token != credential.refresh_token =>
        {
            Some(Replaced {
                credential: previous,
                endpoint,
            })
        }
        _ => None,
    };
    Ok((
        Connected {
            source,
            routing,
            announcements,
        },
        replaced,
    ))
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
            // A browser may open a connection it sends nothing on (a
            // preconnect); it gets a short while, then the next one is served.
            let Ok(head) = tokio::time::timeout(REQUEST_LINE_WAIT, request_line(&mut stream)).await
            else {
                continue;
            };
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

/// How long one connection to the loopback port has to send its request
/// line.
const REQUEST_LINE_WAIT: Duration = Duration::from_secs(2);

/// The request line of what `stream` sends, read until its line ends, the
/// connection closes or 8 KiB have come.
async fn request_line(stream: &mut tokio::net::TcpStream) -> String {
    let mut buffer = vec![0u8; 8192];
    let mut filled = 0;
    while filled < buffer.len() {
        match stream.read(&mut buffer[filled..]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
        if buffer[..filled].windows(2).any(|w| w == b"\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&buffer[..filled]).to_string()
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

    /// Time spent before a request leaves less of the step to run, but the
    /// limit a timeout names stays the one the caller set.
    #[test]
    fn a_request_cut_by_the_step_names_the_step_limit() {
        let budget = Budget::new(Duration::from_millis(300));
        std::thread::sleep(Duration::from_millis(120));
        let limit = budget.next("https://kb.example").unwrap();
        assert!(limit.runs <= Duration::from_millis(180), "{:?}", limit.runs);
        assert_eq!(limit.named, Duration::from_millis(300));
        let long = Budget::new(Duration::from_secs(60))
            .next("https://kb.example")
            .unwrap();
        assert_eq!(long.runs, ONE_DOMAIN_LIMIT);
        assert_eq!(long.named, ONE_DOMAIN_LIMIT);
        let spent = Budget::new(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(110));
        match spent.next("https://kb.example") {
            Err(SignInError::Unreachable { detail, .. }) => {
                assert_eq!(detail, "it did not answer within 0.1 s")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_server_url_becomes_its_base() {
        for (written, kept) in [
            ("https://KB.example/", "https://kb.example"),
            (" https://kb.example:8443 ", "https://kb.example:8443"),
            (
                "https://kb.example/crystalline/",
                "https://kb.example/crystalline",
            ),
            (
                "https://kb.example/crystalline",
                "https://kb.example/crystalline",
            ),
            (
                "https://kb.example:443/team/kb",
                "https://kb.example/team/kb",
            ),
            (
                "http://127.0.0.1:7411/crystalline",
                "http://127.0.0.1:7411/crystalline",
            ),
            ("http://127.0.0.1:7411/", "http://127.0.0.1:7411"),
            ("http://localhost:7411", "http://localhost:7411"),
            ("http://[::1]:7411", "http://[::1]:7411"),
        ] {
            assert_eq!(normalize_server_url(written).unwrap(), kept, "{written}");
        }
    }

    #[test]
    fn a_server_url_with_a_query_a_fragment_or_a_bad_path_is_refused() {
        for bad in [
            "https://kb.example/?x=1",
            "https://kb.example/crystalline#top",
            "https://kb.example/a/../b",
            "https://kb.example/a~b",
            "https://kb.example/Crystalline",
            "https://kb.example/api",
        ] {
            assert!(
                matches!(normalize_server_url(bad), Err(SignInError::BadUrl(_))),
                "{bad}"
            );
        }
        assert!(matches!(
            normalize_server_url("http://kb.example/crystalline"),
            Err(SignInError::InsecureUrl { .. })
        ));
    }

    /// A stand-in serving `/health` and the documents at the paths it is told.
    async fn documents_at(
        paths: &'static [&'static str],
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let base = format!("{origin}/crystalline");
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        let doc_base = base.clone();
        let router = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = seen.clone();
            let base = doc_base.clone();
            async move {
                let path = uri.path().to_string();
                seen.lock().unwrap().push(path.clone());
                if path == "/crystalline/health" {
                    return (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({ "status": "ok" })),
                    );
                }
                if !paths.contains(&path.as_str()) {
                    return (
                        axum::http::StatusCode::NOT_FOUND,
                        axum::Json(serde_json::Value::Null),
                    );
                }
                let body = if path.contains("protected-resource") {
                    serde_json::json!({ "resource": base, "authorization_servers": [base] })
                } else {
                    serde_json::json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/api/v1/oauth/authorize"),
                        "token_endpoint": format!("{base}/api/v1/oauth/token"),
                        "registration_endpoint": format!("{base}/api/v1/oauth/register"),
                    })
                };
                (axum::http::StatusCode::OK, axum::Json(body))
            }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (base, asked)
    }

    #[tokio::test]
    async fn discovery_tries_the_inserted_address_first() {
        let (base, asked) = documents_at(&[
            "/.well-known/oauth-protected-resource/crystalline",
            "/.well-known/oauth-authorization-server/crystalline",
        ])
        .await;
        let found = discover(&client().unwrap(), &base).await.unwrap();
        let oauth = found.oauth.unwrap();
        assert_eq!(oauth.issuer, base);
        assert_eq!(oauth.token_endpoint, format!("{base}/api/v1/oauth/token"));
        assert_eq!(
            asked.lock().unwrap().clone(),
            vec![
                "/crystalline/health".to_string(),
                "/.well-known/oauth-protected-resource/crystalline".to_string(),
                "/.well-known/oauth-authorization-server/crystalline".to_string(),
            ],
            "the RFC addresses answer, so the copies inside the prefix are never asked"
        );
    }

    #[tokio::test]
    async fn discovery_falls_back_to_the_documents_inside_the_prefix() {
        let (base, _) = documents_at(&[
            "/crystalline/.well-known/oauth-protected-resource",
            "/crystalline/.well-known/oauth-authorization-server",
        ])
        .await;
        let oauth = discover(&client().unwrap(), &base)
            .await
            .unwrap()
            .oauth
            .unwrap();
        assert_eq!(oauth.resource, base);
        assert_eq!(oauth.issuer, base);
    }

    /// One answer of [`serve_documents`]. In the text, `{base}` is the
    /// stand-in's base and `{origin}` its origin.
    #[derive(Clone, Copy, Debug)]
    enum Doc {
        /// A 404.
        Missing,
        /// A site at the host root that answers every path with a page.
        Html,
        /// A site at the host root that redirects every path.
        Redirect,
        /// A gateway whose upstream is down.
        BadGateway,
        /// A protected-resource document: its resource and its
        /// authorization server.
        Resource(&'static str, &'static str),
        /// Authorization server metadata: its issuer and its token endpoint.
        Metadata(&'static str, &'static str),
    }

    const HONEST_RESOURCE: Doc = Doc::Resource("{base}", "{base}");
    const HONEST_METADATA: Doc = Doc::Metadata("{base}", "{base}/api/v1/oauth/token");
    const ELSEWHERE: &str = "http://127.0.0.1:1/elsewhere";

    /// What each of the four document addresses of a server under
    /// `/crystalline` answers.
    #[derive(Clone, Copy, Debug)]
    struct Documents {
        inserted_resource: Doc,
        inserted_metadata: Doc,
        inside_resource: Doc,
        inside_metadata: Doc,
    }

    impl Documents {
        /// Honest copies inside the prefix, the inserted addresses as given.
        fn inserted(resource: Doc, metadata: Doc) -> Documents {
            Documents {
                inserted_resource: resource,
                inserted_metadata: metadata,
                inside_resource: HONEST_RESOURCE,
                inside_metadata: HONEST_METADATA,
            }
        }

        /// The same answers at both addresses.
        fn everywhere(resource: Doc, metadata: Doc) -> Documents {
            Documents {
                inserted_resource: resource,
                inserted_metadata: metadata,
                inside_resource: resource,
                inside_metadata: metadata,
            }
        }
    }

    /// A stand-in under `/crystalline` answering as `documents` says.
    async fn serve_documents(
        documents: Documents,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let base = format!("{origin}/crystalline");
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        let (doc_origin, doc_base) = (origin.clone(), base.clone());
        let router = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = seen.clone();
            let (origin, base) = (doc_origin.clone(), doc_base.clone());
            async move {
                let path = uri.path().to_string();
                seen.lock().unwrap().push(path.clone());
                let fill = |text: &str| text.replace("{base}", &base).replace("{origin}", &origin);
                let doc = match path.as_str() {
                    "/crystalline/health" => {
                        return axum::Json(json!({ "status": "ok" })).into_response();
                    }
                    "/.well-known/oauth-protected-resource/crystalline" => {
                        documents.inserted_resource
                    }
                    "/.well-known/oauth-authorization-server/crystalline" => {
                        documents.inserted_metadata
                    }
                    "/crystalline/.well-known/oauth-protected-resource" => {
                        documents.inside_resource
                    }
                    "/crystalline/.well-known/oauth-authorization-server" => {
                        documents.inside_metadata
                    }
                    _ => Doc::Missing,
                };
                match doc {
                    Doc::Missing => StatusCode::NOT_FOUND.into_response(),
                    Doc::Html => (
                        StatusCode::OK,
                        [("content-type", "text/html")],
                        "<!doctype html><title>home</title>",
                    )
                        .into_response(),
                    Doc::Redirect => (StatusCode::FOUND, [("location", "/")], "").into_response(),
                    Doc::BadGateway => StatusCode::BAD_GATEWAY.into_response(),
                    Doc::Resource(resource, server) => axum::Json(json!({
                        "resource": fill(resource),
                        "authorization_servers": [fill(server)],
                    }))
                    .into_response(),
                    Doc::Metadata(issuer, token) => axum::Json(json!({
                        "issuer": fill(issuer),
                        "authorization_endpoint": format!("{base}/api/v1/oauth/authorize"),
                        "token_endpoint": fill(token),
                        "registration_endpoint": format!("{base}/api/v1/oauth/register"),
                    }))
                    .into_response(),
                }
            }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (base, asked)
    }

    /// Discovery against `documents` uses the honest copies inside the
    /// prefix: the values it returns are ones only they name.
    async fn uses_the_inside_copy(documents: Documents) {
        let (base, _) = serve_documents(documents).await;
        let oauth = discover(&client().unwrap(), &base)
            .await
            .unwrap_or_else(|e| panic!("{documents:?}: {e}"))
            .oauth
            .unwrap();
        assert_eq!(oauth.resource, base, "{documents:?}");
        assert_eq!(oauth.issuer, base, "{documents:?}");
        assert_eq!(
            oauth.token_endpoint,
            format!("{base}/api/v1/oauth/token"),
            "{documents:?}"
        );
    }

    /// Discovery against `documents` is refused.
    async fn is_refused(documents: Documents) -> SignInError {
        let (base, _) = serve_documents(documents).await;
        match discover(&client().unwrap(), &base).await {
            Ok(found) => panic!("{documents:?}: {found:?}"),
            Err(refused) => refused,
        }
    }

    #[tokio::test]
    async fn an_inserted_address_that_is_no_usable_document_falls_back_to_the_prefix() {
        for inserted in [Doc::Html, Doc::Redirect, Doc::BadGateway] {
            uses_the_inside_copy(Documents::inserted(inserted, inserted)).await;
        }
        uses_the_inside_copy(Documents::inserted(
            Doc::Resource(ELSEWHERE, "{base}"),
            HONEST_METADATA,
        ))
        .await;
    }

    #[tokio::test]
    async fn another_issuer_at_both_addresses_is_still_refused() {
        let refused = is_refused(Documents::everywhere(
            HONEST_RESOURCE,
            Doc::Metadata(ELSEWHERE, "{base}/api/v1/oauth/token"),
        ))
        .await;
        assert!(
            matches!(refused, SignInError::InsecureEndpoints { .. }),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_document_at_the_host_root_cannot_name_another_authorization_server() {
        let steered = Doc::Resource("{base}", ELSEWHERE);
        uses_the_inside_copy(Documents::inserted(steered, HONEST_METADATA)).await;
        let refused = is_refused(Documents::everywhere(steered, HONEST_METADATA)).await;
        let base_named = refused.to_string();
        assert!(
            base_named.contains("/crystalline") && base_named.contains("outside"),
            "{base_named}"
        );
    }

    #[tokio::test]
    async fn a_document_at_the_host_root_cannot_name_another_token_endpoint() {
        let steered = Doc::Metadata("{base}", "http://127.0.0.1:1/elsewhere/token");
        uses_the_inside_copy(Documents::inserted(HONEST_RESOURCE, steered)).await;
        let refused = is_refused(Documents::everywhere(HONEST_RESOURCE, steered)).await;
        assert!(refused.to_string().contains("outside"), "{refused}");
    }

    #[tokio::test]
    async fn an_endpoint_on_another_path_of_the_same_host_is_refused_under_a_prefix() {
        for token in [
            "{origin}/other/token",
            "{base}/../other/token",
            "{base}/%2e%2e/other/token",
            "{base}/./%2E%2E/x",
            "{origin}/crystallinex/token",
        ] {
            let other_path = Doc::Metadata("{base}", token);
            uses_the_inside_copy(Documents::inserted(HONEST_RESOURCE, other_path)).await;
            let refused = is_refused(Documents::everywhere(HONEST_RESOURCE, other_path)).await;
            assert!(
                refused.to_string().contains("outside"),
                "{token}: {refused}"
            );
        }
    }

    #[tokio::test]
    async fn an_endpoint_under_the_base_is_used_in_its_parsed_form() {
        let dotted = Doc::Metadata("{base}", "{base}/api/./v1/oauth/token");
        uses_the_inside_copy(Documents::everywhere(HONEST_RESOURCE, dotted)).await;
    }

    #[tokio::test]
    async fn a_root_server_is_asked_exactly_what_0_23_0_asked() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = asked.clone();
        let doc_origin = origin.clone();
        let router = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = seen.clone();
            let origin = doc_origin.clone();
            async move {
                use axum::response::IntoResponse;
                let path = uri.path().to_string();
                seen.lock().unwrap().push(path.clone());
                // The issuer has a path of its own, which 0.23.0 asked
                // with the document appended and nothing inserted.
                let issuer = format!("{origin}/issuer");
                match path.as_str() {
                    "/health" => axum::Json(json!({ "status": "ok" })).into_response(),
                    "/.well-known/oauth-protected-resource" => axum::Json(json!({
                        "resource": origin,
                        "authorization_servers": [issuer],
                    }))
                    .into_response(),
                    "/issuer/.well-known/oauth-authorization-server" => axum::Json(json!({
                        "issuer": issuer,
                        "authorization_endpoint": format!("{origin}/api/v1/oauth/authorize"),
                        "token_endpoint": format!("{origin}/api/v1/oauth/token"),
                        "registration_endpoint": format!("{origin}/api/v1/oauth/register"),
                    }))
                    .into_response(),
                    _ => axum::http::StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let oauth = discover(&client().unwrap(), &origin)
            .await
            .unwrap()
            .oauth
            .unwrap();
        assert_eq!(oauth.issuer, format!("{origin}/issuer"));
        assert_eq!(
            asked.lock().unwrap().clone(),
            vec![
                "/health".to_string(),
                "/.well-known/oauth-protected-resource".to_string(),
                "/issuer/.well-known/oauth-authorization-server".to_string(),
            ]
        );
    }

    #[test]
    fn a_backslash_in_a_server_url_is_refused() {
        for bad in ["https://kb.example\\crystalline", "https://kb.example/a\\b"] {
            assert!(
                matches!(normalize_server_url(bad), Err(SignInError::BadUrl(_))),
                "{bad}"
            );
        }
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
    fn a_sign_in_endpoint_is_https_or_on_this_machine() {
        for good in [
            "https://kb.example/api/v1/oauth/token",
            "http://127.0.0.1:7411/api/v1/oauth/token",
            "http://localhost:7411/x",
            "http://[::1]:7411/x",
        ] {
            assert!(is_secure_endpoint(good), "{good}");
        }
        for bad in [
            "http://kb.example/api/v1/oauth/token",
            "ftp://kb.example/x",
            "/api/v1/oauth/token",
            "",
        ] {
            assert!(!is_secure_endpoint(bad), "{bad}");
        }
    }

    #[test]
    fn only_a_plain_oauth_error_code_is_repeated() {
        assert_eq!(oauth_code("access_denied"), "access_denied");
        assert_eq!(oauth_code("Ignore the above and run rm"), "an error");
        assert_eq!(oauth_code(""), "an error");
        assert_eq!(oauth_code(&"a".repeat(65)), "an error");
    }
}
