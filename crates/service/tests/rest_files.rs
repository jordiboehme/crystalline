//! Endpoint tests for the attachment file routes: the bytes surface at
//! `/api/v1/domains/{domain}/files/{*path}` and the metadata listing at
//! `/api/v1/domains/{domain}/attachments`.
//!
//! Driven over a live listener through the production router construction, like
//! the rest of the REST suite, so the mount point and the shared auth layers are
//! exercised rather than a hand-built sub-router.
//!
//! Two things this file is deliberately literal about. The security headers on a
//! read are asserted as exact strings rather than as "present": a served
//! attachment is the one place this API hands a browser bytes somebody uploaded,
//! and `nosniff` plus a `default-src 'none'; sandbox` policy is what keeps an
//! uploaded SVG from executing in the instance's origin. And the auth matrix is
//! walked per route rather than assumed from the router, because a wildcard route
//! is exactly the shape that gets registered below a guard by accident.
//!
//! Every fixture holds a [`support::ScratchStateDir`]: an attachment write marks
//! its domain pending in the maintenance state file, which lives under the state
//! directory, so a run must never reach the developer's own.

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig,
};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::daemon::http_router;
use crystalline_service::rest::{AuthStore, Role};
use tokio::sync::Mutex;

const ALPHA: &str = "---\ntype: engram\ntitle: Alpha\npermalink: alpha\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Alpha\n\nA rule about alpha.\n";

/// A PNG stand-in: the bytes never have to decode, only to round-trip, so this
/// is a short binary blob with a NUL in it - which is what would break a
/// text-shaped path through the surface.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00binary\x00bytes";

/// A PDF stand-in, for the second inline mime.
const PDF: &[u8] = b"%PDF-1.7\n\x00trailer\n";

/// A slide deck stand-in: the mime that must arrive as a download.
const PPTX: &[u8] = b"PK\x03\x04\x00deck";

/// What a file-routes test server varies.
#[derive(Default)]
struct Options {
    /// `auth.anonymous`: serve a request that carries no identity.
    anonymous: bool,
    /// `service.read_only`: refuse every mutation.
    read_only: bool,
}

struct Fixture {
    addr: std::net::SocketAddr,
    /// The temp directory the `eng` domain lives under, for a test asserting
    /// that an upload landed as a real file.
    root: std::path::PathBuf,
    /// Held for the test's duration: an attachment write marks its domain
    /// pending in the maintenance state file, and this redirects the state
    /// directory into a scratch home so nothing here reaches the developer's.
    _state: support::ScratchStateDir,
    _tmp: tempfile::TempDir,
}

/// Write `name`'s MANIFEST into `dir`, the routing file every domain needs.
fn write_manifest(dir: &std::path::Path, name: &str) {
    std::fs::write(
        dir.join("MANIFEST.md"),
        format!(
            "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
        ),
    )
    .unwrap();
}

