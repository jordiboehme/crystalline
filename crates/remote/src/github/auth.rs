//! GitHub identity for this machine: the OAuth device flow, GitHub
//! Enterprise Server (GHES) endpoint derivation and personal access token
//! validation.
//!
//! The device flow talks to GitHub's OAuth authorization server
//! (`github.com/login/...` on GitHub.com, the bare GHES host on an
//! Enterprise Server instance), a different service from the REST API
//! [`super::GitHubProvider`] talks to, so every function here takes its
//! target host explicitly rather than reusing `GitHubProvider`'s api url.
//! Validating an already-issued token, though, is exactly a REST API call,
//! so [`validate_token`] goes straight through
//! [`super::GitHubProvider::current_user`] instead of duplicating that
//! request.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::GitHubProvider;
use crate::error::RemoteError;
use crate::provider::Provider;

/// The Crystalline OAuth App's client id, sent on every device flow
/// request.
///
/// GitHub OAuth App client ids are public by design: they identify the
/// application, not a secret, and are meant to appear in requests made from
/// end-user machines. `github.oauth_client_id` in config overrides this
/// default for anyone using their own registered app instead.
pub const GITHUB_CLIENT_ID: &str = "Ov23liuV2jnyZYpHU6TM";

/// GitHub's own REST API base url, recognized as the default case in
/// [`auth_base`].
const DEFAULT_API_URL: &str = "https://api.github.com";

/// The GitHub.com host that serves both the default REST API and the OAuth
/// device flow endpoints.
const DEFAULT_AUTH_BASE: &str = "https://github.com";

/// How long a request against the device flow endpoints is allowed to take
/// before it is treated as offline. Generous for a slow connection, short
/// enough that a stalled one is reported rather than hanging forever.
const AUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How much longer to wait between polls after GitHub answers `slow_down`,
/// per the OAuth device flow specification.
const SLOW_DOWN_BACKOFF: Duration = Duration::from_secs(5);

/// Derives the OAuth device flow host from a domain's configured REST API
/// base url. `https://api.github.com`, or no override at all, means
/// GitHub.com, whose OAuth endpoints live at `https://github.com`. A GitHub
/// Enterprise Server api base has the documented shape
/// `https://HOST/api/v3`; GHES serves its OAuth endpoints from the bare
/// host, so that case strips the `/api/v3` suffix.
pub fn auth_base(api_url: Option<&str>) -> String {
    let Some(api_url) = api_url else {
        return DEFAULT_AUTH_BASE.to_string();
    };
    let trimmed = api_url.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == DEFAULT_API_URL {
        return DEFAULT_AUTH_BASE.to_string();
    }
    match trimmed.strip_suffix("/api/v3") {
        Some(host) if !host.is_empty() => host.to_string(),
        _ => trimmed.to_string(),
    }
}

/// The page where a signed-in user sees and revokes the OAuth app
/// authorizations the device flow creates - the place to check that a
/// sign-in landed, as opposed to the personal access tokens page, which
/// never lists it. Derived from the auth base so a GitHub Enterprise
/// Server host gets its own.
pub fn authorized_apps_url(auth_base: &str) -> String {
    format!(
        "{}/settings/connections/applications",
        auth_base.trim_end_matches('/')
    )
}

/// What to do after the code is entered, and how to tell whether it worked,
/// in words a person or a model can act on without guessing. Reused by
/// every surface that reports a pending sign-in so they never drift apart.
pub fn confirmation_guidance(auth_base: &str) -> String {
    format!(
        "Open the verification URL in a browser that is signed in to GitHub, enter the code, \
         then click Authorize on the page that follows - the sign-in stays pending until \
         Authorize is clicked, and closing the tab after entering the code is not enough. Once \
         authorized, the app is listed under {} (Authorized OAuth Apps); it never appears under \
         personal access tokens. An enterprise-managed account can only authorize OAuth apps \
         its enterprise allows - if the browser shows a policy error instead of Authorize, an \
         enterprise admin has to allow the app first.",
        authorized_apps_url(auth_base)
    )
}

/// The outcome of starting a device flow sign-in: everything the caller
/// needs to show the user a code and verification url, and to keep polling
/// for the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFlowStart {
    /// The device code, sent back on every poll; never shown to the user.
    pub device_code: String,
    /// The short code the user types in at `verification_url`.
    pub user_code: String,
    /// Where the user confirms the code, normally
    /// `https://github.com/login/device`.
    pub verification_url: String,
    /// The minimum number of seconds to wait between polls.
    pub interval_secs: u64,
    /// How many seconds from now the device code stops being valid.
    pub expires_in_secs: u64,
}

