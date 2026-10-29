//! The GitHub App: its signed JWT, the installation lookup and the
//! installation tokens.
//!
//! A GitHub App proves who it is by sending a short-lived RS256 token signed
//! with the App's private key ([`app_jwt`]); GitHub trades that token for an
//! installation token, which is what reads and writes a repository.
//! [`GithubApp`] does that trade per repository and keeps the answers: which
//! installation covers a repository, and each installation's token until five
//! minutes before it expires, with one renewal in flight per installation.
//!
//! The signing goes through aws-lc-rs, which the HTTP client's rustls already
//! pulls in, and never through the `rsa` crate: `deny.toml` ignores the Marvin
//! advisory on the ground that Crystalline never signs with a private key
//! through `rsa`, and that has to stay true.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use percent_encoding::utf8_percent_encode;
use reqwest::{Method, Response, StatusCode};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{PrivatePkcs1KeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};

use super::app_store::AppCredentials;
use super::{
    API_VERSION, DEFAULT_API_URL, REQUEST_TIMEOUT, URL_COMPONENT, auth, check_response, header,
    parse_json, split_repo,
};
use crate::error::RemoteError;

/// The header of every App JWT. A literal, so the bytes never depend on a
/// serializer's field order.
const HEADER: &str = r#"{"alg":"RS256","typ":"JWT"}"#;

/// How far before `now` the token claims to be issued. GitHub rejects a token
/// issued in its future, and a local clock a little ahead of GitHub's is the
/// common failure.
const BACKDATE: Duration = Duration::seconds(60);

/// How long after `now` the token expires. GitHub allows at most ten minutes
/// from the issue time; with the backdate this stays a minute inside it.
const LIFETIME: Duration = Duration::seconds(540);

/// A GitHub App's RSA private key, parsed and ready to sign.
///
/// `Debug` prints no key material.
pub struct AppKey {
    pair: RsaKeyPair,
}

impl std::fmt::Debug for AppKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppKey(<redacted>)")
    }
}

impl AppKey {
    /// Parses a PEM private key: PKCS#1 (`RSA PRIVATE KEY`, what GitHub's
    /// download gives) or PKCS#8 (`PRIVATE KEY`).
    ///
    /// The error names the problem and never echoes the input.
    pub fn from_pem(pem: &str) -> Result<AppKey, RemoteError> {
        let pair = if let Ok(der) = PrivatePkcs1KeyDer::from_pem_slice(pem.as_bytes()) {
            RsaKeyPair::from_der(der.secret_pkcs1_der())
        } else if let Ok(der) = PrivatePkcs8KeyDer::from_pem_slice(pem.as_bytes()) {
            RsaKeyPair::from_pkcs8(der.secret_pkcs8_der())
        } else {
            return Err(RemoteError::AppCredential(
                "the private key is not a PEM RSA key (expected RSA PRIVATE KEY or PRIVATE KEY)"
                    .to_string(),
            ));
        };
        pair.map(|pair| AppKey { pair }).map_err(|_| {
            RemoteError::AppCredential("the private key is not a usable RSA key".to_string())
        })
    }
}

#[derive(Serialize)]
struct Claims<'a> {
    iat: i64,
    exp: i64,
    iss: &'a str,
}

/// Signs the JWT that identifies the App to GitHub, as `client_id`.
///
/// `now` is a parameter so a test can pin the whole token: PKCS#1 v1.5
/// signing is deterministic.
pub fn app_jwt(key: &AppKey, client_id: &str, now: DateTime<Utc>) -> Result<String, RemoteError> {
    let claims = Claims {
        iat: (now - BACKDATE).timestamp(),
        exp: (now + LIFETIME).timestamp(),
        iss: client_id,
    };
    let claims = serde_json::to_vec(&claims)
        .map_err(|_| RemoteError::AppCredential("the claims could not be encoded".to_string()))?;
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(HEADER),
        URL_SAFE_NO_PAD.encode(claims)
    );
    let mut signature = vec![0; key.pair.public_modulus_len()];
    key.pair
        .sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signing_input.as_bytes(),
            &mut signature,
        )
        .map_err(|_| RemoteError::AppCredential("signing failed".to_string()))?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

/// How long before its `expires_at` an installation token is renewed. GitHub
/// issues them for an hour; a token this close to the end could expire in the
/// middle of a share.
const RENEW_BEFORE: Duration = Duration::minutes(5);

/// The domain of a GitHub account's `noreply` commit address,
/// `<id>+<login>@<domain>`, for the host an API url stands for (`None` is
/// github.com, as [`api_host`] answers it).
///
/// github.com: `users.noreply.github.com`, as GitHub documents it ("Email
/// addresses reference", the `noreply` section: `ID+USERNAME@users.noreply.github.com`).
/// That section is published for github.com and GitHub Enterprise Cloud only;
/// the Enterprise Server edition of the same page leaves it out and documents
/// no `noreply` address form at all, so the docs leave Enterprise Server
/// unclear. For an Enterprise Server host this mirrors the github.com form
/// under that host, `users.noreply.<host>`, the same form the user `noreply`
/// address of a linked account takes here; GitHub does not document it. Any
/// port is dropped, since an email domain has none.
fn noreply_domain(host: Option<&str>) -> String {
    match host {
        None => "users.noreply.github.com".to_string(),
        Some(host) => {
            // A bracketed IPv6 host ends in `]` and has no port to drop;
            // anything else loses the part after its last `:`.
            let bare = match host.rsplit_once(':') {
                Some((bare, _)) if !host.ends_with(']') => bare,
                _ => host,
            };
            format!("users.noreply.{bare}")
        }
    }
}