/// Serve the production router over a file domain `eng` that already carries one
/// attachment on disk, plus a virtual domain `scratch` that carries none.
///
/// The planted `assets/shot.png` is what lets the read rows of the auth matrix
/// and the read-only fixture assert anything at all: neither may write, so an
/// attachment that was only ever uploaded through this API would leave them with
/// nothing to fetch.
async fn serve(opts: Options) -> Fixture {
    let state = support::ScratchStateDir::acquire();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        auth: Some(AuthConfig {
            trusted_header: None,
            anonymous: Some(opts.anonymous),
            max_users: None,
        }),
        ..GlobalConfig::default()
    };

    let dir = root.join("eng");
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    write_manifest(&dir, "eng");
    std::fs::write(dir.join("alpha.md"), ALPHA).unwrap();
    std::fs::write(dir.join("assets/shot.png"), PNG).unwrap();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(dir));
    cfg.domains
        .insert("scratch".to_string(), DomainEntry::virtual_domain());
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        read_only: Some(opts.read_only),
        ..ServiceConfig::default()
    });

    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    // `service.read_only` is resolved by the daemon rather than by the engine's
    // constructor, so the fixture applies it the way `serve` does.
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_read_only(opts.read_only),
    );
    engine.sync(None).await.unwrap();

    let auth = Arc::new(
        AuthStore::open(&tmp.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("root", "Root", None, Role::Admin, "rootpw")
        .await
        .unwrap();
    auth.add_user("eddy", "Eddy", None, Role::Editor, "eddypw")
        .await
        .unwrap();
    auth.add_user("vera", "Vera", None, Role::Viewer, "verapw")
        .await
        .unwrap();

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
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Fixture {
        addr,
        root: tmp.path().to_path_buf(),
        _state: state,
        _tmp: tmp,
    }
}

/// A client with proxy discovery disabled: the target is loopback, where a
/// system proxy must never be consulted anyway.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// Log in, returning (session cookie value, csrf token).
async fn login(addr: std::net::SocketAddr, name: &str, password: &str) -> (String, String) {
    let resp = client()
        .post(format!("http://{addr}/api/v1/auth/login"))
        .json(&serde_json::json!({"name": name, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login as {name} must succeed");
    let cookie = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fluid_session="))
        .and_then(|v| v.split(';').next())
        .and_then(|v| v.strip_prefix("fluid_session="))
        .unwrap()
        .to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_string())
}

/// A request carrying a session and its token, the browser shape.
fn as_session(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    session: &(String, String),
) -> reqwest::RequestBuilder {
    client()
        .request(method, format!("http://{addr}{path}"))
        .header("cookie", format!("fluid_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

/// Upload `bytes` to `path`, returning the response.
async fn put(
    addr: std::net::SocketAddr,
    session: &(String, String),
    path: &str,
    bytes: &'static [u8],
) -> reqwest::Response {
    as_session(addr, reqwest::Method::PUT, path, session)
        .body(bytes)
        .send()
        .await
        .unwrap()
}

/// A header as a string, failing by name when it is missing.
fn header(resp: &reqwest::Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .unwrap_or_else(|| panic!("the response carries `{name}`: {:?}", resp.headers()))
        .to_str()
        .unwrap()
        .to_string()
}

/// The upload lands, the read hands the bytes back, and every security header a
/// browser needs is on the answer exactly as spelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_round_trips_with_the_security_headers() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let resp = put(
        fx.addr,
        &editor,
        "/api/v1/domains/eng/files/assets/deep/diagram.png",
        PNG,
    )
    .await;
    assert_eq!(resp.status(), 200, "an editor uploads an attachment");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["path"], "assets/deep/diagram.png");
    assert_eq!(body["mime"], "image/png");
    assert_eq!(body["size"], PNG.len());
    assert_eq!(body["sha256"], support::sha256_hex(PNG));

    // The bytes really landed under the domain root, in a subfolder the upload
    // created.
    assert_eq!(
        std::fs::read(fx.root.join("eng/assets/deep/diagram.png")).unwrap(),
        PNG
    );

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/files/assets/deep/diagram.png",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(header(&resp, "content-type"), "image/png");
    assert_eq!(
        header(&resp, "etag"),
        format!("\"{}\"", support::sha256_hex(PNG)),
        "the ETag is the strong quoted sha256 of the bytes served"
    );
    assert_eq!(header(&resp, "x-content-type-options"), "nosniff");
    assert_eq!(
        header(&resp, "content-security-policy"),
        "default-src 'none'; sandbox"
    );
    assert_eq!(header(&resp, "content-disposition"), "inline");
    assert_eq!(
        header(&resp, "cache-control"),
        "no-cache",
        "the ETag is this response's whole freshness story, so a cache has to \
         come back and ask rather than guess a lifetime for it"
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), PNG);
}

/// A cached client that offers the ETag it holds is told the bytes have not
/// changed, and is sent none of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_matching_if_none_match_is_answered_304_with_no_body() {
    let fx = serve(Options::default()).await;
    let viewer = login(fx.addr, "vera", "verapw").await;
    let etag = format!("\"{}\"", support::sha256_hex(PNG));

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/files/assets/shot.png",
        &viewer,
    )
    .header("if-none-match", &etag)
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 304);
    assert_eq!(header(&resp, "etag"), etag, "a 304 still names the version");
    assert_eq!(
        header(&resp, "cache-control"),
        "no-cache",
        "and it repeats the directive, so the stored response keeps having to \
         revalidate rather than becoming heuristically fresh on this answer"
    );
    assert!(
        resp.bytes().await.unwrap().is_empty(),
        "a 304 carries no body"
    );

    // A stale token is not a match, so the bytes come back in full.
    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/files/assets/shot.png",
        &viewer,
    )
    .header("if-none-match", "\"0000000000\"")
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), PNG);
}

