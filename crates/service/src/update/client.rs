//! The release client: the lookup of the newest stable release on GitHub and
//! the download of a release asset.
//!
//! No token is ever sent or read (S1): the lookup is anonymous, with the
//! User-Agent `crystalline/<version>` and no other identifying header (S8).
//! When the API refuses for its rate limit, the tag is read from the
//! redirect of the web page instead, which carries no digest. Proxies come
//! from the environment (`HTTPS_PROXY`, `NO_PROXY` and the like).
//!
//! The parsing of both answers lives in core ([`LatestRelease`]); this module
//! only makes the requests.

use std::path::Path;
use std::time::Duration;

use crystalline_core::update::{LatestRelease, Lookup};
use crystalline_core::version::release_version;
use reqwest::StatusCode;
use reqwest::header::{ETAG, HeaderValue, IF_NONE_MATCH, LOCATION, RETRY_AFTER};

/// GitHub's API root.
const API_BASE: &str = "https://api.github.com";

/// GitHub's web root, which serves the release page redirect and the
/// release downloads.
const WEB_BASE: &str = "https://github.com";

/// The repository the releases come from.
const REPO: &str = "jordiboehme/crystalline";

/// The User-Agent of every request, and the only header that names us.
const USER_AGENT: &str = concat!("crystalline/", env!("CARGO_PKG_VERSION"));

/// How long a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Each request of the lookup, the API request and the fallback, may take
/// this long, so a lookup that falls back takes at most twice this.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);

/// A download may take this long in all: generous, since a release archive
/// is tens of MB on a slow link.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// A download whose next read waits this long ends.
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest asset a download takes; a release archive is far smaller.
const MAX_DOWNLOAD_BYTES: usize = 256 * 1024 * 1024;

/// The largest API answer the lookup reads; a release object with all its
/// assets is tens of KB.
const MAX_API_BODY_BYTES: usize = 1024 * 1024;

/// The most redirects a download follows (GitHub uses one).
const MAX_REDIRECTS: usize = 3;

/// Why a lookup or a download failed. The messages are fixed text: no tag,
/// body, ETag or location from the answer is ever part of one.
#[derive(Debug)]
pub enum UpdateError {
    /// The request did not complete: no connection, a timeout, a broken body.
    Http(reqwest::Error),
    /// A file could not be written.
    Io(std::io::Error),
    /// GitHub refused the request (a 403 that is not about the rate limit).
    Refused,
    /// GitHub answered with a status the client does not take.
    Status(u16),
    /// The newest release is a draft or a pre-release, or its tag is not
    /// exactly `v<major>.<minor>.<patch>`.
    NotARelease,
    /// The answer could not be read: a body that is no release, or a
    /// redirect without a release page.
    BadAnswer,
    /// The asset name is not a plain file name.
    BadAsset,
    /// The download is larger than any release asset.
    TooLarge,
    /// A download was redirected to a place that is not GitHub, or over
    /// plain http, or more than three times.
    Redirect,
    /// Another `crystalline update now` holds the update lock.
    Locked,
    /// The update lock could not be made or taken.
    Lock(std::io::Error),
    /// The release has no asset for this install method on this system.
    NoAsset,
    /// GitHub's API answered but lists no digest for the asset, so the
    /// download cannot be checked against it (fail closed).
    NoDigest,
    /// A download failed its verification.
    Verify(crystalline_core::update::VerifyError),
    /// The binary could not be unpacked from the archive.
    Extract(std::io::Error),
    /// The archive does not hold the binary exactly once as a plain file.
    NoMember,
    /// The new binary's macOS code signature does not meet the release
    /// requirement.
    Codesign,
    /// The new binary does not print the release's version.
    VersionMismatch,
    /// The new binary could not be put over the old one.
    Swap(std::io::Error),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Http(err) => write!(f, "cannot reach GitHub: {err}"),
            UpdateError::Io(err) => write!(f, "cannot write the download: {err}"),
            UpdateError::Refused => f.write_str("GitHub refused the request"),
            UpdateError::Status(status) => write!(f, "GitHub answered with status {status}"),
            UpdateError::NotARelease => {
                f.write_str("the newest release on GitHub is not a stable release")
            }
            UpdateError::BadAnswer => f.write_str("GitHub's answer could not be read"),
            UpdateError::BadAsset => f.write_str("not the name of a release asset"),
            UpdateError::TooLarge => f.write_str("the download is larger than any release asset"),
            UpdateError::Redirect => {
                f.write_str("the download was redirected away from GitHub or too often")
            }
            UpdateError::Locked => f.write_str("another update is running"),
            UpdateError::Lock(err) => write!(f, "cannot take the update lock: {err}"),
            UpdateError::NoAsset => {
                f.write_str("the release has no download for this install on this system")
            }
            UpdateError::NoDigest => f.write_str("GitHub lists no sha256 digest for the download"),
            UpdateError::Verify(err) => write!(f, "verification failed: {err}"),
            UpdateError::Extract(err) => write!(f, "cannot unpack the binary: {err}"),
            UpdateError::NoMember => {
                f.write_str("the archive does not hold the binary once as a plain file")
            }
            UpdateError::Codesign => f.write_str(
                "the new binary's macOS code signature does not meet the release requirement",
            ),
            UpdateError::VersionMismatch => {
                f.write_str("the new binary does not print the release's version")
            }
            UpdateError::Swap(err) => write!(f, "cannot replace the binary: {err}"),
        }
    }
}

