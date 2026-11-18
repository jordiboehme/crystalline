//! The release client against a local fake of GitHub: what it sends, the
//! ETag, the rate-limit fallback to the release page redirect, and that only
//! a stable release is ever taken.

#[path = "../support/mod.rs"]
mod support;

#[path = "../reflection/fixture.rs"]
mod fixture;

mod check;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, Response, StatusCode};
use crystalline_core::update::{LatestRelease, Lookup};
use crystalline_service::update::{UpdateError, download, latest_release};

/// One request the fake saw.
#[derive(Debug, Clone)]
struct Seen {
    method: Method,
    path: String,
    headers: HeaderMap,
}

type Answer = dyn Fn(&Method, &str, &HeaderMap) -> Response<Body> + Send + Sync;

/// A fake GitHub on 127.0.0.1, answering every request through `answer`
/// and recording it.
struct Fake {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    async fn start(
        answer: impl Fn(&Method, &str, &HeaderMap) -> Response<Body> + Send + Sync + 'static,
    ) -> Fake {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let answer: Arc<Answer> = Arc::new(answer);
        let record = seen.clone();
        let router = Router::new().fallback(move |request: Request<Body>| {
            let record = record.clone();
            let answer = answer.clone();
            async move {
                let (parts, _) = request.into_parts();
                record.lock().unwrap().push(Seen {
                    method: parts.method.clone(),
                    path: parts.uri.path().to_string(),
                    headers: parts.headers.clone(),
                });
                answer(&parts.method, parts.uri.path(), &parts.headers)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Fake {
            base: format!("http://{addr}"),
            seen,
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// The seams are process-wide environment variables, and `cargo test` runs
/// the tests of a binary as threads of one process: every test that sets
/// them holds this lock until it is done.
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Point both seams at `fake`, and drop any proxy the developer's shell
/// sets, which would otherwise carry the requests to 127.0.0.1 away.
fn use_fake(fake: &Fake) {
    // SAFETY: every test that touches the environment holds `ENV`.
    unsafe {
        std::env::set_var("CRYSTALLINE_TEST_UPDATE_API", &fake.base);
        std::env::set_var("CRYSTALLINE_TEST_UPDATE_DOWNLOAD", &fake.base);
        for proxy in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::remove_var(proxy);
        }
    }
}

const API_PATH: &str = "/repos/jordiboehme/crystalline/releases/latest";
const WEB_PATH: &str = "/jordiboehme/crystalline/releases/latest";

fn respond(status: StatusCode, headers: &[(&str, &str)], body: &str) -> Response<Body> {
    let mut response = Response::builder().status(status);
    for (name, value) in headers {
        response = response.header(*name, *value);
    }
    response.body(Body::from(body.to_string())).unwrap()
}

fn release_body(tag: &str, prerelease: bool) -> String {
    serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": prerelease,
        "published_at": "2026-11-17T10:00:00Z",
        "assets": [{
            "name": "crystalline-v0.34.0-linux-amd64.tar.gz",
            "digest": "sha256:0123"
        }]
    })
    .to_string()
}

fn redirect_to(tag: &str) -> Response<Body> {
    respond(
        StatusCode::FOUND,
        &[(
            "location",
            &format!("https://github.com/jordiboehme/crystalline/releases/tag/{tag}"),
        )],
        "",
    )
}

fn fresh(lookup: Lookup) -> (LatestRelease, Option<String>) {
    match lookup {
        Lookup::Fresh(release, etag) => (release, etag),
        Lookup::NotModified => panic!("expected a fresh answer"),
    }
}

#[tokio::test]
async fn sends_only_the_user_agent() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|_, path, _| match path {
        API_PATH => respond(
            StatusCode::OK,
            &[("etag", "\"v1\"")],
            &release_body("v0.34.0", false),
        ),
        _ => respond(StatusCode::OK, &[], "asset"),
    })
    .await;
    use_fake(&fake);
    // S1: a collaboration token in the environment is never sent.
    unsafe {
        std::env::set_var("CRYSTALLINE_GITHUB_TOKEN", "ghp_never_sent");
        std::env::set_var("GITHUB_TOKEN", "ghp_never_sent");
    }
    let (release, etag) = fresh(latest_release(None).await.expect("a lookup"));
    let dir = tempfile::tempdir().unwrap();
    download(
        "v0.34.0",
        "crystalline-v0.34.0-linux-amd64.tar.gz",
        &dir.path().join("asset"),
    )
    .await
    .expect("a download");
    unsafe {
        std::env::remove_var("CRYSTALLINE_GITHUB_TOKEN");
        std::env::remove_var("GITHUB_TOKEN");
    }

    assert_eq!(release.tag, "v0.34.0");
    assert!(release.from_api);
    assert_eq!(
        release
            .digests
            .get("crystalline-v0.34.0-linux-amd64.tar.gz"),
        Some(&"sha256:0123".to_string())
    );
    assert_eq!(
        release
            .published_at
            .map(|at| at.rfc3339_secs_z())
            .as_deref(),
        Some("2026-11-17T10:00:00Z")
    );
    assert_eq!(etag.as_deref(), Some("\"v1\""));
    let seen = fake.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let agent = format!("crystalline/{}", env!("CARGO_PKG_VERSION"));
    for request in &seen {
        assert_eq!(
            request
                .headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok()),
            Some(agent.as_str()),
            "{request:?}"
        );
        assert!(
            !request.headers.contains_key("authorization"),
            "{request:?}"
        );
        assert!(!request.headers.contains_key("cookie"), "{request:?}");
    }
    assert!(
        !seen[0].headers.contains_key("if-none-match"),
        "no ETag to send"
    );
}