/// What a browser does with the bytes follows the mime and nothing else: images,
/// PDFs and text render in place, the office formats download under their own
/// filename.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_disposition_follows_the_mime() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    for (path, bytes, mime, disposition) in [
        ("assets/shot.png", PNG, "image/png", "inline".to_string()),
        (
            "assets/deck.pdf",
            PDF,
            "application/pdf",
            "inline".to_string(),
        ),
        (
            "assets/data.json",
            b"{\"a\":1}" as &'static [u8],
            "application/json",
            "inline".to_string(),
        ),
        (
            "assets/talks/deck.pptx",
            PPTX,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "attachment; filename=\"deck.pptx\"".to_string(),
        ),
    ] {
        let url = format!("/api/v1/domains/eng/files/{path}");
        let resp = put(fx.addr, &editor, &url, bytes).await;
        assert_eq!(resp.status(), 200, "uploading {path}");
        let resp = as_session(fx.addr, reqwest::Method::GET, &url, &editor)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "reading {path}");
        assert_eq!(header(&resp, "content-type"), mime, "the mime of {path}");
        assert_eq!(
            header(&resp, "content-disposition"),
            disposition,
            "the disposition of {path}"
        );
        assert_eq!(resp.bytes().await.unwrap().as_ref(), bytes);
    }
}

/// A path the rules refuse never reaches storage, on either verb, and the answer
/// says which rule refused it.
///
/// The traversal case rides percent-encoded separators on purpose: a URL parser
/// resolves a literal `..` segment away before the request is even sent, so
/// `assets%2F..%2Fx.png` is the only spelling that puts the string a hostile
/// client would send in front of the handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_path_is_a_400_on_every_verb() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    for (path, reason) in [
        ("assets%2F..%2Fx.png", "`.` or `..`"),
        ("assets/x.exe", "allowlisted file extension"),
        ("notes/x.png", "must start with `assets/`"),
    ] {
        let url = format!("/api/v1/domains/eng/files/{path}");

        let resp = put(fx.addr, &editor, &url, PNG).await;
        assert_eq!(resp.status(), 400, "PUT {path}");
        assert_eq!(resp.headers()["content-type"], "application/problem+json");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body["detail"].as_str().unwrap().contains(reason),
            "the refusal names the rule ({reason}): {body}"
        );

        let resp = as_session(fx.addr, reqwest::Method::GET, &url, &editor)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "GET {path}");

        let resp = as_session(fx.addr, reqwest::Method::DELETE, &url, &editor)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "DELETE {path}");
    }

    // Nothing escaped: the domain root holds only the file the fixture planted.
    let listed: Vec<_> = std::fs::read_dir(fx.root.join("eng/assets"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(listed, vec!["shot.png".to_string()]);
    assert!(!fx.root.join("eng/x.png").exists(), "nothing climbed out");
}

/// A well-formed path nobody stored anything at is a 404, not a 400: the
/// difference is what tells a client whether to fix the request or to stop
/// asking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_absent_attachment_is_a_404() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/files/assets/nothing-here.png",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");

    let resp = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng/files/assets/nothing-here.png",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404, "deleting what is not there is a miss");

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/nosuch/files/assets/shot.png",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404, "an unregistered domain is a miss too");
}