impl std::error::Error for UpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            UpdateError::Http(err) => Some(err),
            UpdateError::Io(err)
            | UpdateError::Lock(err)
            | UpdateError::Extract(err)
            | UpdateError::Swap(err) => Some(err),
            UpdateError::Verify(err) => Some(err),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for UpdateError {
    /// The URL is dropped, so no message carries a network-chosen one, such
    /// as the signed asset URL a download was redirected to (S23).
    fn from(err: reqwest::Error) -> Self {
        if err.is_redirect() {
            UpdateError::Redirect
        } else {
            UpdateError::Http(err.without_url())
        }
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(err: std::io::Error) -> Self {
        UpdateError::Io(err)
    }
}

/// The newest stable release: `GET <api>/repos/<repo>/releases/latest`,
/// with `If-None-Match: <etag>` when the last answer had one. On a 403 or
/// 429 that says the rate limit is used up (`x-ratelimit-remaining: 0` or a
/// `retry-after`), the tag is read from the 302 of `HEAD
/// <web>/<repo>/releases/latest` instead (`from_api: false`, no digests).
/// Any other 403 is [`UpdateError::Refused`]. A draft, a pre-release or a
/// tag that is not exactly `v<x.y.z>` is [`UpdateError::NotARelease`].
pub async fn latest_release(etag: Option<&str>) -> Result<Lookup, UpdateError> {
    let client = lookup_client()?;
    let mut request = client.get(format!("{}/repos/{REPO}/releases/latest", api_base()));
    if let Some(value) = etag.and_then(|etag| HeaderValue::from_str(etag).ok()) {
        request = request.header(IF_NONE_MATCH, value);
    }
    let response = request.send().await?;
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED {
        return Ok(Lookup::NotModified);
    }
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        if rate_limited(response.headers()) {
            return from_redirect(&client).await;
        }
        return Err(if status == StatusCode::FORBIDDEN {
            UpdateError::Refused
        } else {
            UpdateError::Status(status.as_u16())
        });
    }
    if !status.is_success() {
        return Err(UpdateError::Status(status.as_u16()));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let Some(body) = read_capped(response, MAX_API_BODY_BYTES).await? else {
        return Err(UpdateError::BadAnswer);
    };
    match LatestRelease::from_api(&body) {
        Ok(Some(release)) => Ok(Lookup::Fresh(release, etag)),
        Ok(None) => Err(UpdateError::NotARelease),
        Err(_) => Err(UpdateError::BadAnswer),
    }
}

/// Download `<web>/<repo>/releases/download/<tag>/<asset>` into the file
/// `dest` and hand back its bytes, so the checks run on exactly what was
/// written. `tag` must be exactly `v<x.y.z>` and `asset` a plain file name.
/// Redirects are followed only over https to `github.com` and
/// `*.githubusercontent.com`, at most three times. The file is
/// written beside `dest` and renamed into place, so `dest` is never left
/// half written; it is still unverified until the caller's checks pass.
pub async fn download(tag: &str, asset: &str, dest: &Path) -> Result<Vec<u8>, UpdateError> {
    if release_version(tag).is_none() {
        return Err(UpdateError::NotARelease);
    }
    if asset.is_empty()
        || asset == "."
        || asset == ".."
        || asset.contains(['/', '\\', '?', '#', '%'])
    {
        return Err(UpdateError::BadAsset);
    }
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(DOWNLOAD_READ_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .https_only(cfg!(not(debug_assertions)))
        .redirect(download_redirects())
        .build()?;
    let response = client
        .get(format!(
            "{}/{REPO}/releases/download/{tag}/{asset}",
            download_base()
        ))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(UpdateError::Status(status.as_u16()));
    }
    let Some(bytes) = read_capped(response, MAX_DOWNLOAD_BYTES).await? else {
        return Err(UpdateError::TooLarge);
    };
    let part = dest.with_file_name(format!(".{asset}.part-{}", std::process::id()));
    let written = match tokio::fs::write(&part, &bytes).await {
        Ok(()) => tokio::fs::rename(&part, dest).await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(err.into());
    }
    Ok(bytes)
}

/// The body of `response`, or `None` when it is longer than `cap` bytes.
async fn read_capped(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<Option<Vec<u8>>, UpdateError> {
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > cap {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(bytes))
}

/// The redirect policy of a download: [`may_follow`] decides each hop, and
/// a chain longer than [`MAX_REDIRECTS`] is refused.
fn download_redirects() -> reqwest::redirect::Policy {
    let test_origin = test_origin();
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if may_follow(attempt.url(), test_origin.as_deref()) {
            attempt.follow()
        } else {
            attempt.error("a redirect away from GitHub")
        }
    })
}