#[tokio::test]
async fn not_modified_with_etag() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|_, _, headers| {
        if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"v1\"") {
            respond(StatusCode::NOT_MODIFIED, &[], "")
        } else {
            respond(
                StatusCode::OK,
                &[("etag", "\"v2\"")],
                &release_body("v0.34.0", false),
            )
        }
    })
    .await;
    use_fake(&fake);
    assert_eq!(
        latest_release(Some("\"v1\"")).await.expect("a lookup"),
        Lookup::NotModified
    );
    let (_, etag) = fresh(latest_release(Some("\"v0\"")).await.expect("a lookup"));
    assert_eq!(etag.as_deref(), Some("\"v2\""));
}

#[tokio::test]
async fn rate_limit_403_falls_back_to_the_redirect() {
    let _env = ENV.lock().await;
    for (status, limit) in [
        (StatusCode::FORBIDDEN, ("x-ratelimit-remaining", "0")),
        (StatusCode::TOO_MANY_REQUESTS, ("retry-after", "60")),
        (StatusCode::FORBIDDEN, ("retry-after", "60")),
    ] {
        let fake = Fake::start(move |method, path, _| match (method, path) {
            (&Method::GET, API_PATH) => respond(status, &[limit], "{\"message\":\"rate\"}"),
            (&Method::HEAD, WEB_PATH) => redirect_to("v0.34.0"),
            _ => respond(StatusCode::NOT_FOUND, &[], ""),
        })
        .await;
        use_fake(&fake);
        let (release, etag) = fresh(latest_release(Some("\"v1\"")).await.expect("a lookup"));
        assert_eq!(release.tag, "v0.34.0", "{status} {limit:?}");
        assert_eq!(release.version.render(), "0.34.0");
        assert!(!release.from_api);
        assert!(release.digests.is_empty(), "the fallback has no digests");
        assert_eq!(release.published_at, None);
        assert_eq!(etag, None);
        let seen = fake.seen();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[1].method, Method::HEAD);
        assert_eq!(seen[1].path, WEB_PATH);
    }
}

#[tokio::test]
async fn a_plain_403_is_an_error() {
    let _env = ENV.lock().await;
    for (status, headers) in [
        (StatusCode::FORBIDDEN, &[][..]),
        (
            StatusCode::FORBIDDEN,
            &[("x-ratelimit-remaining", "12")][..],
        ),
        (StatusCode::TOO_MANY_REQUESTS, &[][..]),
    ] {
        let fake = Fake::start(move |method, _, _| match *method {
            Method::HEAD => redirect_to("v0.34.0"),
            _ => respond(status, headers, "{}"),
        })
        .await;
        use_fake(&fake);
        let err = latest_release(None).await.expect_err("an error");
        if status == StatusCode::FORBIDDEN {
            assert!(matches!(err, UpdateError::Refused), "{err:?}");
            assert_eq!(err.to_string(), "GitHub refused the request");
        } else {
            assert!(matches!(err, UpdateError::Status(429)), "{err:?}");
        }
        let seen = fake.seen();
        assert_eq!(seen.len(), 1, "no fallback: {seen:?}");
    }
}