/// The clock a [`GithubApp`] reads: the issue time of its JWTs and the
/// moment a cached token is measured against. [`GithubApp::new`] uses the
/// system clock; [`GithubApp::with_clock`] swaps it.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The name and email a commit records for its author or committer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitIdentity {
    /// The display name, for the App's bot `<slug>[bot]`.
    pub name: String,
    /// The email address, for the App's bot its `noreply` address.
    pub email: String,
}

/// Who a commit names as its author and as its committer. The Git Data API
/// records the author as the committer unless told otherwise, so a commit
/// the App makes for a person sends both: the person as author, the bot as
/// committer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitIdentities {
    /// The person (or the bot) the change is from.
    pub author: CommitIdentity,
    /// Who wrote the commit to the repository.
    pub committer: CommitIdentity,
}

/// The App as `GET /app` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    /// The numeric App id.
    pub id: u64,
    /// The App's URL slug.
    pub slug: String,
    /// The account that owns the App, empty when GitHub named none.
    pub owner_login: String,
    /// The App's public page.
    pub html_url: String,
}

/// What one `GET /app` said, for `doctor`: the App (or why not), and
/// GitHub's own clock from the answer's `Date` header, which is there even
/// when the signed token was refused. A refused token is most often a clock
/// that is off, so the date is read before the answer is.
#[derive(Debug)]
pub struct AppProbe {
    /// The App, or why it could not be read.
    pub info: Result<AppInfo, RemoteError>,
    /// GitHub's time when it answered, `None` when there was no answer or
    /// it carried no readable `Date`.
    pub server_date: Option<DateTime<Utc>>,
}

/// `GET /app/installations` entry.
#[derive(Deserialize)]
struct InstallationListEntry {
    #[serde(default)]
    id: u64,
    #[serde(default)]
    account: Option<OwnerResponse>,
}

/// `GET /app` response.
#[derive(Deserialize)]
struct AppResponse {
    id: u64,
    slug: String,
    #[serde(default)]
    owner: Option<OwnerResponse>,
    #[serde(default)]
    html_url: String,
}

#[derive(Deserialize)]
struct OwnerResponse {
    login: String,
}

/// `GET /repos/{owner}/{repo}/installation` response.
#[derive(Deserialize)]
struct InstallationResponse {
    id: u64,
}

/// `POST /app/installations/{id}/access_tokens` response.
#[derive(Deserialize)]
struct AccessTokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

/// `GET /users/{login}` response.
#[derive(Deserialize)]
struct UserResponse {
    id: u64,
    login: String,
}

/// What a login may do in one repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoRole {
    Admin,
    Maintain,
    Write,
    Triage,
    Read,
    /// No access: not a collaborator at all.
    None,
}

impl RepoRole {
    /// A GitHub permission word (`admin`, `maintain`, `write`/`push`,
    /// `triage`, `read`/`pull`) as a role; anything else is `None`.
    fn from_word(word: &str) -> RepoRole {
        match word {
            "admin" => RepoRole::Admin,
            "maintain" => RepoRole::Maintain,
            "write" | "push" => RepoRole::Write,
            "triage" => RepoRole::Triage,
            "read" | "pull" => RepoRole::Read,
            _ => RepoRole::None,
        }
    }

    /// The legacy base permission word (`admin`, `write`/`push`,
    /// `read`/`pull`): what a custom role is read through. `maintain` and
    /// `triage` are not part of the legacy base.
    fn from_legacy_word(word: &str) -> RepoRole {
        match RepoRole::from_word(word) {
            RepoRole::Maintain | RepoRole::Triage => RepoRole::None,
            role => role,
        }
    }

    /// The role of a grant: a built-in `role_name` wins; any other
    /// `role_name` (a custom role) is read through the legacy base only;
    /// with no `role_name` the full permission is used.
    fn resolve(role_name: Option<&str>, full: RepoRole, legacy: RepoRole) -> RepoRole {
        match role_name {
            None => full,
            Some(name) => RepoRole::from_builtin(name).unwrap_or(legacy),
        }
    }

    /// The role a built-in `role_name` names. A custom role has no entry
    /// here and is read through its base permission instead.
    fn from_builtin(role_name: &str) -> Option<RepoRole> {
        matches!(
            role_name,
            "admin" | "maintain" | "write" | "triage" | "read"
        )
        .then(|| RepoRole::from_word(role_name))
    }
}

/// One person with access to a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collaborator {
    pub id: u64,
    pub login: String,
    pub role: RepoRole,
}