/// `POST {auth_base}/login/device/code` response. Only the fields
/// [`DeviceFlowStart`] carries are modeled; anything else GitHub sends is
/// silently ignored by serde.
#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

/// `POST {auth_base}/login/device/code` request body. GitHub's OAuth
/// endpoints accept either form-urlencoded or JSON bodies; JSON is used
/// here since the `json` feature is already pulled in for reading
/// responses, avoiding a second reqwest body-encoding feature just for
/// this.
#[derive(Debug, Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
    scope: &'a str,
}

/// Starts a device flow sign-in against `auth_base` (see [`auth_base`] for
/// how to derive it from a domain's api url) using `client_id`, requesting
/// the `repo` OAuth scope Crystalline needs to read and write on the user's
/// behalf.
pub async fn start_device_flow(
    auth_base: &str,
    client_id: &str,
) -> Result<DeviceFlowStart, RemoteError> {
    let response = auth_client()
        .post(format!("{auth_base}/login/device/code"))
        .header("Accept", "application/json")
        .json(&DeviceCodeRequest {
            client_id,
            scope: "repo",
        })
        .send()
        .await
        .map_err(|_| RemoteError::Offline)?;
    let body: DeviceCodeResponse = parse_auth_json(response).await?;
    Ok(DeviceFlowStart {
        device_code: body.device_code,
        user_code: body.user_code,
        verification_url: body.verification_uri,
        interval_secs: body.interval,
        expires_in_secs: body.expires_in,
    })
}

/// One poll of the device flow token endpoint.
#[derive(Debug, PartialEq, Eq)]
pub enum DevicePoll {
    /// The user confirmed the code; here is the access token.
    Token(String),
    /// Not confirmed yet; keep polling no sooner than the current interval.
    Pending,
    /// GitHub asked for a slower polling cadence; the caller should add a
    /// few seconds to its interval before polling again.
    SlowDown,
}

/// `POST {auth_base}/login/oauth/access_token` response. Every outcome
/// (pending, slow down, expired, declined, success) comes back as this same
/// shape rather than through the HTTP status, per the OAuth device flow
/// specification. `error_description` is optional per the spec: GitHub sends
/// it for some error codes and not others, so an undocumented code falls back
/// to repeating the raw code itself rather than leaving the detail blank.
#[derive(Debug, Default, Deserialize)]
struct AccessTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// `POST {auth_base}/login/oauth/access_token` request body.
#[derive(Debug, Serialize)]
struct AccessTokenRequest<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'a str,
}

/// Polls the device flow token endpoint once. Returns
/// [`DevicePoll::Pending`] while the user has not confirmed the code yet,
/// [`DevicePoll::SlowDown`] when GitHub asks for a slower cadence and
/// [`DevicePoll::Token`] once sign-in is confirmed.
/// [`RemoteError::AuthExpired`] means the device code expired before the
/// user confirmed it; [`RemoteError::Api`] with status 403 means the user
/// declined the request on GitHub.
pub async fn poll_device_flow_once(
    auth_base: &str,
    client_id: &str,
    device_code: &str,
) -> Result<DevicePoll, RemoteError> {
    let response = auth_client()
        .post(format!("{auth_base}/login/oauth/access_token"))
        .header("Accept", "application/json")
        .json(&AccessTokenRequest {
            client_id,
            device_code,
            grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        })
        .send()
        .await
        .map_err(|_| RemoteError::Offline)?;
    let body: AccessTokenResponse = parse_auth_json(response).await?;
    if let Some(token) = body.access_token {
        return Ok(DevicePoll::Token(token));
    }
    match body.error.as_deref() {
        Some("authorization_pending") => Ok(DevicePoll::Pending),
        Some("slow_down") => Ok(DevicePoll::SlowDown),
        Some("expired_token") => Err(RemoteError::AuthExpired),
        Some("access_denied") => Err(RemoteError::Api {
            status: 403,
            message: "the sign-in was declined on GitHub".to_string(),
        }),
        Some(other) => {
            // An undocumented device-flow error code: the OAuth device flow
            // specification names five (`authorization_pending`,
            // `slow_down`, `expired_token`, `access_denied` and
            // `unsupported_grant_type`, which GitHub never actually sends
            // here), so this is a real gap rather than a transport failure.
            // `status: 0` would surface a meaningless "status 0" to the user;
            // 200 reflects the successful HTTP response actually received,
            // with the detail carrying the useful information.
            let detail = body.error_description.as_deref().unwrap_or(other);
            Err(RemoteError::Api {
                status: 200,
                message: format!("GitHub answered the sign-in with {other}: {detail}"),
            })
        }
        None => Err(RemoteError::Api {
            status: 0,
            message: "the device flow response had neither a token nor an error".to_string(),
        }),
    }
}

