//! The GitHub App web authorization-code flow, as the engine drives it: the
//! pending handshakes, the three verbs a browser link is made and spent
//! through, and the refresh of an expiring user token.
//!
//! Available only on an instance that registered a GitHub App of its own
//! (`github.oauth_client_id` plus `github.oauth_client_secret`) and knows its
//! own address (`service.public_url`). With any of the three absent every
//! existing path - the device flow, a pasted token, the environment token -
//! behaves exactly as it did, and the verbs here refuse with the setting to
//! write.
//!
//! The link is minted lazily, for one account, and is spent in two hops:
//!
//! 1. [`Engine::start_github_web_connect`] mints a **nonce** bound to the
//!    account that asked, and answers the address of this instance's connect
//!    route carrying it.
//! 2. The browser arrives there signed in, and
//!    [`Engine::github_web_authorize`] spends the nonce for a redirect to
//!    GitHub: a fresh PKCE pair whose verifier is kept beside a **state**, and
//!    whose challenge goes out in the url.
//! 3. GitHub sends the browser back to the callback with that state and a
//!    code, and [`Engine::complete_github_web_connect`] spends the state,
//!    exchanges the code, validates the token and stores it.
//!
//! Both halves are single use and both are consumed on the way IN, before
//! anything is judged, so a failed attempt burns its own nonce or state
//! instead of leaving one to be retried. Both expire five minutes after they
//! are minted, and the store is swept on every touch.
//!
//! The account binding on the nonce is what stops the account-linking attack
//! the whole handshake exists to prevent: an attacker who gets their own link
//! in front of a victim would otherwise have the victim's browser authorize
//! GitHub into the ATTACKER's Crystalline account. A nonce spent by a
//! different account than the one it was minted for is
//! [`EngineError::Forbidden`], distinct from the plain
//! [`EngineError::Invalid`] an unknown or expired one gets.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use crystalline_remote::github::web::{self, WebTokens};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use super::*;

/// The RFC 3986 unreserved characters, as `crystalline_remote::github::web`
/// encodes with: a base64url secret crosses into a query string unchanged and
/// anything else is percent-encoded.
const URL_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The route a minted link points the browser at. Owned here, above the JSON
/// API that serves it, because the link is built from `service.public_url`
/// here and the two spellings have to be one.
pub const GITHUB_CONNECT_PATH: &str = "/api/v1/auth/github/connect";

/// The route GitHub sends the browser back to. Registered with the GitHub App
/// in exactly this spelling, below `service.public_url`.
pub const GITHUB_CALLBACK_PATH: &str = "/api/v1/auth/github/callback";

/// How long a minted nonce or state stays spendable.
const PENDING_TTL_SECS: i64 = 300;

/// How many handshakes of each kind are kept at once. A browser link is spent
/// within a minute or two in practice, so this is roomy for a team and still
/// a hard ceiling on what an unauthenticated flood could pin in memory.
const MAX_PENDING: usize = 64;

/// How long before an access token actually expires it is refreshed. GitHub
/// App user tokens live eight hours, so a five minute margin covers a clock
/// skew and a slow call without refreshing on every touch.
const REFRESH_SKEW_SECS: i64 = 300;

/// What a caller is handed to put in front of a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConnectStart {
    /// The absolute address of this instance's connect route, carrying the
    /// nonce. Opened in a browser that is signed in as the same account.
    pub url: String,
    /// How long the link stays spendable.
    pub expires_in_secs: u64,
}

/// A minted link waiting for the browser, bound to the account that asked for
/// it.
struct PendingNonce {
    nonce: String,
    account: String,
    expires_at: DateTime<Utc>,
}

/// A redirect to GitHub waiting for the callback, holding the PKCE verifier
/// whose challenge went out in the authorize url.
struct PendingState {
    state: String,
    account: String,
    verifier: String,
    expires_at: DateTime<Utc>,
}

/// The in-flight halves of the web flow. Vectors rather than maps on purpose:
/// every lookup walks the whole (bounded) list and compares in constant time,
/// so neither the time a lookup takes nor the point it stops comparing at
/// leaks anything about the secrets on file.
#[derive(Default)]
pub(super) struct WebConnectPending {
    nonces: Vec<PendingNonce>,
    states: Vec<PendingState>,
}

impl WebConnectPending {
    /// Drops everything that has expired, and then, if the list is still at
    /// its ceiling, the oldest entry: a flood can cost somebody the link they
    /// are slowest to click, never the whole instance's memory.
    fn sweep(&mut self, now: DateTime<Utc>) {
        self.nonces.retain(|n| n.expires_at > now);
        self.states.retain(|s| s.expires_at > now);
        while self.nonces.len() >= MAX_PENDING {
            self.nonces.remove(0);
        }
        while self.states.len() >= MAX_PENDING {
            self.states.remove(0);
        }
    }