#[tokio::test]
async fn pre_releases_are_skipped() {
    let _env = ENV.lock().await;
    for body in [
        release_body("v0.34.0", true),
        release_body("v0.35.0-rc.1", false),
        release_body("v0.35.0.rc1", false),
        serde_json::json!({"tag_name": "v0.34.0", "draft": true}).to_string(),
    ] {
        let fake = Fake::start(move |_, _, _| respond(StatusCode::OK, &[], &body)).await;
        use_fake(&fake);
        let err = latest_release(None).await.expect_err("not offered");
        assert!(matches!(err, UpdateError::NotARelease), "{err:?}");
    }
    // The fallback refuses a pre-release tag the same way.
    let fake = Fake::start(|method, _, _| match *method {
        Method::HEAD => redirect_to("v0.35.0-rc.1"),
        _ => respond(StatusCode::FORBIDDEN, &[("x-ratelimit-remaining", "0")], ""),
    })
    .await;
    use_fake(&fake);
    let err = latest_release(None).await.expect_err("not offered");
    assert!(matches!(err, UpdateError::NotARelease), "{err:?}");
}

#[tokio::test]
async fn an_answer_that_is_no_release_is_an_error() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|method, _, _| match *method {
        Method::HEAD => respond(
            StatusCode::FOUND,
            &[("location", "https://github.com/login")],
            "",
        ),
        _ => respond(StatusCode::OK, &[], "<html>"),
    })
    .await;
    use_fake(&fake);
    let err = latest_release(None).await.expect_err("no release");
    assert!(matches!(err, UpdateError::BadAnswer), "{err:?}");
}

#[tokio::test]
async fn download_writes_the_asset_and_returns_its_bytes() {
    let _env = ENV.lock().await;
    let fake = Fake::start(|_, path, _| match path {
        "/jordiboehme/crystalline/releases/download/v0.34.0/SHA256SUMS" => {
            respond(StatusCode::OK, &[], "sums")
        }
        _ => respond(StatusCode::NOT_FOUND, &[], ""),
    })
    .await;
    use_fake(&fake);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("SHA256SUMS");
    let bytes = download("v0.34.0", "SHA256SUMS", &dest).await.unwrap();
    assert_eq!(bytes, b"sums");
    assert_eq!(std::fs::read(&dest).unwrap(), b"sums");

    let missing = download("v0.34.0", "SHA256SUMS.minisig", &dir.path().join("sig")).await;
    assert!(
        matches!(missing, Err(UpdateError::Status(404))),
        "{missing:?}"
    );
    assert!(!dir.path().join("sig").exists());

    // A tag that is no stable release and an asset that is no plain file
    // name are refused before any request.
    let before = fake.seen().len();
    for (tag, asset) in [
        ("v0.35.0-rc.1", "SHA256SUMS"),
        ("0.34.0", "SHA256SUMS"),
        ("v0.34.0", "../SHA256SUMS"),
        ("v0.34.0", ""),
        ("v0.34.0", "a?b"),
    ] {
        assert!(
            download(tag, asset, &dir.path().join("x")).await.is_err(),
            "{tag} {asset}"
        );
    }
    assert_eq!(fake.seen().len(), before, "no request was made");
}

#[tokio::test]
async fn an_api_answer_over_the_cap_is_an_error() {
    let _env = ENV.lock().await;
    // 2 MiB of padding around a valid release, once with a length and once
    // streamed without one.
    for streamed in [false, true] {
        let fake = Fake::start(move |_, _, _| {
            let padding = " ".repeat(2 * 1024 * 1024);
            let body = format!("{padding}{}", release_body("v0.34.0", false));
            if streamed {
                let chunks: Vec<Result<String, std::io::Error>> = body
                    .as_bytes()
                    .chunks(64 * 1024)
                    .map(|chunk| Ok(String::from_utf8(chunk.to_vec()).unwrap()))
                    .collect();
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(futures::stream::iter(chunks)))
                    .unwrap()
            } else {
                respond(StatusCode::OK, &[], &body)
            }
        })
        .await;
        use_fake(&fake);
        let err = latest_release(None).await.expect_err("over the cap");
        assert!(
            matches!(err, UpdateError::BadAnswer),
            "streamed={streamed}: {err:?}"
        );
    }
}