/// Runs the device flow to completion: polls no sooner than
/// `start.interval_secs`, backing off by an extra few seconds each time
/// GitHub answers `slow_down`, until either a token arrives or
/// `start.expires_in_secs` elapses (reported as `RemoteError::AuthExpired`).
/// This is a plain future with no internal task or timeout of its own, so a
/// caller who wants to cancel a pending sign-in wraps it in `select!` or
/// drops the task it was spawned on.
pub async fn run_device_flow(
    auth_base: &str,
    client_id: &str,
    start: &DeviceFlowStart,
) -> Result<String, RemoteError> {
    poll_until_token(start, SLOW_DOWN_BACKOFF, move || {
        poll_device_flow_once(auth_base, client_id, &start.device_code)
    })
    .await
}

/// The waiting-and-backoff loop behind [`run_device_flow`], with the one
/// poll taken as a parameter so the cadence (the initial interval, the
/// `slow_down` backoff growth and the expiry deadline) can be tested on a
/// paused tokio clock, without a socket or a real HTTP round-trip to race
/// the auto-advanced time against. [`run_device_flow`] passes
/// [`poll_device_flow_once`] and the real [`SLOW_DOWN_BACKOFF`], so the
/// shipped flow is exactly this loop.
async fn poll_until_token<P, F>(
    start: &DeviceFlowStart,
    slow_down_backoff: Duration,
    mut poll: P,
) -> Result<String, RemoteError>
where
    P: FnMut() -> F,
    F: Future<Output = Result<DevicePoll, RemoteError>>,
{
    let started = tokio::time::Instant::now();
    let expires_in = Duration::from_secs(start.expires_in_secs);
    let mut interval = Duration::from_secs(start.interval_secs);
    loop {
        tokio::time::sleep(interval).await;
        if started.elapsed() >= expires_in {
            return Err(RemoteError::AuthExpired);
        }
        match poll().await? {
            DevicePoll::Token(token) => return Ok(token),
            DevicePoll::Pending => {}
            DevicePoll::SlowDown => interval += slow_down_backoff,
        }
    }
}

/// Validates a token (typically a personal access token pasted in rather
/// than obtained via the device flow) by asking GitHub who it belongs to.
/// Reuses [`GitHubProvider::current_user`] rather than issuing the `GET
/// /user` request again here.
pub async fn validate_token(api_url: Option<&str>, token: &str) -> Result<String, RemoteError> {
    let provider = GitHubProvider::new(api_url.map(str::to_string), Some(token.to_string()));
    provider.current_user().await
}

/// Builds a short-lived HTTP client for the device flow endpoints. These
/// calls are infrequent (a handful of polls during a single sign-in), so
/// paying for a fresh client each time keeps this module free of shared,
/// initialize-once state.
fn auth_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(AUTH_REQUEST_TIMEOUT)
        .build()
        .expect("the client config here is static and always valid")
}