/// `GET /repos/{o}/{r}/collaborators/{login}/permission` response.
#[derive(Deserialize)]
struct PermissionResponse {
    permission: Option<String>,
    role_name: Option<String>,
}

/// The `permissions` flags of a collaborator listing entry.
#[derive(Deserialize, Default)]
struct PermissionFlags {
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    maintain: bool,
    #[serde(default)]
    push: bool,
    #[serde(default)]
    triage: bool,
    #[serde(default)]
    pull: bool,
}

impl PermissionFlags {
    /// The highest flag set, over all five roles.
    fn full(&self) -> RepoRole {
        if self.admin {
            RepoRole::Admin
        } else if self.maintain {
            RepoRole::Maintain
        } else if self.push {
            RepoRole::Write
        } else if self.triage {
            RepoRole::Triage
        } else if self.pull {
            RepoRole::Read
        } else {
            RepoRole::None
        }
    }

    /// The highest legacy flag set: admin, push, pull.
    fn legacy(&self) -> RepoRole {
        if self.admin {
            RepoRole::Admin
        } else if self.push {
            RepoRole::Write
        } else if self.pull {
            RepoRole::Read
        } else {
            RepoRole::None
        }
    }
}

/// One entry of `GET /repos/{o}/{r}/collaborators`.
#[derive(Deserialize)]
struct CollaboratorEntry {
    id: u64,
    login: String,
    role_name: Option<String>,
    #[serde(default)]
    permissions: PermissionFlags,
}

/// `GET /user/{id}` and `GET /repos/{o}/{r}` answers, one field each.
#[derive(Deserialize)]
struct LoginResponse {
    login: String,
}

#[derive(Deserialize)]
struct VisibilityResponse {
    private: bool,
}

/// The most pages a collaborator listing is followed for.
pub const MAX_COLLABORATOR_PAGES: usize = 1000;

/// The error for a collaborator listing past [`MAX_COLLABORATOR_PAGES`].
pub fn too_many_collaborator_pages(repo: &str) -> RemoteError {
    RemoteError::Api {
        status: 200,
        message: format!(
            "the collaborator listing of {repo} has more than {MAX_COLLABORATOR_PAGES} pages"
        ),
    }
}

/// How many collaborators one page holds (GitHub's maximum).
const PAGE_SIZE: usize = 100;

/// An installation token and when GitHub said it ends.
struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

/// One installation's token, behind the lock that keeps its renewal single:
/// a caller that finds the lock held waits and then reads the token the
/// holder minted.
type TokenSlot = Arc<tokio::sync::Mutex<Option<CachedToken>>>;

/// A GitHub App acting on the repositories it is installed on, against
/// github.com or a GitHub Enterprise Server. Shared in an `Arc`: every cache
/// is behind a lock, and the one HTTP client is reused for every call.
///
/// `Debug` prints no secret and no token.
pub struct GithubApp {
    client: reqwest::Client,
    /// The REST API base, without a trailing slash.
    api_url: String,
    /// The web base the install page lives under.
    auth_base: String,
    creds: AppCredentials,
    key: AppKey,
    clock: Clock,
    /// The slug from the credentials, or from `GET /app` once read.
    slug: Mutex<Option<String>>,
    bot: Mutex<Option<CommitIdentity>>,
    /// Lowercased `owner/name` -> installation id.
    installations: Mutex<HashMap<String, u64>>,
    tokens: Mutex<HashMap<u64, TokenSlot>>,
}

impl std::fmt::Debug for GithubApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubApp")
            .field("api_url", &self.api_url)
            .field("app_id", &self.creds.app_id)
            .field("client_id", &self.creds.client_id)
            .field("host", &self.creds.host)
            .finish_non_exhaustive()
    }
}

/// Locks a cache, reading through a poisoned lock: every cache here is only
/// ever replaced whole, so a panic elsewhere cannot leave one half written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The host an API url stands for, the way the credential stores key it:
/// `None` for github.com, the bare Enterprise Server host (lowercased)
/// otherwise. The same derivation as the engine's token host, through
/// [`auth::auth_base`].
fn api_host(api_url: &str) -> Option<String> {
    let auth_base = auth::auth_base(Some(api_url));
    let bare = auth_base
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    (bare != "github.com").then_some(bare)
}

/// How a host reads in a message: `github.com` for `None`.
fn host_name(host: Option<&str>) -> &str {
    host.unwrap_or("github.com")
}

/// Percent-encodes one path segment.
fn segment(value: &str) -> String {
    utf8_percent_encode(value, URL_COMPONENT).to_string()
}