#[tokio::test]
async fn a_download_follows_redirects_only_within_github() {
    let _env = ENV.lock().await;
    const ASSET: &str = "/jordiboehme/crystalline/releases/download/v0.34.0/";
    let fake = Fake::start(|_, path, headers| {
        let host = headers.get("host").unwrap().to_str().unwrap().to_string();
        let port = host.rsplit(':').next().unwrap().to_string();
        let to = |location: String| respond(StatusCode::FOUND, &[("location", &location)], "");
        match path.strip_prefix(ASSET) {
            // Within the fake itself, which a debug build allows.
            Some("same") => to(format!("http://{host}/stored/same")),
            // Another host over plain http, and a host that is not GitHub.
            Some("other-host") => to(format!("http://localhost:{port}/stored/x")),
            Some("not-github") => to("https://example.com/stored/x".to_string()),
            Some("github-over-http") => to("http://github.com/stored/x".to_string()),
            Some("loop") => to(format!("http://{host}{ASSET}loop")),
            _ if path.starts_with("/stored/") => respond(StatusCode::OK, &[], "stored"),
            _ => respond(StatusCode::NOT_FOUND, &[], ""),
        }
    })
    .await;
    use_fake(&fake);
    let dir = tempfile::tempdir().unwrap();
    let bytes = download("v0.34.0", "same", &dir.path().join("same"))
        .await
        .expect("a redirect within the fake is followed");
    assert_eq!(bytes, b"stored");

    for asset in ["other-host", "not-github", "github-over-http", "loop"] {
        let err = download("v0.34.0", asset, &dir.path().join(asset))
            .await
            .expect_err("refused");
        assert!(matches!(err, UpdateError::Redirect), "{asset}: {err:?}");
        assert_eq!(
            err.to_string(),
            "the download was redirected to a place that is not GitHub"
        );
    }
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(left, ["same"], "a refused download leaves no file");
    assert!(
        !fake
            .seen()
            .iter()
            .any(|seen| seen.path.starts_with("/stored/x")),
        "no refused target was fetched"
    );
}

#[tokio::test]
async fn an_http_error_carries_no_url() {
    let _env = ENV.lock().await;
    // Nothing listens on the seam: the connection is refused.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let fake = Fake {
        base: base.clone(),
        seen: Arc::default(),
    };
    use_fake(&fake);
    let err = latest_release(None).await.expect_err("no server");
    assert!(matches!(err, UpdateError::Http(_)), "{err:?}");
    assert!(!err.to_string().contains("127.0.0.1"), "{err}");
    assert!(!err.to_string().contains("http://"), "{err}");
}

/// S11: each `CRYSTALLINE_TEST_UPDATE_*` variable is read only inside an
/// item compiled into debug builds alone, in core and in this crate.
#[test]
fn the_seams_are_debug_only() {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut names = Vec::new();
    for krate in ["core", "service"] {
        let mut dirs = vec![crates.join(krate).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let lines: Vec<&str> = text.lines().collect();
                for (at, line) in lines.iter().enumerate() {
                    let Some(start) = line.find("\"CRYSTALLINE_TEST_UPDATE_") else {
                        continue;
                    };
                    let name = line[start + 1..].split('"').next().unwrap().to_string();
                    let item = lines[..at]
                        .iter()
                        .rposition(|l| l.starts_with("fn ") || l.starts_with("pub"))
                        .expect("the read sits in an item");
                    assert_eq!(
                        lines[item - 1].trim(),
                        "#[cfg(debug_assertions)]",
                        "{}:{}: the read of {name} is not debug-only",
                        path.display(),
                        at + 1
                    );
                    names.push(name);
                }
            }
        }
    }
    names.sort();
    assert_eq!(
        names,
        [
            "CRYSTALLINE_TEST_UPDATE_API",
            "CRYSTALLINE_TEST_UPDATE_DOWNLOAD",
            "CRYSTALLINE_TEST_UPDATE_KEY"
        ],
        "each seam is read in exactly one place"
    );
}