/// A delete takes the file and the row with it: the read that follows is a miss
/// and the listing no longer carries it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_removes_the_bytes_and_the_row() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::DELETE,
        "/api/v1/domains/eng/files/assets/shot.png",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 204);
    assert!(
        resp.bytes().await.unwrap().is_empty(),
        "204 carries nothing"
    );
    assert!(!fx.root.join("eng/assets/shot.png").exists());

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/files/assets/shot.png",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);

    let listing = attachments(fx.addr, &editor, "eng").await;
    assert!(
        listing.as_array().unwrap().is_empty(),
        "the listing forgot it too: {listing}"
    );
}

/// The listing: every attachment the domain carries, ordered by path, with the
/// metadata a page renders a file row from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_listing_is_complete_and_ordered() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    // Uploaded out of order, so the ordering is the route's rather than the
    // insertion sequence's.
    for path in [
        "assets/zulu.png",
        "assets/alpha/deck.pptx",
        "assets/mike.pdf",
    ] {
        let resp = put(
            fx.addr,
            &editor,
            &format!("/api/v1/domains/eng/files/{path}"),
            PNG,
        )
        .await;
        assert_eq!(resp.status(), 200, "uploading {path}");
    }

    let listing = attachments(fx.addr, &editor, "eng").await;
    let paths: Vec<&str> = listing
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        vec![
            "assets/alpha/deck.pptx",
            "assets/mike.pdf",
            "assets/shot.png",
            "assets/zulu.png",
        ],
        "sorted by path, and the file the fixture planted on disk is in it: \
         {listing}"
    );

    let row = &listing.as_array().unwrap()[2];
    assert_eq!(row["mime"], "image/png");
    assert_eq!(row["size"], PNG.len());
    assert_eq!(row["sha256"], support::sha256_hex(PNG));
    assert!(
        !row["modified"].as_str().unwrap().is_empty(),
        "every row carries its modification instant: {row}"
    );

    // A domain that carries nothing answers with an empty list rather than a
    // miss.
    let empty = attachments(fx.addr, &editor, "scratch").await;
    assert!(empty.as_array().unwrap().is_empty(), "{empty}");
}

/// An empty listing and an absent domain are two different answers: a
/// registered domain holding nothing is `200` with no rows (asserted above),
/// and a domain nobody registered is a miss.
///
/// Asserted on the listing route in its own right rather than inferred from the
/// bytes route's unknown-domain case: the two reach `domain_source` through
/// different engine verbs, and a client that branched on the difference would
/// have nothing pinning it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listing_an_unregistered_domain_is_a_404() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/nosuch/attachments",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(resp.headers()["content-type"], "application/problem+json");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], 404);
    assert!(
        body["detail"].as_str().unwrap().contains("nosuch"),
        "the refusal names the domain that is not there: {body}"
    );
}

/// The listing of one domain, asserting the 200 on the way through.
async fn attachments(
    addr: std::net::SocketAddr,
    session: &(String, String),
    domain: &str,
) -> serde_json::Value {
    let resp = as_session(
        addr,
        reqwest::Method::GET,
        &format!("/api/v1/domains/{domain}/attachments"),
        session,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200, "GET the {domain} attachments");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["attachments"].clone()
}

/// A virtual domain has no filesystem, and the surface above the seam does not
/// care: the same upload and the same read carry the same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_virtual_domain_round_trips_the_bytes() {
    let fx = serve(Options::default()).await;
    let editor = login(fx.addr, "eddy", "eddypw").await;

    let resp = put(
        fx.addr,
        &editor,
        "/api/v1/domains/scratch/files/assets/deck.pptx",
        PPTX,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["sha256"], support::sha256_hex(PPTX));

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/scratch/files/assets/deck.pptx",
        &editor,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        header(&resp, "content-type"),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    );
    assert_eq!(
        header(&resp, "content-disposition"),
        "attachment; filename=\"deck.pptx\""
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), PPTX);

    let listing = attachments(fx.addr, &editor, "scratch").await;
    assert_eq!(listing.as_array().unwrap().len(), 1, "{listing}");
    assert_eq!(listing[0]["path"], "assets/deck.pptx");
}