    /// Takes the nonce out of the store whatever it turns out to be: single
    /// use means spent on a refusal too, so a guessed account can never be
    /// retried against the same link.
    fn take_nonce(&mut self, nonce: &str, now: DateTime<Utc>) -> Option<PendingNonce> {
        self.nonces.retain(|n| n.expires_at > now);
        let found = self
            .nonces
            .iter()
            .position(|n| constant_time_eq(n.nonce.as_bytes(), nonce.as_bytes()))?;
        Some(self.nonces.remove(found))
    }

    /// [`WebConnectPending::take_nonce`] for the state half.
    fn take_state(&mut self, state: &str, now: DateTime<Utc>) -> Option<PendingState> {
        self.states.retain(|s| s.expires_at > now);
        let found = self
            .states
            .iter()
            .position(|s| constant_time_eq(s.state.as_bytes(), state.as_bytes()))?;
        Some(self.states.remove(found))
    }
}

/// Byte equality that always looks at every byte of `a`, so a comparison
/// against a secret this process holds cannot be walked one byte at a time by
/// timing it. Mirrors the JSON API's own `constant_time_eq`, which sits above
/// this crate and cannot be reached from here.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// A fresh 256 bit secret from the OS CSPRNG, base64url with no padding - the
/// shape every other raw-random site in this workspace mints.
fn secret_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG is available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The registered GitHub App this instance authenticates as. Everything the
/// token and grant endpoints need; the browser half needs
/// [`WebConfig`] on top.
struct GithubApp {
    client_id: String,
    client_secret: String,
    /// `github.api_url`, for a GitHub Enterprise Server host.
    api_url: Option<String>,
}

impl GithubApp {
    /// The host the authorize and token endpoints live on.
    fn auth_base(&self) -> String {
        crystalline_remote::github::auth::auth_base(self.api_url.as_deref())
    }
}

/// The app plus the one thing only the browser half needs: this instance's own
/// address, which the callback GitHub is given is built from.
struct WebConfig {
    app: GithubApp,
    /// `service.public_url`, with no trailing slash.
    public_url: String,
}

impl WebConfig {
    /// The address GitHub sends the browser back to, registered with the app
    /// in this exact spelling.
    fn redirect_uri(&self) -> String {
        format!("{}{GITHUB_CALLBACK_PATH}", self.public_url)
    }
}

impl Engine {
    /// The registered app's credentials, or the one sentence naming the
    /// setting that is missing. Refusals rather than errors: the request is
    /// well formed and the instance simply has not been set up for this, so
    /// what it gets back is the setting to write.
    fn github_app(&self) -> Result<GithubApp> {
        let (client_id, client_secret, api_url) = {
            let config = self.config.read().unwrap();
            let github = config.github.as_ref();
            (
                github.and_then(|g| g.oauth_client_id.clone()),
                github.and_then(|g| g.oauth_client_secret.clone()),
                github.and_then(|g| g.api_url.clone()),
            )
        };
        let Some(client_secret) = client_secret.filter(|s| !s.trim().is_empty()) else {
            return Err(EngineError::Refused(
                "this instance cannot link a GitHub identity through a browser: set \
                 github.oauth_client_secret (or CRYSTALLINE_GITHUB_OAUTH_CLIENT_SECRET) to the \
                 client secret of a registered GitHub App, or connect with a device code instead"
                    .to_string(),
            ));
        };
        // The embedded fallback `oauth_client_id` answers with is a device
        // flow app with no secret and no registered callback, so an instance
        // that set only the secret is not configured for this at all.
        let Some(client_id) = client_id.filter(|s| !s.trim().is_empty()) else {
            return Err(EngineError::Refused(
                "this instance cannot link a GitHub identity through a browser: set \
                 github.oauth_client_id to the client id of the same registered GitHub App the \
                 client secret belongs to"
                    .to_string(),
            ));
        };
        Ok(GithubApp {
            client_id,
            client_secret,
            api_url,
        })
    }

    /// [`Engine::github_app`] as a question rather than a refusal, for the two
    /// paths that do something extra when an app is registered and carry on
    /// unchanged when none is: the refresh and the revoke on disconnect.
    /// Neither builds a browser url, so neither needs `service.public_url`.
    fn github_app_opt(&self) -> Option<GithubApp> {
        self.github_app().ok()
    }

    /// [`Engine::github_app`] plus this instance's own address, for the three
    /// verbs that put a url in front of a browser.
    fn web_config(&self) -> Result<WebConfig> {
        let app = self.github_app()?;
        let Some(public_url) = self
            .config
            .read()
            .unwrap()
            .service_public_url()
            .map(str::to_string)
        else {
            return Err(EngineError::Refused(
                "this instance cannot link a GitHub identity through a browser: set \
                 service.public_url to the address people reach this instance at, so GitHub can \
                 be given a callback address to send the browser back to"
                    .to_string(),
            ));
        };
        Ok(WebConfig {
            app,
            public_url: public_url.trim_end_matches('/').to_string(),
        })
    }