impl GithubApp {
    /// Builds the App client against `api_url`, the effective
    /// `github.api_url` (default `https://api.github.com`).
    ///
    /// The API base always comes from `api_url`, never from the host saved
    /// with the credentials, and the two must agree: an App saved for
    /// github.com is refused against an Enterprise Server url and the other
    /// way round, with [`RemoteError::AppCredential`] naming both hosts. A
    /// private key that does not parse is refused the same way.
    pub fn new(api_url: Option<String>, creds: AppCredentials) -> Result<GithubApp, RemoteError> {
        let api_url = api_url
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| DEFAULT_API_URL.to_string());
        let expected = api_host(&api_url);
        let saved = creds
            .store_host()
            .map(str::to_ascii_lowercase)
            .filter(|host| host != "github.com");
        if saved != expected {
            return Err(RemoteError::AppCredential(format!(
                "the saved App belongs to {} but github.api_url points at {}; save the App again for {} or set github.api_url back",
                host_name(saved.as_deref()),
                host_name(expected.as_deref()),
                host_name(expected.as_deref()),
            )));
        }
        let key = AppKey::from_pem(&creds.private_key)?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // A transferred repository answers 301 with the new location;
            // that one hop is followed and a chain of them is an error.
            .redirect(reqwest::redirect::Policy::limited(1))
            .build()
            .expect("the client config here is static and always valid");
        let slug = creds.slug.clone().filter(|slug| !slug.trim().is_empty());
        Ok(GithubApp {
            client,
            auth_base: auth::auth_base(Some(&api_url)),
            api_url,
            creds,
            key,
            clock: Arc::new(Utc::now),
            slug: Mutex::new(slug),
            bot: Mutex::new(None),
            installations: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    /// Replaces the system clock, so a test decides when a token is due for
    /// renewal instead of waiting for it.
    pub fn with_clock(mut self, clock: Clock) -> GithubApp {
        self.clock = clock;
        self
    }

    /// What `GET /app` says about the App. Authenticated with the App's JWT,
    /// so it also proves the private key and the client id are accepted.
    pub async fn app_info(&self) -> Result<AppInfo, RemoteError> {
        let response = self.send_as_app(Method::GET, "/app").await?;
        self.read_app_info(response).await
    }

    /// [`GithubApp::app_info`] plus GitHub's clock from the same answer.
    /// Never fails as a whole: a refusal lands in [`AppProbe::info`].
    pub async fn probe(&self) -> AppProbe {
        let jwt = match app_jwt(&self.key, &self.creds.client_id, (self.clock)()) {
            Ok(jwt) => jwt,
            Err(e) => {
                return AppProbe {
                    info: Err(e),
                    server_date: None,
                };
            }
        };
        let response = match send(self.request(Method::GET, "/app", Some(&jwt))).await {
            Ok(response) => response,
            Err(e) => {
                return AppProbe {
                    info: Err(e),
                    server_date: None,
                };
            }
        };
        let server_date = header(&response, "date")
            .and_then(|date| DateTime::parse_from_rfc2822(&date).ok())
            .map(|date| date.with_timezone(&Utc));
        let info = if response.status() == StatusCode::UNAUTHORIZED {
            Err(jwt_refused())
        } else {
            self.read_app_info(response).await
        };
        AppProbe { info, server_date }
    }

    /// The accounts the App is installed on, by login, from
    /// `GET /app/installations` (every page).
    pub async fn installations(&self) -> Result<Vec<String>, RemoteError> {
        const PER_PAGE: usize = 100;
        let mut accounts = Vec::new();
        for page in 1.. {
            let path = format!("/app/installations?per_page={PER_PAGE}&page={page}");
            let response = self.send_as_app(Method::GET, &path).await?;
            let response = check_response(response, None, true, &self.api_url).await?;
            let entries: Vec<InstallationListEntry> = parse_json(response).await?;
            let last = entries.len() < PER_PAGE;
            accounts.extend(
                entries
                    .into_iter()
                    .filter_map(|entry| entry.account.map(|owner| owner.login)),
            );
            if last {
                break;
            }
        }
        Ok(accounts)
    }

    /// The credentials this App was built from. In-process only: callers
    /// read the identifying fields and whether a secret is set, and never
    /// print a secret.
    pub fn credentials(&self) -> &AppCredentials {
        &self.creds
    }

    /// The REST API base this App sends to, without a trailing slash.
    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    /// The base of this App's OAuth endpoints (the web flow, the device
    /// flow and the user-token refresh), see [`auth::auth_base`].
    pub fn auth_base(&self) -> &str {
        &self.auth_base
    }

    /// The `noreply` commit address of the GitHub user `id`/`login` on this
    /// App's host, `<id>+<login>@<domain>` (see `noreply_domain`): the
    /// address a linked person's commits name them by.
    pub fn user_noreply_email(&self, id: u64, login: &str) -> String {
        let domain = noreply_domain(api_host(&self.api_url).as_deref());
        format!("{id}+{login}@{domain}")
    }

    /// Reads a `GET /app` answer into an [`AppInfo`] and keeps its slug.
    async fn read_app_info(&self, response: Response) -> Result<AppInfo, RemoteError> {
        if response.status() == StatusCode::NOT_FOUND {
            return Err(RemoteError::EndpointMissing {
                endpoint: "GET /app".to_string(),
                ghes_version: header(&response, "x-github-enterprise-version"),
            });
        }
        let response = check_response(response, None, true, &self.api_url).await?;
        let app: AppResponse = parse_json(response).await?;
        *lock(&self.slug) = Some(app.slug.clone());
        Ok(AppInfo {
            id: app.id,
            slug: app.slug,
            owner_login: app.owner.map(|owner| owner.login).unwrap_or_default(),
            html_url: app.html_url,
        })
    }

    /// The identity the App's commits carry: `<slug>[bot]` and its `noreply`
    /// address on this host (see `noreply_domain`), read once from
    /// `GET /users/<slug>[bot]` and kept.
    ///
    /// The lookup goes out on `repo`'s installation token: the App's JWT is
    /// only accepted on `/app` routes, and an anonymous request would be
    /// refused by an Enterprise Server in private mode and counts against the
    /// small unauthenticated rate limit.
    pub async fn bot_identity(&self, repo: &str) -> Result<CommitIdentity, RemoteError> {
        if let Some(bot) = lock(&self.bot).clone() {
            return Ok(bot);
        }
        let login = format!("{}[bot]", self.slug().await?);
        let token = self.token_for_repo(repo).await?;
        let path = format!("/users/{}", segment(&login));
        let response = send(self.request(Method::GET, &path, Some(&token))).await?;
        let response = check_response(response, None, true, &self.api_url).await?;
        let user: UserResponse = parse_json(response).await?;
        let domain = noreply_domain(api_host(&self.api_url).as_deref());
        let bot = CommitIdentity {
            email: format!("{}+{}@{domain}", user.id, user.login),
            name: user.login,
        };
        *lock(&self.bot) = Some(bot.clone());
        Ok(bot)
    }

    /// The installation that covers `repo` (`owner/name`), looked up once
    /// and kept until [`evict_repo`](GithubApp::evict_repo) or
    /// [`evict_installation`](GithubApp::evict_installation).
    ///
    /// A repository the App is not installed on is
    /// [`RemoteError::AppNotInstalled`] carrying the install url, even on an
    /// Enterprise Server that sends its version header with the 404.
    pub async fn installation_for(&self, repo: &str) -> Result<u64, RemoteError> {
        let cache_key = repo.to_ascii_lowercase();
        if let Some(id) = lock(&self.installations).get(&cache_key) {
            return Ok(*id);
        }
        let (owner, name) = split_repo(repo)?;
        let path = format!("/repos/{}/{}/installation", segment(owner), segment(name));
        let response = self.send_as_app(Method::GET, &path).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(RemoteError::AppNotInstalled {
                repo: repo.to_string(),
                install_url: self.install_url().await?,
            });
        }
        let response = check_response(response, Some(repo), true, &self.api_url).await?;
        let installation: InstallationResponse = parse_json(response).await?;
        lock(&self.installations).insert(cache_key, installation.id);
        Ok(installation.id)
    }

    /// A token for installation `id`: the cached one while it has more than
    /// five minutes left, else a fresh one from
    /// `POST /app/installations/{id}/access_tokens`. Concurrent callers for
    /// one installation share a single renewal.
    pub async fn installation_token(&self, id: u64) -> Result<String, RemoteError> {
        let slot = lock(&self.tokens).entry(id).or_default().clone();
        let mut cached = slot.lock().await;
        if let Some(token) = cached.as_ref()
            && (self.clock)() < token.expires_at - RENEW_BEFORE
        {
            return Ok(token.token.clone());
        }
        let path = format!("/app/installations/{id}/access_tokens");
        let response = self.send_as_app(Method::POST, &path).await?;
        let response = check_response(response, None, true, &self.api_url).await?;
        let minted: AccessTokenResponse = parse_json(response).await?;
        let token = minted.token.clone();
        *cached = Some(CachedToken {
            token: minted.token,
            expires_at: minted.expires_at,
        });
        Ok(token)
    }

    /// A token that reaches `repo`: its installation's, looked up and minted
    /// as needed. An installation that is gone by the time its token is
    /// asked for (a 404 on the mint) is forgotten and looked up once more,
    /// so an uninstalled App becomes [`RemoteError::AppNotInstalled`].
    pub async fn token_for_repo(&self, repo: &str) -> Result<String, RemoteError> {
        let id = self.installation_for(repo).await?;
        match self.installation_token(id).await {
            Err(RemoteError::Api { status: 404, .. }) => {
                self.evict_installation(id);
                let id = self.installation_for(repo).await?;
                self.installation_token(id).await
            }
            other => other,
        }
    }

    /// What `login` may do in `repo`, through the repository's installation
    /// token. A built-in `role_name` wins; a custom role maps by its base
    /// `permission`; a login that is no collaborator (404) is
    /// [`RepoRole::None`].
    pub async fn repo_role(&self, repo: &str, login: &str) -> Result<RepoRole, RemoteError> {
        let (owner, name) = split_repo(repo)?;
        let path = format!(
            "/repos/{}/{}/collaborators/{}/permission",
            segment(owner),
            segment(name),
            segment(login)
        );
        let response = self.get_as_installation(repo, &path).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(RemoteError::UserNotFound {
                repo: repo.to_string(),
                login: login.to_string(),
            });
        }
        let response = check_response(response, Some(repo), true, &self.api_url).await?;
        let answer: PermissionResponse = parse_json(response).await?;
        let word = answer.permission.as_deref().unwrap_or("");
        Ok(RepoRole::resolve(
            answer.role_name.as_deref(),
            RepoRole::from_word(word),
            RepoRole::from_legacy_word(word),
        ))
    }