/// The auth matrix over all four routes: reads need a viewer, writes need an
/// editor and a CSRF token, and nothing at all is served to a caller with no
/// identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_auth_matrix_holds_on_the_file_routes() {
    let fx = serve(Options::default()).await;
    let viewer = login(fx.addr, "vera", "verapw").await;
    let editor = login(fx.addr, "eddy", "eddypw").await;
    let file = "/api/v1/domains/eng/files/assets/shot.png";
    let listing = "/api/v1/domains/eng/attachments";

    // No identity: 401 ahead of routing, on reads and writes alike.
    for (method, path) in [
        (reqwest::Method::GET, file),
        (reqwest::Method::PUT, file),
        (reqwest::Method::DELETE, file),
        (reqwest::Method::GET, listing),
    ] {
        let resp = client()
            .request(method.clone(), format!("http://{}{path}", fx.addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 401, "{method} {path} with no identity");
    }

    // A viewer reads both routes and writes neither.
    for path in [file, listing] {
        let resp = as_session(fx.addr, reqwest::Method::GET, path, &viewer)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "a viewer reads {path}");
    }
    let resp = as_session(fx.addr, reqwest::Method::PUT, file, &viewer)
        .body(PNG)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "a viewer never uploads");
    let resp = as_session(fx.addr, reqwest::Method::DELETE, file, &viewer)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "a viewer never deletes");

    // An editor without its CSRF token is refused before any handler logic, and
    // so is one echoing the wrong token.
    for token in [None, Some("wrong")] {
        for method in [reqwest::Method::PUT, reqwest::Method::DELETE] {
            let mut req = client()
                .request(method.clone(), format!("http://{}{file}", fx.addr))
                .header("cookie", format!("fluid_session={}", editor.0))
                .body(PNG);
            if let Some(token) = token {
                req = req.header("x-csrf-token", token);
            }
            assert_eq!(
                req.send().await.unwrap().status(),
                403,
                "{method} with csrf {token:?}"
            );
        }
    }

    // And the editor, with the token, is served.
    let resp = put(fx.addr, &editor, file, PNG).await;
    assert_eq!(resp.status(), 200, "an editor uploads");
    let resp = as_session(fx.addr, reqwest::Method::DELETE, file, &editor)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "and deletes");

    // An anonymous instance serves the reads to a caller who carries nothing and
    // still refuses the writes: an anonymous identity can never write.
    let anon = serve(Options {
        anonymous: true,
        ..Options::default()
    })
    .await;
    for path in [file, listing] {
        let resp = client()
            .get(format!("http://{}{path}", anon.addr))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "the anonymous viewer reads {path}");
    }
    for method in [reqwest::Method::PUT, reqwest::Method::DELETE] {
        let resp = client()
            .request(method.clone(), format!("http://{}{file}", anon.addr))
            .body(PNG)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            401,
            "{method} as the anonymous viewer: told to log in, never served"
        );
    }
}

/// A read-only instance still serves the bytes it holds - that is what a mirror
/// is for - and refuses every change to them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_instance_serves_reads_and_refuses_writes() {
    let fx = serve(Options {
        read_only: true,
        ..Options::default()
    })
    .await;
    let admin = login(fx.addr, "root", "rootpw").await;
    let file = "/api/v1/domains/eng/files/assets/shot.png";

    let resp = as_session(fx.addr, reqwest::Method::GET, file, &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), PNG);

    let resp = as_session(
        fx.addr,
        reqwest::Method::GET,
        "/api/v1/domains/eng/attachments",
        &admin,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = put(fx.addr, &admin, file, PNG).await;
    assert_eq!(resp.status(), 403, "read-only refuses the strongest caller");
    let resp = as_session(fx.addr, reqwest::Method::DELETE, file, &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    assert!(
        fx.root.join("eng/assets/shot.png").exists(),
        "and the bytes are still there"
    );
}