/// Parses a device flow response body as JSON, mapping a non-2xx status or
/// a malformed body to [`RemoteError::Api`].
async fn parse_auth_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, RemoteError> {
    let status = response.status();
    if !status.is_success() {
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| "no error message provided".to_string());
        return Err(RemoteError::Api {
            status: status.as_u16(),
            message,
        });
    }
    response.json::<T>().await.map_err(|e| RemoteError::Api {
        status: status.as_u16(),
        message: format!("could not parse the response body: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    /// A [`DeviceFlowStart`] with the polling cadence under test; the codes
    /// and urls never matter to [`poll_until_token`], which only reads the
    /// two timing fields.
    fn start_with(interval_secs: u64, expires_in_secs: u64) -> DeviceFlowStart {
        DeviceFlowStart {
            device_code: "devicecode".to_string(),
            user_code: "WDJB-MJHT".to_string(),
            verification_url: "https://github.com/login/device".to_string(),
            interval_secs,
            expires_in_secs,
        }
    }

    // The loop tests below run on a paused tokio clock, so every
    // `sleep` resolves instantly and `Instant::now()` reports exactly the
    // waits the loop asked for. Each records the virtual moment of every
    // poll, which makes the assertion the interval SEQUENCE itself: no wall
    // clock, no socket and no dependency on how loaded the machine is. The
    // HTTP side of a poll is covered separately by the `poll_device_flow_once`
    // tests in `tests/github_auth.rs`.

    #[tokio::test(start_paused = true)]
    async fn the_flow_polls_at_the_start_interval_until_a_token_arrives() {
        let started = tokio::time::Instant::now();
        let mut outcomes = VecDeque::from([
            Ok(DevicePoll::Pending),
            Ok(DevicePoll::Pending),
            Ok(DevicePoll::Token("tok-final".to_string())),
        ]);
        let mut polled_at = Vec::new();
        let start = start_with(5, 900);

        let token = poll_until_token(&start, SLOW_DOWN_BACKOFF, || {
            polled_at.push(started.elapsed());
            std::future::ready(outcomes.pop_front().expect("one poll too many"))
        })
        .await
        .unwrap();

        assert_eq!(token, "tok-final");
        assert_eq!(
            polled_at,
            vec![
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(15)
            ],
            "the loop waited the start interval before every poll"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_flow_grows_the_interval_on_every_slow_down() {
        let started = tokio::time::Instant::now();
        let mut outcomes = VecDeque::from([
            Ok(DevicePoll::Pending),
            Ok(DevicePoll::SlowDown),
            Ok(DevicePoll::SlowDown),
            Ok(DevicePoll::Token("tok-after-slowdown".to_string())),
        ]);
        let mut polled_at = Vec::new();
        let start = start_with(5, 900);

        let token = poll_until_token(&start, SLOW_DOWN_BACKOFF, || {
            polled_at.push(started.elapsed());
            std::future::ready(outcomes.pop_front().expect("one poll too many"))
        })
        .await
        .unwrap();

        assert_eq!(token, "tok-after-slowdown");
        // 5, then 5 again, then 10 once the first slow_down landed, then 15
        // once the second did: five seconds added per slow_down.
        assert_eq!(
            polled_at,
            vec![
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(20),
                Duration::from_secs(35)
            ],
            "each slow_down added the backoff to the interval"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_flow_reports_auth_expired_once_the_window_elapses() {
        let started = tokio::time::Instant::now();
        let mut polled_at = Vec::new();
        let start = start_with(5, 12);

        let err = poll_until_token(&start, SLOW_DOWN_BACKOFF, || {
            polled_at.push(started.elapsed());
            std::future::ready(Ok(DevicePoll::Pending))
        })
        .await
        .unwrap_err();

        assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");
        assert_eq!(
            polled_at,
            vec![Duration::from_secs(5), Duration::from_secs(10)],
            "the wait that crossed the deadline expired instead of polling again"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_poll_ends_the_flow() {
        let start = start_with(5, 900);

        let err = poll_until_token(&start, SLOW_DOWN_BACKOFF, || {
            std::future::ready(Err(RemoteError::Offline))
        })
        .await
        .unwrap_err();

        assert!(matches!(err, RemoteError::Offline), "{err:?}");
    }

    #[test]
    fn auth_base_defaults_to_github_com_when_no_override_is_given() {
        assert_eq!(auth_base(None), "https://github.com");
    }

    #[test]
    fn auth_base_maps_the_default_api_url_to_github_com() {
        assert_eq!(
            auth_base(Some("https://api.github.com")),
            "https://github.com"
        );
        assert_eq!(
            auth_base(Some("https://api.github.com/")),
            "https://github.com"
        );
    }

    #[test]
    fn auth_base_strips_api_v3_for_a_ghes_host() {
        assert_eq!(
            auth_base(Some("https://github.acme.example/api/v3")),
            "https://github.acme.example"
        );
        assert_eq!(
            auth_base(Some("https://github.acme.example/api/v3/")),
            "https://github.acme.example"
        );
    }

    #[test]
    fn authorized_apps_url_points_at_the_applications_page_for_github_com() {
        assert_eq!(
            authorized_apps_url("https://github.com"),
            "https://github.com/settings/connections/applications"
        );
    }

    #[test]
    fn authorized_apps_url_derives_from_a_ghes_auth_base() {
        assert_eq!(
            authorized_apps_url("https://github.example.com"),
            "https://github.example.com/settings/connections/applications"
        );
    }

    #[test]
    fn authorized_apps_url_tolerates_a_trailing_slash_on_the_base() {
        assert_eq!(
            authorized_apps_url("https://github.com/"),
            "https://github.com/settings/connections/applications"
        );
    }

    #[test]
    fn confirmation_guidance_contains_the_derived_url_and_the_word_authorize() {
        let guidance = confirmation_guidance("https://github.com");
        assert!(guidance.contains("https://github.com/settings/connections/applications"));
        assert!(guidance.contains("Authorize"));
    }
}