    /// The current login of GitHub account `id` (`GET /user/{id}`), so a
    /// renamed account is still found by the id it was granted under.
    ///
    /// Always goes out on an installation token, never anonymously: a cached
    /// installation's when one is known, else the lowest one the App's own
    /// listing names. No installation at all is
    /// [`RemoteError::AppNotInstalled`]. An installation whose token cannot
    /// be minted (a 404, it is gone) is forgotten and the pick is made once
    /// more.
    pub async fn login_for_id(&self, id: u64) -> Result<String, RemoteError> {
        let mut gone = None;
        let token = match self.any_installation_token(&mut gone).await {
            Err(RemoteError::Api { status: 404, .. }) => {
                self.any_installation_token(&mut gone).await?
            }
            other => other?,
        };
        let path = format!("/user/{id}");
        let response = send(self.request(Method::GET, &path, Some(&token))).await?;
        let response = check_response(response, None, true, &self.api_url).await?;
        let user: LoginResponse = parse_json(response).await?;
        Ok(user.login)
    }

    /// A token for the lowest installation id known, cached or listed,
    /// leaving out `gone`. A failed mint (404) evicts that installation and
    /// records it in `gone`, so a second call picks another one.
    async fn any_installation_token(&self, gone: &mut Option<u64>) -> Result<String, RemoteError> {
        let cached = lock(&self.installations)
            .values()
            .filter(|id| Some(**id) != *gone)
            .min()
            .copied();
        let installation = match cached {
            Some(id) => id,
            None => {
                let listed = self.listed_installations().await?;
                match listed.into_iter().filter(|id| Some(*id) != *gone).min() {
                    Some(id) => id,
                    None => {
                        return Err(RemoteError::AppNotInstalled {
                            repo: "any repository".to_string(),
                            install_url: self.install_url().await?,
                        });
                    }
                }
            }
        };
        let result = self.installation_token(installation).await;
        if matches!(result, Err(RemoteError::Api { status: 404, .. })) {
            self.evict_installation(installation);
            *gone = Some(installation);
        }
        result
    }