/// Whether a download follows a redirect to `url`: only over https to
/// `github.com` or a host under `githubusercontent.com`, or, in a debug build
/// pointed at a fake, to that fake's own origin.
fn may_follow(url: &reqwest::Url, test_origin: Option<&str>) -> bool {
    if test_origin.is_some_and(|origin| url.origin().ascii_serialization() == origin) {
        return true;
    }
    url.scheme() == "https"
        && url.port().is_none()
        && url
            .host_str()
            .is_some_and(|host| host == "github.com" || host.ends_with(".githubusercontent.com"))
}

/// The origin of the download seam when it points away from GitHub, so a
/// test's fake may redirect within itself.
#[cfg(debug_assertions)]
fn test_origin() -> Option<String> {
    let base = download_base();
    (base != WEB_BASE)
        .then(|| reqwest::Url::parse(&base).ok())
        .flatten()
        .map(|url| url.origin().ascii_serialization())
}

/// A release build has no seam, so no origin but GitHub's.
#[cfg(not(debug_assertions))]
fn test_origin() -> Option<String> {
    None
}

/// The client of the lookup: short timeouts, and redirects not followed, so
/// the fallback sees its 302.
fn lookup_client() -> Result<reqwest::Client, UpdateError> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(LOOKUP_TIMEOUT)
        .https_only(cfg!(not(debug_assertions)))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// Whether a 403 or 429 is GitHub's rate limit.
fn rate_limited(headers: &reqwest::header::HeaderMap) -> bool {
    headers.contains_key(RETRY_AFTER)
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|remaining| remaining.trim() == "0")
}

/// The fallback: the tag from the redirect of the latest release page.
async fn from_redirect(client: &reqwest::Client) -> Result<Lookup, UpdateError> {
    let response = client
        .head(format!("{}/{REPO}/releases/latest", download_base()))
        .send()
        .await?;
    let status = response.status();
    if !status.is_redirection() {
        return Err(UpdateError::Status(status.as_u16()));
    }
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(UpdateError::BadAnswer)?;
    // A location to a release page whose tag is no stable release is the
    // same refusal as the API's; anything else is no release page at all.
    match LatestRelease::from_redirect(location) {
        Some(release) => Ok(Lookup::Fresh(release, None)),
        None if location.contains("/releases/tag/") => Err(UpdateError::NotARelease),
        None => Err(UpdateError::BadAnswer),
    }
}

/// The API root; a debug build takes `CRYSTALLINE_TEST_UPDATE_API` instead
/// when it is set, so a test can answer with a local fake (S11).
#[cfg(debug_assertions)]
fn api_base() -> String {
    std::env::var("CRYSTALLINE_TEST_UPDATE_API")
        .map(|base| base.trim_end_matches('/').to_string())
        .unwrap_or_else(|_| API_BASE.to_string())
}

/// The API root. A release build has no seam.
#[cfg(not(debug_assertions))]
fn api_base() -> String {
    API_BASE.to_string()
}

/// The web root of the redirect and the downloads; a debug build takes
/// `CRYSTALLINE_TEST_UPDATE_DOWNLOAD` instead when it is set (S11).
#[cfg(debug_assertions)]
fn download_base() -> String {
    std::env::var("CRYSTALLINE_TEST_UPDATE_DOWNLOAD")
        .map(|base| base.trim_end_matches('/').to_string())
        .unwrap_or_else(|_| WEB_BASE.to_string())
}

/// The web root of the redirect and the downloads. A release build has no
/// seam.
#[cfg(not(debug_assertions))]
fn download_base() -> String {
    WEB_BASE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn follows(url: &str, test_origin: Option<&str>) -> bool {
        may_follow(&reqwest::Url::parse(url).unwrap(), test_origin)
    }

    #[test]
    fn a_download_follows_only_https_to_github() {
        for url in [
            "https://github.com/jordiboehme/crystalline/releases/download/v0.34.0/a",
            "https://release-assets.githubusercontent.com/x?sig=1",
            "https://objects.githubusercontent.com/x",
        ] {
            assert!(follows(url, None), "{url}");
        }
        for url in [
            "http://github.com/x",
            "http://release-assets.githubusercontent.com/x",
            "https://example.com/x",
            "https://githubusercontent.com.example.com/x",
            "https://evilgithubusercontent.com/x",
            "https://github.com.example.com/x",
            "https://github.com:8443/x",
            "ftp://github.com/x",
        ] {
            assert!(!follows(url, None), "{url}");
        }
    }

    #[test]
    fn a_fake_may_redirect_only_within_itself() {
        let fake = Some("http://127.0.0.1:4000");
        assert!(follows("http://127.0.0.1:4000/elsewhere", fake));
        assert!(!follows("http://127.0.0.1:4001/elsewhere", fake));
        assert!(!follows("http://localhost:4000/elsewhere", fake));
    }
}