    /// Mints the browser link that connects `account`'s own GitHub identity.
    ///
    /// The nonce it carries is bound to that account and spendable once,
    /// within five minutes, by a browser signed in as the same account: a link
    /// forwarded to somebody else connects nothing, which is the whole point
    /// of minting one per account rather than serving a bare authorize url.
    ///
    /// Refuses on a read-only instance, where the credential could never be
    /// stored, rather than sending somebody through GitHub to find that out.
    pub async fn start_github_web_connect(&self, account: &str) -> Result<WebConnectStart> {
        let identity = personal_identity(account)?;
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let config = self.web_config()?;
        let now = Utc::now();
        let nonce = secret_token();
        {
            let mut pending = self.web_connect.lock().unwrap();
            pending.sweep(now);
            pending.nonces.push(PendingNonce {
                nonce: nonce.clone(),
                account: account.to_string(),
                expires_at: now + Duration::seconds(PENDING_TTL_SECS),
            });
        }
        tracing::info!(
            identity = %identity_label(&identity),
            "github web link minted"
        );
        Ok(WebConnectStart {
            url: format!(
                "{}{GITHUB_CONNECT_PATH}?n={}",
                config.public_url,
                utf8_percent_encode(&nonce, URL_COMPONENT)
            ),
            expires_in_secs: PENDING_TTL_SECS as u64,
        })
    }

    /// Spends a minted nonce for the GitHub authorize url to redirect the
    /// browser to, keeping the PKCE verifier behind a fresh state.
    ///
    /// `account` is the account the BROWSER is signed in as, which the caller
    /// has already authenticated; this call decides whether that account is
    /// the one the link was minted for. A mismatch is
    /// [`EngineError::Forbidden`] and an unknown, expired or already spent
    /// nonce is [`EngineError::Invalid`] - two outcomes rather than one,
    /// because they are two different things for the person to do.
    ///
    /// The nonce is spent either way.
    pub fn github_web_authorize(&self, account: &str, nonce: &str) -> Result<String> {
        let config = self.web_config()?;
        let now = Utc::now();
        let found = {
            let mut pending = self.web_connect.lock().unwrap();
            pending.take_nonce(nonce, now)
        };
        let Some(found) = found else {
            return Err(EngineError::Invalid(
                "this GitHub connection link has expired or has already been used - start the \
                 connection again from your client to get a fresh one"
                    .to_string(),
            ));
        };
        if found.account != account {
            return Err(EngineError::Forbidden(format!(
                "this GitHub connection link was made for another account, not '{}' - start the \
                 connection again from your own client",
                account.escape_debug()
            )));
        }
        let (verifier, challenge) = web::pkce_pair();
        let state = secret_token();
        {
            let mut pending = self.web_connect.lock().unwrap();
            pending.sweep(now);
            pending.states.push(PendingState {
                state: state.clone(),
                account: found.account,
                verifier,
                expires_at: now + Duration::seconds(PENDING_TTL_SECS),
            });
        }
        Ok(web::authorize_url(
            &config.app.auth_base(),
            &config.app.client_id,
            &config.redirect_uri(),
            &state,
            &challenge,
        ))
    }