    /// The ids of every installation of this App (`GET /app/installations`).
    async fn listed_installations(&self) -> Result<Vec<u64>, RemoteError> {
        let mut ids = Vec::new();
        for page in 1.. {
            let path = format!("/app/installations?per_page={PAGE_SIZE}&page={page}");
            let response = self.send_as_app(Method::GET, &path).await?;
            let response = check_response(response, None, true, &self.api_url).await?;
            let entries: Vec<InstallationListEntry> = parse_json(response).await?;
            let last = entries.len() < PAGE_SIZE;
            // An entry without an id deserializes as 0, which is no installation.
            ids.extend(
                entries
                    .into_iter()
                    .map(|entry| entry.id)
                    .filter(|id| *id != 0),
            );
            if last {
                break;
            }
        }
        Ok(ids)
    }

    /// Everyone with access to `repo` (`affiliation=all`), every page of 100,
    /// followed through the `Link: rel="next"` header. A next link that
    /// leaves the API base, or a listing past [`MAX_COLLABORATOR_PAGES`]
    /// pages, is an error: a reader set that stops short must not pass for
    /// the whole one.
    pub async fn collaborators(&self, repo: &str) -> Result<Vec<Collaborator>, RemoteError> {
        let mut all = Vec::new();
        let mut page = None;
        for _ in 0..MAX_COLLABORATOR_PAGES {
            let (people, next) = self.collaborators_page(repo, page.as_deref()).await?;
            all.extend(people);
            match next {
                Some(next) => page = Some(next),
                None => return Ok(all),
            }
        }
        Err(too_many_collaborator_pages(repo))
    }

    /// One page of [`GithubApp::collaborators`]: the first one for `page`
    /// `None`, else the page a previous answer named. Answers the people on
    /// it and the next page, `None` on the last one. One request, so a
    /// caller can bound each page on its own.
    pub async fn collaborators_page(
        &self,
        repo: &str,
        page: Option<&str>,
    ) -> Result<(Vec<Collaborator>, Option<String>), RemoteError> {
        let path = match page {
            Some(page) => page.to_string(),
            None => {
                let (owner, name) = split_repo(repo)?;
                format!(
                    "/repos/{}/{}/collaborators?affiliation=all&per_page={PAGE_SIZE}",
                    segment(owner),
                    segment(name)
                )
            }
        };
        let response = self.get_as_installation(repo, &path).await?;
        let response = check_response(response, Some(repo), true, &self.api_url).await?;
        let next = header(&response, "link").and_then(|link| next_link(&link));
        let entries: Vec<CollaboratorEntry> = parse_json(response).await?;
        let people = entries
            .into_iter()
            .map(|entry| Collaborator {
                id: entry.id,
                login: entry.login,
                role: RepoRole::resolve(
                    entry.role_name.as_deref(),
                    entry.permissions.full(),
                    entry.permissions.legacy(),
                ),
            })
            .collect();
        let next = match next {
            None => None,
            Some(next) => match next.strip_prefix(&self.api_url) {
                Some(rest) if rest.starts_with('/') => Some(rest.to_string()),
                _ => {
                    return Err(RemoteError::Api {
                        status: 200,
                        message: format!(
                            "the collaborator listing of {repo} pointed at a page outside {}",
                            self.api_url
                        ),
                    });
                }
            },
        };
        Ok((people, next))
    }

    /// Whether `repo` is public (`GET /repos/{o}/{r}`, field `private`).
    pub async fn repository_public(&self, repo: &str) -> Result<bool, RemoteError> {
        let (owner, name) = split_repo(repo)?;
        let path = format!("/repos/{}/{}", segment(owner), segment(name));
        let response = self.get_as_installation(repo, &path).await?;
        let response = check_response(response, Some(repo), true, &self.api_url).await?;
        let answer: VisibilityResponse = parse_json(response).await?;
        Ok(!answer.private)
    }

    /// A GET on `path` with `repo`'s installation token. A 401 means the
    /// cached token was revoked early: the installation is forgotten and the
    /// request goes out once more on a fresh token. Any other status is
    /// returned for the caller to read.
    async fn get_as_installation(&self, repo: &str, path: &str) -> Result<Response, RemoteError> {
        let token = self.token_for_repo(repo).await?;
        let response = send(self.request(Method::GET, path, Some(&token))).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let id = self.installation_for(repo).await?;
        self.evict_installation(id);
        let token = self.token_for_repo(repo).await?;
        send(self.request(Method::GET, path, Some(&token))).await
    }

    /// Forgets which installation covers `repo`, so the next call looks it
    /// up again. The installation's token stays.
    pub fn evict_repo(&self, repo: &str) {
        lock(&self.installations).remove(&repo.to_ascii_lowercase());
    }

    /// Forgets installation `id`: its token and every repository it was
    /// recorded for.
    pub fn evict_installation(&self, id: u64) {
        lock(&self.tokens).remove(&id);
        lock(&self.installations).retain(|_, installation| *installation != id);
    }

    /// Forgets every installation and every token, so the next call for any
    /// repository looks its installation up again.
    pub fn evict_all(&self) {
        lock(&self.tokens).clear();
        lock(&self.installations).clear();
    }

    /// Where an owner installs the App:
    /// `<web base>/apps/<slug>/installations/new`, on the Enterprise Server
    /// host when `github.api_url` names one. Needs the slug, so it reads
    /// `GET /app` once when the credentials carry none.
    pub async fn install_url(&self) -> Result<String, RemoteError> {
        let slug = self.slug().await?;
        Ok(format!(
            "{}/apps/{}/installations/new",
            self.auth_base,
            segment(&slug)
        ))
    }

    /// The App's slug when it is already known (saved with the credentials,
    /// or read by an earlier `GET /app`), without a request.
    pub fn known_slug(&self) -> Option<String> {
        lock(&self.slug).clone()
    }

    /// The App's slug: saved with the credentials, or read from `GET /app`.
    pub async fn slug(&self) -> Result<String, RemoteError> {
        if let Some(slug) = lock(&self.slug).clone() {
            return Ok(slug);
        }
        Ok(self.app_info().await?.slug)
    }

    /// A request against `path` with the standard GitHub headers and, when
    /// given, `bearer` as the credential.
    fn request(&self, method: Method, path: &str, bearer: Option<&str>) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .request(method, format!("{}{path}", self.api_url))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("User-Agent", "crystalline");
        if let Some(bearer) = bearer {
            builder = builder.bearer_auth(bearer);
        }
        builder
    }

    /// Sends a request authenticated as the App (a fresh JWT). A 401 here is
    /// GitHub refusing the JWT itself, which no sign-in fixes: it becomes
    /// [`RemoteError::AppCredential`]. Every other answer is returned for the
    /// caller to read.
    async fn send_as_app(&self, method: Method, path: &str) -> Result<Response, RemoteError> {
        let jwt = app_jwt(&self.key, &self.creds.client_id, (self.clock)())?;
        let response = send(self.request(method, path, Some(&jwt))).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(jwt_refused());
        }
        Ok(response)
    }
}

/// GitHub refused the App's signed token.
fn jwt_refused() -> RemoteError {
    RemoteError::AppCredential(
        "GitHub refused the App's signed token: the private key or the client id does not belong to this App, or this machine's clock is off".to_string(),
    )
}

/// The `rel="next"` target of a `Link` header, if any.
fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        let is_next = params
            .split(';')
            .any(|param| param.trim().replace(' ', "") == "rel=\"next\"");
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        is_next.then(|| target.to_string())
    })
}