    /// Completes the flow from the callback: spends the state, exchanges
    /// `code` for the account's tokens, validates them against GitHub and
    /// stores them as that account's personal credential.
    ///
    /// The token is validated (its login read) BEFORE it is stored, exactly as
    /// every other connect path validates, so the login on file is the one the
    /// token actually belongs to rather than whatever the exchange returned.
    /// It is saved through the same [`Engine::github_save_plan_for`] every
    /// other connect saves through, so the keychain or file store and the
    /// credential cache behave as they do today - with the rotating refresh
    /// token and both expiries beside the access token.
    ///
    /// The state is spent either way.
    pub async fn complete_github_web_connect(
        &self,
        state: &str,
        code: &str,
    ) -> Result<GithubIdentity> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let config = self.web_config()?;
        let now = Utc::now();
        let found = {
            let mut pending = self.web_connect.lock().unwrap();
            pending.take_state(state, now)
        };
        let Some(found) = found else {
            return Err(EngineError::Invalid(
                "this GitHub authorization has expired or has already been completed - start the \
                 connection again from your client"
                    .to_string(),
            ));
        };
        let identity = personal_identity(&found.account)?;
        let tokens = web::exchange_code(
            &config.app.auth_base(),
            &config.app.client_id,
            &config.app.client_secret,
            code,
            &config.redirect_uri(),
            &found.verifier,
        )
        .await?;
        let user = self
            .connect_auth
            .validate_token(config.app.api_url.as_deref(), &tokens.access_token)
            .await?;
        let token_host = origin::token_host(config.app.api_url.as_deref());
        let plan = self.github_save_plan_for(&identity, token_host.as_deref())?;
        save_off_runtime(
            plan,
            stored_from(&tokens, token_host.as_deref(), user, Utc::now()),
        )
        .await?;
        tracing::info!(
            identity = %identity_label(&identity),
            "github web link completed"
        );
        // A device flow of this person's still in flight must not land on top
        // of the credential this call just settled.
        self.clear_pending_for(&identity);
        self.github_identity_status(&found.account).await
    }

    /// Refreshes `identity`'s stored credential when it is about to expire,
    /// before anything uses it. A no-op for the instance credential, for a
    /// token with no expiry or no refresh token on file, and on an instance
    /// with no GitHub App configured.
    ///
    /// GitHub rotates the refresh token on every use, so two refreshes of one
    /// credential racing each other would leave the loser holding a refresh
    /// token the winner already spent. Everything past the cheap check runs
    /// under this credential's own async mutex, and the first thing it does
    /// inside the lock is read the credential again: a caller that queued
    /// behind the refresh it was waiting for finds a token that is no longer
    /// due and does nothing at all, so N concurrent callers make exactly one
    /// request to GitHub.
    ///
    /// The rotated pair is persisted before this returns, so the token the
    /// caller then resolves is one that is on disk: a process that died
    /// between the refresh and the save would otherwise have thrown away the
    /// only refresh token the account had.
    ///
    /// A refresh GitHub refuses because the refresh token is spent, expired or
    /// revoked surfaces as [`crystalline_remote::RemoteError::AuthExpired`],
    /// the same "connect again" outcome an expired credential already has.
    pub(super) async fn refresh_personal_credential(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
    ) -> Result<()> {
        if !matches!(identity, TokenIdentity::Personal(_)) {
            return Ok(());
        }
        let skew = Duration::seconds(REFRESH_SKEW_SECS);
        if !self.credential_is_due(identity, host, skew)? {
            return Ok(());
        }
        let Some(app) = self.github_app_opt() else {
            return Ok(());
        };
        let lock = self.github_refresh_lock(&credential_cache_key(identity, host));
        let _guard = lock.lock().await;
        if !self.credential_is_due(identity, host, skew)? {
            return Ok(());
        }
        let (_store, token) = self.github_credential_for(identity, host)?;
        let Some(token) = token else { return Ok(()) };
        let Some(refresh_token) = token.refresh_token.clone() else {
            return Ok(());
        };
        let rotated = match web::refresh(
            &app.auth_base(),
            &app.client_id,
            &app.client_secret,
            &refresh_token,
        )
        .await
        {
            Ok(rotated) => rotated,
            Err(e) => {
                tracing::warn!(
                    identity = %identity_label(identity),
                    error = %e,
                    "github token refresh failed"
                );
                self.drop_github_credential_on_auth(&e);
                return Err(e.into());
            }
        };
        let plan = self.github_save_plan_for(identity, host)?;
        save_off_runtime(
            plan,
            stored_from(&rotated, host, token.user.clone(), token.created_at),
        )
        .await?;
        tracing::info!(
            identity = %identity_label(identity),
            "github token refreshed"
        );
        Ok(())
    }

    /// Whether this credential is on file, expiring within `skew` and
    /// refreshable. The cheap half of [`Engine::refresh_personal_credential`],
    /// asked once before the lock and once inside it.
    fn credential_is_due(
        &self,
        identity: &TokenIdentity,
        host: Option<&str>,
        skew: Duration,
    ) -> Result<bool> {
        let (_store, token) = self.github_credential_for(identity, host)?;
        Ok(token.is_some_and(|t| t.refresh_token.is_some() && t.expires_within(Utc::now(), skew)))
    }

    /// The async mutex serializing refreshes of one credential, created on
    /// first use. Mirrors [`Engine::origin_lock`]; the map only ever grows by
    /// one entry per credential slot this process refreshes.
    fn github_refresh_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.github_refresh_locks
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Asks GitHub to forget the whole grant behind `token`, so a disconnect
    /// here is a disconnect there too. Best effort by construction: the local
    /// credential is being forgotten whatever GitHub answers, and an instance
    /// that is offline, or that has since rotated its app secret, must still
    /// be able to disconnect.
    pub(super) async fn revoke_github_grant(&self, token: &StoredToken) {
        let Some(app) = self.github_app_opt() else {
            return;
        };
        if let Err(e) = web::revoke_grant(
            app.api_url.as_deref(),
            &app.client_id,
            &app.client_secret,
            &token.access_token,
        )
        .await
        {
            tracing::warn!(
                error = %e,
                "the GitHub grant could not be revoked; the stored credential is forgotten anyway"
            );
        }
    }
}