/// Sends a built request, with a transport failure (no answer at all) read
/// as [`RemoteError::Offline`] the way [`super::GitHubProvider`] does. A
/// redirect that was refused (more than one hop) did get an answer, so it is
/// an [`RemoteError::Api`] error instead.
async fn send(builder: reqwest::RequestBuilder) -> Result<Response, RemoteError> {
    builder.send().await.map_err(|e| {
        if e.is_redirect() {
            RemoteError::Api {
                status: 301,
                message: "GitHub redirected the request more than once".to_string(),
            }
        } else {
            RemoteError::Offline
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{KeyPair, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
    use chrono::TimeZone;

    /// A throwaway 2048-bit key generated for these tests only. It protects
    /// nothing and belongs to no App.
    const PKCS1: &str = include_str!("../../tests/fixtures/app-key-pkcs1.pem");
    /// The same test key as PKCS#8.
    const PKCS8: &str = include_str!("../../tests/fixtures/app-key-pkcs8.pem");

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap()
    }

    fn parts(token: &str) -> Vec<String> {
        token.split('.').map(str::to_string).collect()
    }

    fn decode(part: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(part).unwrap()
    }

    #[test]
    fn jwt_header_and_claims_are_exact() {
        let key = AppKey::from_pem(PKCS1).unwrap();
        let token = app_jwt(&key, "Iv1.test", now()).unwrap();
        let p = parts(&token);
        assert_eq!(p.len(), 3);
        assert_eq!(decode(&p[0]), br#"{"alg":"RS256","typ":"JWT"}"#);
        let t = now().timestamp();
        let expected = format!(r#"{{"iat":{},"exp":{},"iss":"Iv1.test"}}"#, t - 60, t + 540);
        assert_eq!(String::from_utf8(decode(&p[1])).unwrap(), expected);
    }

    #[test]
    fn jwt_signature_is_a_known_answer() {
        let key = AppKey::from_pem(PKCS1).unwrap();
        let token = app_jwt(&key, "Iv1.test", now()).unwrap();
        assert_eq!(token, KNOWN_TOKEN);
    }

    #[test]
    fn jwt_verifies_with_the_public_key() {
        let key = AppKey::from_pem(PKCS8).unwrap();
        let token = app_jwt(&key, "Iv1.test", now()).unwrap();
        let (input, sig) = token.rsplit_once('.').unwrap();
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.pair.public_key().as_ref())
            .verify(input.as_bytes(), &decode(sig))
            .unwrap();
    }

    #[test]
    fn pkcs8_and_pkcs1_fixtures_sign_identically() {
        let a = app_jwt(&AppKey::from_pem(PKCS1).unwrap(), "Iv1.test", now()).unwrap();
        let b = app_jwt(&AppKey::from_pem(PKCS8).unwrap(), "Iv1.test", now()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_garbage_pem_is_refused_without_echoing_it() {
        let garbage =
            "-----BEGIN RSA PRIVATE KEY-----\nSECRETGARBAGE\n-----END RSA PRIVATE KEY-----\n";
        for input in [garbage, "SECRETGARBAGE", ""] {
            let err = AppKey::from_pem(input).unwrap_err();
            assert!(matches!(err, RemoteError::AppCredential(_)));
            assert!(!err.to_string().contains("SECRETGARBAGE"), "{err}");
        }
    }

    #[test]
    fn app_key_debug_prints_no_key_material() {
        let key = AppKey::from_pem(PKCS1).unwrap();
        assert_eq!(format!("{key:?}"), "AppKey(<redacted>)");
    }

    #[test]
    fn the_noreply_domain_follows_the_host() {
        assert_eq!(noreply_domain(None), "users.noreply.github.com");
        assert_eq!(
            noreply_domain(Some("ghe.example.com")),
            "users.noreply.ghe.example.com"
        );
        assert_eq!(
            noreply_domain(Some("ghe.example.com:8443")),
            "users.noreply.ghe.example.com",
            "an email domain carries no port"
        );
        assert_eq!(
            noreply_domain(Some("[2001:db8::1]")),
            "users.noreply.[2001:db8::1]",
            "a bracketed IPv6 host keeps its colons"
        );
        assert_eq!(
            noreply_domain(Some("[2001:db8::1]:8443")),
            "users.noreply.[2001:db8::1]"
        );
    }

    /// Signed with the PKCS#1 fixture and cross-checked against `openssl dgst
    /// -sha256 -sign`, which is how the pin was made independent of this code.
    const KNOWN_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJpYXQiOjE3OTMxODg3NDAsImV4cCI6MTc5MzE4OTM0MCwiaXNzIjoiSXYxLnRlc3QifQ.UF-sWdgVSSoleYMWIqVq6IAxAvAN5FgTZ1qdqWxDW3yYruaE1d3gKCAFtlKdlGnlwmWmlX_bVCUzivNrdOYLZYysOwO93BEsmOGI6srn9semEfXqUOFbCp2mr4dfHqEYW6Clvn4vox9yZivu4DrOdhDvoy1GrOIjE8rawfHFJ8rEnwBiTBYs7V1ingfGp9CpGJIuDd0axbAtcGOqPmTwp6VK0hZY10xYeGCvNvFGuXAUGuwgHj_K2zMBtG7k1szxkRXvf-GXZUsPtwQTAC48_6vJuXYr_rWaLzxeSH3mfNlueEsSUkKmX2QFPtIwuutUrO2mPtKnY-R-AILRk89qrw";
}