/// The credential one exchange or refresh writes to the store. `created_at` is
/// the caller's to decide: a fresh connect is being created now, while a
/// refresh keeps the moment the person actually connected, which is what the
/// profile card's "connected since" means.
fn stored_from(
    tokens: &WebTokens,
    host: Option<&str>,
    user: String,
    created_at: DateTime<Utc>,
) -> StoredToken {
    StoredToken {
        access_token: tokens.access_token.clone(),
        host: host.unwrap_or("github.com").to_string(),
        user,
        created_at,
        // Whatever GitHub answered, never the token that was just spent: the
        // old one is dead the moment it is used, so an answer carrying none
        // means this credential has stopped being refreshable.
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens.expires_at,
        refresh_token_expires_at: tokens.refresh_token_expires_at,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use axum::Json;
    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{delete, post};
    use crystalline_core::config::{GitHubConfig, ServiceConfig};
    use crystalline_index::TursoStore;
    use tokio::net::TcpListener;

    use super::*;

    const CLIENT_ID: &str = "Iv1.app";
    const CLIENT_SECRET: &str = "s3cret";
    const PUBLIC_URL: &str = "https://kb.example.test";

    /// The throwaway GitHub the engine talks to: one token endpoint and one
    /// grant endpoint, on an ephemeral localhost port, in the style
    /// crates/remote's own web-flow tests use. It records what it was sent so
    /// a test can count the requests the engine actually made.
    #[derive(Default)]
    struct MockGitHub {
        token_bodies: StdMutex<Vec<Value>>,
        replies: StdMutex<Vec<Value>>,
        revokes: AtomicU32,
        revoke_status: StdMutex<u16>,
        /// How long the token endpoint takes to answer, so a concurrency test
        /// has a window to race inside.
        token_delay_ms: AtomicU32,
    }

    impl MockGitHub {
        fn token_calls(&self) -> usize {
            self.token_bodies.lock().unwrap().len()
        }
    }

    /// A GitHub that answers every exchange and refresh with `reply`.
    async fn spawn_github(reply: Value) -> (String, Arc<MockGitHub>) {
        let mock = Arc::new(MockGitHub {
            replies: StdMutex::new(vec![reply]),
            revoke_status: StdMutex::new(204),
            ..MockGitHub::default()
        });
        let app = Router::new()
            .route(
                "/login/oauth/access_token",
                post(
                    |State(mock): State<Arc<MockGitHub>>, body: String| async move {
                        let delay = mock.token_delay_ms.load(Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(delay as u64))
                                .await;
                        }
                        mock.token_bodies
                            .lock()
                            .unwrap()
                            .push(serde_json::from_str(&body).unwrap());
                        let mut replies = mock.replies.lock().unwrap();
                        let reply = if replies.len() > 1 {
                            replies.remove(0)
                        } else {
                            replies[0].clone()
                        };
                        Json(reply)
                    },
                ),
            )
            .route(
                "/applications/{client_id}/grant",
                delete(|State(mock): State<Arc<MockGitHub>>| async move {
                    mock.revokes.fetch_add(1, Ordering::SeqCst);
                    StatusCode::from_u16(*mock.revoke_status.lock().unwrap()).unwrap()
                }),
            )
            .with_state(mock.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), mock)
    }

    fn expiring_reply(access: &str, refresh: &str) -> Value {
        json!({
            "access_token": access,
            "expires_in": 28800,
            "refresh_token": refresh,
            "refresh_token_expires_in": 15_811_200,
            "token_type": "bearer",
        })
    }

    /// A [`ConnectAuth`] that validates any token as one fixed login, with no
    /// device path and no network behind it.
    struct AcceptingAuth(&'static str);

    #[async_trait::async_trait]
    impl ConnectAuth for AcceptingAuth {
        async fn start_device_flow(
            &self,
            _auth_base: &str,
            _client_id: &str,
        ) -> std::result::Result<crystalline_remote::DeviceFlowStart, RemoteError> {
            Err(RemoteError::NotConnected)
        }

        async fn run_device_flow(
            &self,
            _auth_base: &str,
            _client_id: &str,
            _start: &crystalline_remote::DeviceFlowStart,
        ) -> std::result::Result<String, RemoteError> {
            Err(RemoteError::NotConnected)
        }

        async fn validate_token(
            &self,
            _api_url: Option<&str>,
            _token: &str,
        ) -> std::result::Result<String, RemoteError> {
            Ok(self.0.to_string())
        }
    }

    /// An engine whose credentials live in a tempdir file store and whose
    /// GitHub is the mock at `api_url`, never the developer's keychain and
    /// never the real network.
    async fn web_engine(
        tmp: &tempfile::TempDir,
        api_url: Option<&str>,
        client_id: Option<&str>,
        client_secret: Option<&str>,
        public_url: Option<&str>,
    ) -> Engine {
        let store = TursoStore::open_in_memory().await.unwrap();
        let tokens = tmp.path().join("tokens");
        std::fs::create_dir_all(&tokens).unwrap();
        let config = GlobalConfig {
            github: Some(GitHubConfig {
                enabled: Some(true),
                share_identity: Some("personal".to_string()),
                api_url: api_url.map(str::to_string),
                oauth_client_id: client_id.map(str::to_string),
                oauth_client_secret: client_secret.map(str::to_string),
                ..GitHubConfig::default()
            }),
            service: Some(ServiceConfig {
                public_url: public_url.map(str::to_string),
                ..ServiceConfig::default()
            }),
            ..GlobalConfig::default()
        };
        Engine::new(
            Arc::new(Mutex::new(store)),
            config,
            None,
            Some(tmp.path().join("config.yaml")),
        )
        .with_token_store_dir(tokens)
        .with_connect_auth(Arc::new(AcceptingAuth("alice-gh")))
    }

    /// A fully configured engine against a mock GitHub that hands out one
    /// expiring pair.
    async fn connected_engine(tmp: &tempfile::TempDir) -> (Engine, Arc<MockGitHub>) {
        let (api_url, mock) = spawn_github(expiring_reply("ghu_first", "ghr_first")).await;
        let engine = web_engine(
            tmp,
            Some(&api_url),
            Some(CLIENT_ID),
            Some(CLIENT_SECRET),
            Some(PUBLIC_URL),
        )
        .await;
        (engine, mock)
    }

    fn query_value(url: &str, key: &str) -> String {
        let (_, query) = url.split_once('?').expect("no query");
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key} in {url}"))
            .to_string()
    }

    /// The token on file for one account, read back the way the store wrote it.
    fn stored_for(tmp: &tempfile::TempDir, account: &str) -> Option<StoredToken> {
        TokenStore::file_fallback_for(
            &TokenIdentity::Personal(account.to_string()),
            &tmp.path().join("tokens"),
        )
        .unwrap()
        .load()
        .unwrap()
    }

    /// Writes an expiring credential for `account`, standing in for a web
    /// link that landed some hours ago.
    fn write_expiring(tmp: &tempfile::TempDir, account: &str, expires_in: i64) {
        TokenStore::file_fallback_for(
            &TokenIdentity::Personal(account.to_string()),
            &tmp.path().join("tokens"),
        )
        .unwrap()
        .save(&StoredToken {
            access_token: "ghu_old".to_string(),
            host: "github.com".to_string(),
            user: "alice-gh".to_string(),
            created_at: Utc::now() - Duration::hours(8),
            refresh_token: Some("ghr_old".to_string()),
            expires_at: Some(Utc::now() + Duration::seconds(expires_in)),
            refresh_token_expires_at: Some(Utc::now() + Duration::days(180)),
        })
        .unwrap();
    }

    #[tokio::test]
    async fn a_link_is_minted_for_one_account_and_points_at_this_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _mock) = connected_engine(&tmp).await;

        let start = engine.start_github_web_connect("alice").await.unwrap();

        assert!(
            start
                .url
                .starts_with("https://kb.example.test/api/v1/auth/github/connect?n="),
            "{}",
            start.url
        );
        assert_eq!(start.expires_in_secs, 300);
    }

    #[tokio::test]
    async fn a_nonce_is_spendable_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _mock) = connected_engine(&tmp).await;
        let nonce = query_value(
            &engine.start_github_web_connect("alice").await.unwrap().url,
            "n",
        );

        let redirect = engine.github_web_authorize("alice", &nonce).unwrap();
        assert!(
            redirect.starts_with(&format!(
                "{}/login/oauth/authorize?",
                engine.web_config().unwrap().app.auth_base()
            )),
            "{redirect}"
        );
        assert!(
            redirect.contains("code_challenge_method=S256"),
            "{redirect}"
        );

        let again = engine.github_web_authorize("alice", &nonce).unwrap_err();
        assert!(
            matches!(again, EngineError::Invalid(ref text) if text.contains("already been used")),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn a_nonce_stops_being_spendable_once_its_five_minutes_are_up() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _mock) = connected_engine(&tmp).await;
        let nonce = query_value(
            &engine.start_github_web_connect("alice").await.unwrap().url,
            "n",
        );

        engine.web_connect.lock().unwrap().nonces[0].expires_at = Utc::now() - Duration::seconds(1);

        let expired = engine.github_web_authorize("alice", &nonce).unwrap_err();
        assert!(matches!(expired, EngineError::Invalid(_)), "{expired:?}");
    }

    #[tokio::test]
    async fn a_nonce_another_account_presents_is_forbidden_and_spent() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _mock) = connected_engine(&tmp).await;
        let nonce = query_value(
            &engine.start_github_web_connect("alice").await.unwrap().url,
            "n",
        );

        let stolen = engine.github_web_authorize("bob", &nonce).unwrap_err();
        assert!(
            matches!(stolen, EngineError::Forbidden(ref text) if text.contains("another account")),
            "{stolen:?}"
        );

        // Single use means spent on the refusal too, so the link the victim
        // was sent cannot be retried by anybody, its owner included.
        let after = engine.github_web_authorize("alice", &nonce).unwrap_err();
        assert!(matches!(after, EngineError::Invalid(_)), "{after:?}");
    }

    #[tokio::test]
    async fn a_state_is_spendable_exactly_once_and_an_unknown_one_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _mock) = connected_engine(&tmp).await;
        let nonce = query_value(
            &engine.start_github_web_connect("alice").await.unwrap().url,
            "n",
        );
        let state = query_value(
            &engine.github_web_authorize("alice", &nonce).unwrap(),
            "state",
        );

        let unknown = engine
            .complete_github_web_connect("not-a-state", "the-code")
            .await
            .unwrap_err();
        assert!(matches!(unknown, EngineError::Invalid(_)), "{unknown:?}");

        engine
            .complete_github_web_connect(&state, "the-code")
            .await
            .unwrap();

        let again = engine
            .complete_github_web_connect(&state, "the-code")
            .await
            .unwrap_err();
        assert!(matches!(again, EngineError::Invalid(_)), "{again:?}");
    }

    #[tokio::test]
    async fn a_completed_link_stores_the_rotating_pair_for_the_account() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, mock) = connected_engine(&tmp).await;
        let nonce = query_value(
            &engine.start_github_web_connect("alice").await.unwrap().url,
            "n",
        );
        let redirect = engine.github_web_authorize("alice", &nonce).unwrap();
        let state = query_value(&redirect, "state");

        let identity = engine
            .complete_github_web_connect(&state, "the-code")
            .await
            .unwrap();

        assert!(identity.connected);
        assert_eq!(identity.account, "alice");
        assert_eq!(identity.login.as_deref(), Some("alice-gh"));

        let sent = mock.token_bodies.lock().unwrap()[0].clone();
        assert_eq!(sent["client_id"], CLIENT_ID);
        assert_eq!(sent["client_secret"], CLIENT_SECRET);
        assert_eq!(sent["code"], "the-code");
        assert_eq!(
            sent["redirect_uri"],
            "https://kb.example.test/api/v1/auth/github/callback"
        );
        assert!(sent["code_verifier"].as_str().unwrap().len() >= 43);

        let stored = stored_for(&tmp, "alice").expect("no credential on file");
        assert_eq!(stored.access_token, "ghu_first");
        assert_eq!(stored.refresh_token.as_deref(), Some("ghr_first"));
        assert_eq!(stored.user, "alice-gh");
        assert!(stored.expires_at.is_some());
        assert!(stored.refresh_token_expires_at.is_some());
    }

    #[tokio::test]
    async fn a_credential_that_is_not_about_to_expire_is_not_refreshed() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, mock) = connected_engine(&tmp).await;
        write_expiring(&tmp, "alice", 3600);

        engine
            .refresh_personal_credential(&TokenIdentity::Personal("alice".to_string()), None)
            .await
            .unwrap();

        assert_eq!(mock.token_calls(), 0);
        assert_eq!(
            stored_for(&tmp, "alice").unwrap().access_token,
            "ghu_old",
            "nothing was rotated"
        );
    }

    #[tokio::test]
    async fn concurrent_callers_refresh_an_expiring_credential_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (api_url, mock) = spawn_github(expiring_reply("ghu_rotated", "ghr_rotated")).await;
        mock.token_delay_ms.store(50, Ordering::SeqCst);
        let engine = Arc::new(
            web_engine(
                &tmp,
                Some(&api_url),
                Some(CLIENT_ID),
                Some(CLIENT_SECRET),
                Some(PUBLIC_URL),
            )
            .await,
        );
        let host = engine.github_token_host();
        write_expiring(&tmp, "alice", 60);

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let engine = engine.clone();
            let host = host.clone();
            tasks.push(tokio::spawn(async move {
                engine
                    .refresh_personal_credential(
                        &TokenIdentity::Personal("alice".to_string()),
                        host.as_deref(),
                    )
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        assert_eq!(mock.token_calls(), 1, "one refresh request, not eight");
        let sent = mock.token_bodies.lock().unwrap()[0].clone();
        assert_eq!(sent["grant_type"], "refresh_token");
        assert_eq!(sent["refresh_token"], "ghr_old");

        let stored = stored_for(&tmp, "alice").unwrap();
        assert_eq!(stored.access_token, "ghu_rotated");
        assert_eq!(stored.refresh_token.as_deref(), Some("ghr_rotated"));
        // The cache serves the rotated pair too, so the share that asked for
        // the refresh does not read the token that was just replaced.
        let (_store, cached) = engine
            .github_credential_for(
                &TokenIdentity::Personal("alice".to_string()),
                host.as_deref(),
            )
            .unwrap();
        assert_eq!(cached.unwrap().access_token, "ghu_rotated");
    }

    #[tokio::test]
    async fn a_refresh_token_github_no_longer_accepts_asks_for_a_new_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let (api_url, mock) = spawn_github(json!({ "error": "bad_refresh_token" })).await;
        let engine = web_engine(
            &tmp,
            Some(&api_url),
            Some(CLIENT_ID),
            Some(CLIENT_SECRET),
            Some(PUBLIC_URL),
        )
        .await;
        let host = engine.github_token_host();
        write_expiring(&tmp, "alice", 60);

        let failed = engine
            .refresh_personal_credential(
                &TokenIdentity::Personal("alice".to_string()),
                host.as_deref(),
            )
            .await
            .unwrap_err();

        assert_eq!(mock.token_calls(), 1);
        assert!(
            matches!(failed, EngineError::Remote(RemoteError::AuthExpired)),
            "{failed:?}"
        );
        assert!(
            failed.to_string().contains("sign in again"),
            "{failed}: the connect-again path's own words"
        );
    }

    #[tokio::test]
    async fn an_unconfigured_instance_never_refreshes_and_says_nothing_about_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (api_url, mock) = spawn_github(expiring_reply("ghu_rotated", "ghr_rotated")).await;
        let engine = web_engine(&tmp, Some(&api_url), None, None, None).await;
        write_expiring(&tmp, "alice", 60);

        engine
            .refresh_personal_credential(&TokenIdentity::Personal("alice".to_string()), None)
            .await
            .unwrap();

        assert_eq!(mock.token_calls(), 0);
    }

    #[tokio::test]
    async fn a_disconnect_revokes_the_grant_on_github() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, mock) = connected_engine(&tmp).await;
        write_expiring(&tmp, "alice", 3600);

        let identity = engine.disconnect_github_identity("alice").await.unwrap();

        assert!(!identity.connected);
        assert_eq!(mock.revokes.load(Ordering::SeqCst), 1);
        assert!(stored_for(&tmp, "alice").is_none());
    }

    #[tokio::test]
    async fn a_disconnect_survives_a_revoke_github_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, mock) = connected_engine(&tmp).await;
        *mock.revoke_status.lock().unwrap() = 500;
        write_expiring(&tmp, "alice", 3600);

        let identity = engine.disconnect_github_identity("alice").await.unwrap();

        assert!(!identity.connected, "the credential is forgotten anyway");
        assert_eq!(mock.revokes.load(Ordering::SeqCst), 1);
        assert!(stored_for(&tmp, "alice").is_none());
    }

    #[tokio::test]
    async fn a_disconnect_revokes_without_a_public_url_which_only_the_browser_half_needs() {
        let tmp = tempfile::tempdir().unwrap();
        let (api_url, mock) = spawn_github(json!({})).await;
        let engine = web_engine(
            &tmp,
            Some(&api_url),
            Some(CLIENT_ID),
            Some(CLIENT_SECRET),
            None,
        )
        .await;
        write_expiring(&tmp, "alice", 3600);

        engine.disconnect_github_identity("alice").await.unwrap();

        assert_eq!(mock.revokes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_disconnect_with_no_app_configured_revokes_nothing_and_still_works() {
        let tmp = tempfile::tempdir().unwrap();
        let (api_url, mock) = spawn_github(json!({})).await;
        let engine = web_engine(&tmp, Some(&api_url), None, None, None).await;
        write_expiring(&tmp, "alice", 3600);

        engine.disconnect_github_identity("alice").await.unwrap();

        assert_eq!(mock.revokes.load(Ordering::SeqCst), 0);
        assert!(stored_for(&tmp, "alice").is_none());
    }

    #[tokio::test]
    async fn every_missing_setting_refuses_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        for (client_id, secret, public_url, named) in [
            (
                Some(CLIENT_ID),
                None,
                Some(PUBLIC_URL),
                "github.oauth_client_secret",
            ),
            (
                None,
                Some(CLIENT_SECRET),
                Some(PUBLIC_URL),
                "github.oauth_client_id",
            ),
            (
                Some(CLIENT_ID),
                Some(CLIENT_SECRET),
                None,
                "service.public_url",
            ),
        ] {
            let engine = web_engine(&tmp, None, client_id, secret, public_url).await;
            let refusal = engine.start_github_web_connect("alice").await.unwrap_err();
            assert!(
                matches!(refusal, EngineError::Refused(ref text) if text.contains(named)),
                "{refusal:?} should name {named}"
            );
        }
    }

    #[tokio::test]
    async fn a_read_only_instance_refuses_to_start_or_complete_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, _mock) = connected_engine(&tmp).await;
        engine.read_only = true;

        assert!(matches!(
            engine.start_github_web_connect("alice").await.unwrap_err(),
            EngineError::ReadOnly
        ));
        assert!(matches!(
            engine
                .complete_github_web_connect("any-state", "any-code")
                .await
                .unwrap_err(),
            EngineError::ReadOnly
        ));
    }

    #[tokio::test]
    async fn the_pending_store_is_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _mock) = connected_engine(&tmp).await;

        for _ in 0..(MAX_PENDING + 10) {
            engine.start_github_web_connect("alice").await.unwrap();
        }

        assert_eq!(engine.web_connect.lock().unwrap().nonces.len(), MAX_PENDING);
    }

    #[test]
    fn a_minted_secret_is_a_fresh_256_bit_url_safe_token() {
        let first = secret_token();
        assert_eq!(first.len(), 43);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{first}"
        );
        assert_ne!(secret_token(), first);
    }

    #[test]
    fn constant_time_eq_matches_ordinary_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
